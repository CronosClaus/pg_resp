//! T0/T1/T2 command dispatch: translates parsed RESP args (resp-proto) into
//! resp-store calls and back into a Reply. Bible §3.4 scope — T3 (data
//! structures, pub/sub, RESP3, MULTI/EXEC) is v0.2+ backlog, logged not
//! built; unknown commands fall through to a well-formed `-ERR unknown
//! command` reply, never a hang.
//!
//! Per pgrx-patterns skill §8.8 (empirically confirmed, Phase 2): this
//! function must never panic — a panic here is caught at the per-connection
//! fence in `lib.rs`'s server loop, but that only limits the damage to one
//! dropped connection. Without that fence a panic silently kills the entire
//! server thread (every connection, every future connection) while
//! Postgres itself and the bgworker process stay completely healthy —
//! distinct from and not the same failure as an external SIGKILL (§8.7),
//! which forces PG's own crash-recovery cycle instead. Every path here
//! returns a Reply; nothing unwraps attacker-controlled input.

use crate::glob::glob_match;
use resp_proto::Reply;
use resp_store::{Condition, Expiry, IncrError, Store};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Per-connection state dispatch needs across calls. Deliberately pgrx-free
/// (like the rest of this module) — `pg_resp.password`'s actual GUC value
/// is read once per command in `lib.rs` and passed in as a plain byte
/// slice, keeping this crate's fast-loop testability (no PG needed) intact.
#[derive(Default)]
pub struct ConnState {
    pub authenticated: bool,
}

/// Bible §3.6: "constant-time compare" for `AUTH`. A naive `==` short-
/// circuits on the first differing byte — a timing side-channel an
/// attacker could use to guess the password one byte at a time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn err_wrong_args(cmd: &str) -> Reply {
    Reply::error(format!("ERR wrong number of arguments for '{cmd}' command"))
}

fn err_syntax() -> Reply {
    Reply::error("ERR syntax error")
}

fn err_not_integer() -> Reply {
    Reply::error("ERR value is not an integer or out of range")
}

fn err_invalid_expire(cmd: &str) -> Reply {
    Reply::error(format!("ERR invalid expire time in '{cmd}' command"))
}

fn parse_i64(bytes: &[u8]) -> Option<i64> {
    std::str::from_utf8(bytes).ok()?.parse::<i64>().ok()
}

fn upper(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_uppercase()
}

/// Translate a target epoch time (seconds or milliseconds since Unix epoch)
/// into a monotonic `Instant` deadline, correlated against one wall-clock/
/// monotonic-clock pair captured once per command (`sys_now`/`mono_now`).
fn resolve_absolute_deadline(
    sys_now: SystemTime,
    mono_now: Instant,
    target_epoch_ms: i64,
) -> Instant {
    let now_epoch_ms = sys_now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if target_epoch_ms <= now_epoch_ms {
        // Already in the past: treat as immediately expired. `is_live` uses
        // `now < deadline`, so a deadline equal to mono_now is expired.
        mono_now
    } else {
        mono_now + Duration::from_millis((target_epoch_ms - now_epoch_ms) as u64)
    }
}

#[derive(Default)]
struct SetOptions {
    expiry: Option<Expiry>, // None means "not yet decided"; resolved to Expiry::None if still unset
    condition: Condition,
    want_get: bool,
}

/// Parse SET's trailing options per the resp-protocol skill's decision table:
/// EX/PX/EXAT/PXAT/KEEPTTL mutually exclusive; NX/XX mutually exclusive; GET
/// independent of both. Returns `Err(Reply)` for a syntax/value error.
fn parse_set_options(
    sys_now: SystemTime,
    mono_now: Instant,
    opts: &[Vec<u8>],
) -> Result<SetOptions, Reply> {
    let mut result = SetOptions::default();
    let mut i = 0;
    while i < opts.len() {
        let token = upper(&opts[i]);
        match token.as_str() {
            "NX" => {
                if result.condition != Condition::None {
                    return Err(err_syntax());
                }
                result.condition = Condition::IfNotExists;
            }
            "XX" => {
                if result.condition != Condition::None {
                    return Err(err_syntax());
                }
                result.condition = Condition::IfExists;
            }
            "GET" => result.want_get = true,
            "KEEPTTL" => {
                if result.expiry.is_some() {
                    return Err(err_syntax());
                }
                result.expiry = Some(Expiry::KeepTtl);
            }
            "EX" | "PX" | "EXAT" | "PXAT" => {
                if result.expiry.is_some() {
                    return Err(err_syntax());
                }
                i += 1;
                let Some(raw) = opts.get(i) else {
                    return Err(err_syntax());
                };
                let Some(n) = parse_i64(raw) else {
                    return Err(err_not_integer());
                };
                if n <= 0 {
                    return Err(err_invalid_expire("set"));
                }
                let deadline = match token.as_str() {
                    "EX" => mono_now + Duration::from_secs(n as u64),
                    "PX" => mono_now + Duration::from_millis(n as u64),
                    "EXAT" => resolve_absolute_deadline(sys_now, mono_now, n.saturating_mul(1000)),
                    "PXAT" => resolve_absolute_deadline(sys_now, mono_now, n),
                    _ => unreachable!(),
                };
                result.expiry = Some(Expiry::At(deadline));
            }
            _ => return Err(err_syntax()),
        }
        i += 1;
    }
    Ok(result)
}

pub fn dispatch(
    store: &mut Store,
    sys_now: SystemTime,
    mono_now: Instant,
    args: &[Vec<u8>],
    conn: &mut ConnState,
    required_password: Option<&[u8]>,
    invalidations_lost: u64,
) -> Reply {
    if args.is_empty() {
        // Empty inline / *0 command: no-op. Caller should send nothing back;
        // represented here as a reply the caller knows to suppress. Simplest
        // honest option within the Reply type: an empty array is a safe,
        // well-formed no-reply-ish sentinel callers can special-case if they
        // want true silence — Phase 1 scope keeps it simple and just skips
        // writing a reply for empty commands at the server-loop level.
        return Reply::Array(Some(vec![]));
    }

    let cmd = upper(&args[0]);
    let rest = &args[1..];

    // NOAUTH gate (bible §3.6): only checked at all when a password is
    // actually configured. AUTH/HELLO/QUIT must get through even when not
    // yet authenticated — otherwise a client could never authenticate, or
    // never cleanly disconnect.
    if required_password.is_some()
        && !conn.authenticated
        && !matches!(cmd.as_str(), "AUTH" | "HELLO" | "QUIT")
    {
        return Reply::error("NOAUTH Authentication required.");
    }

    match cmd.as_str() {
        "PING" => match rest.len() {
            0 => Reply::simple("PONG"),
            1 => Reply::bulk(rest[0].clone()),
            _ => err_wrong_args("ping"),
        },
        "ECHO" => match rest.len() {
            1 => Reply::bulk(rest[0].clone()),
            _ => err_wrong_args("echo"),
        },
        // Client-handshake survival, not RESP3 support. Two rounds of
        // empirical correction went into this (docker-based compat matrix,
        // bible §5 Phase 1 gate — recorded in reports/phase1.md and the
        // resp-protocol skill):
        // 1. Original assumption: clients fall back to RESP2 on an
        //    "unknown command" reply to HELLO. First fix attempt instead
        //    made HELLO a real command, replying spec-correct `-NOPROTO`
        //    for unsupported versions (the RESP spec's own documented
        //    behavior for "protocol version too big").
        // 2. That spec-correct NOPROTO reply broke redis-py: its default
        //    `protocol=None` resolves to RESP3 internally
        //    (`check_protocol_version`'s `DEFAULT_RESP_VERSION`), so it
        //    always sends `HELLO 3` first — and its handshake code treats a
        //    NOPROTO response as fatal (no downgrade-and-retry), while it
        //    DID tolerate the original plain "unknown command" reply and
        //    proceeded in RESP2. Real clients apparently read "unknown
        //    command" as "this server predates HELLO entirely, assume
        //    RESP2" and NOPROTO as "this server understands HELLO but
        //    explicitly refuses me" — a harder failure they don't retry.
        // Net result: HELLO succeeds (RESP2 array, never a RESP3 Map — bible
        // D9 forbids implementing real RESP3) for version 2 or absent;
        // anything else gets the exact same reply shape a truly-unknown
        // command would, not NOPROTO — matching what was empirically proven
        // to work, not just what the spec says is correct.
        "HELLO" => {
            let ver = if rest.is_empty() {
                Some(2)
            } else {
                parse_i64(&rest[0])
            };
            match ver {
                Some(2) => Reply::Array(Some(vec![
                    Reply::bulk("server"),
                    Reply::bulk("redis"),
                    Reply::bulk("version"),
                    Reply::bulk("7.0.0"),
                    Reply::bulk("proto"),
                    Reply::Integer(2),
                    Reply::bulk("id"),
                    Reply::Integer(1),
                    Reply::bulk("mode"),
                    Reply::bulk("standalone"),
                    Reply::bulk("role"),
                    Reply::bulk("master"),
                    Reply::bulk("modules"),
                    Reply::Array(Some(vec![])),
                ])),
                _ => Reply::error("ERR unknown command 'HELLO'"),
            }
        }
        "GET" => match rest.len() {
            1 => match store.get(mono_now, &rest[0]) {
                Some(v) => Reply::bulk(v.to_vec()),
                None => Reply::nil(),
            },
            _ => err_wrong_args("get"),
        },
        "SET" => {
            if rest.len() < 2 {
                return err_wrong_args("set");
            }
            let key = &rest[0];
            let value = rest[1].clone();
            let opts = match parse_set_options(sys_now, mono_now, &rest[2..]) {
                Ok(o) => o,
                Err(e) => return e,
            };
            let expiry = opts.expiry.unwrap_or(Expiry::None);
            let outcome = store.set(mono_now, key, value, expiry, opts.condition, opts.want_get);
            if opts.want_get {
                match outcome.old_value {
                    Some(v) => Reply::bulk(v),
                    None => Reply::nil(),
                }
            } else if outcome.applied {
                Reply::ok()
            } else {
                Reply::nil()
            }
        }
        "DEL" => {
            if rest.is_empty() {
                return err_wrong_args("del");
            }
            let keys: Vec<&[u8]> = rest.iter().map(|k| k.as_slice()).collect();
            Reply::Integer(store.del(mono_now, &keys) as i64)
        }
        "EXISTS" => {
            if rest.is_empty() {
                return err_wrong_args("exists");
            }
            let keys: Vec<&[u8]> = rest.iter().map(|k| k.as_slice()).collect();
            Reply::Integer(store.exists(mono_now, &keys) as i64)
        }
        "TTL" => match rest.len() {
            1 => Reply::Integer(store.ttl_seconds(mono_now, &rest[0])),
            _ => err_wrong_args("ttl"),
        },
        "PTTL" => match rest.len() {
            1 => Reply::Integer(store.pttl_millis(mono_now, &rest[0])),
            _ => err_wrong_args("pttl"),
        },
        "EXPIRE" => {
            if rest.len() != 2 {
                return err_wrong_args("expire");
            }
            let Some(secs) = parse_i64(&rest[1]) else {
                return err_not_integer();
            };
            let deadline = if secs <= 0 {
                mono_now // already-expired deadline: EXPIRE with <=0 seconds deletes-on-next-access
            } else {
                mono_now + Duration::from_secs(secs as u64)
            };
            Reply::Integer(store.expire(mono_now, &rest[0], deadline) as i64)
        }
        "INCR" => match rest.len() {
            1 => incr_reply(store, mono_now, &rest[0], 1),
            _ => err_wrong_args("incr"),
        },
        "DECR" => match rest.len() {
            1 => incr_reply(store, mono_now, &rest[0], -1),
            _ => err_wrong_args("decr"),
        },
        "INCRBY" => {
            if rest.len() != 2 {
                return err_wrong_args("incrby");
            }
            let Some(delta) = parse_i64(&rest[1]) else {
                return err_not_integer();
            };
            incr_reply(store, mono_now, &rest[0], delta)
        }
        "DECRBY" => {
            if rest.len() != 2 {
                return err_wrong_args("decrby");
            }
            let Some(delta) = parse_i64(&rest[1]) else {
                return err_not_integer();
            };
            let Some(neg) = delta.checked_neg() else {
                return Reply::error("ERR decrement would overflow");
            };
            incr_reply(store, mono_now, &rest[0], neg)
        }
        "MGET" => {
            if rest.is_empty() {
                return err_wrong_args("mget");
            }
            let keys: Vec<&[u8]> = rest.iter().map(|k| k.as_slice()).collect();
            let values = store.mget(mono_now, &keys);
            Reply::Array(Some(
                values
                    .into_iter()
                    .map(|v| match v {
                        Some(v) => Reply::bulk(v),
                        None => Reply::nil(),
                    })
                    .collect(),
            ))
        }
        "MSET" => {
            if rest.is_empty() || !rest.len().is_multiple_of(2) {
                return err_wrong_args("mset");
            }
            let pairs: Vec<(&[u8], Vec<u8>)> = rest
                .chunks_exact(2)
                .map(|pair| (pair[0].as_slice(), pair[1].clone()))
                .collect();
            store.mset(&pairs);
            Reply::ok()
        }

        // --- T1 (bible §3.4, phase 2) ---
        "AUTH" => {
            if rest.len() != 1 {
                return err_wrong_args("auth");
            }
            match required_password {
                None => Reply::error(
                    "ERR Client sent AUTH, but no password is set. Did you mean AUTH <username> <password>?",
                ),
                Some(expected) => {
                    if constant_time_eq(expected, &rest[0]) {
                        conn.authenticated = true;
                        Reply::ok()
                    } else {
                        Reply::error("WRONGPASS invalid username-password pair or user is disabled.")
                    }
                }
            }
        }
        "SELECT" => match rest.len() {
            1 if rest[0] == b"0" => Reply::ok(),
            1 => Reply::error("ERR DB index is out of range"), // bible §3.4: accept db 0 only
            _ => err_wrong_args("select"),
        },
        "DBSIZE" => {
            if !rest.is_empty() {
                return err_wrong_args("dbsize");
            }
            Reply::Integer(store.stats().keys as i64)
        }
        "FLUSHDB" | "FLUSHALL" => {
            // Single-db model (bible §3.4/§3.6: SELECT accepts db 0 only) —
            // FLUSHDB and FLUSHALL are equivalent here. Accept an optional
            // ASYNC/SYNC argument (real Redis's own syntax) as a no-op,
            // rather than erroring on it — this store has no async path.
            if rest.len() > 1
                || (rest.len() == 1 && !matches!(upper(&rest[0]).as_str(), "ASYNC" | "SYNC"))
            {
                return err_wrong_args(&cmd.to_lowercase());
            }
            store.clear();
            Reply::ok()
        }
        "INFO" => {
            let stats = store.stats();
            let max_memory = stats
                .max_memory_bytes
                .map(|b| b.to_string())
                .unwrap_or_else(|| "0".to_string());
            Reply::bulk(format!(
                "# Server\r\n\
                 redis_version:7.0.0\r\n\
                 pg_resp_version:{}\r\n\
                 # Memory\r\n\
                 used_memory:{}\r\n\
                 maxmemory:{}\r\n\
                 # Stats\r\n\
                 keyspace_hits:{}\r\n\
                 keyspace_misses:{}\r\n\
                 evicted_keys:{}\r\n\
                 invalidations_lost:{}\r\n\
                 # Keyspace\r\n\
                 db0:keys={}\r\n",
                env!("CARGO_PKG_VERSION"),
                stats.used_bytes,
                max_memory,
                stats.hits,
                stats.misses,
                stats.evictions,
                // Not a store counter: this one is incremented by *backends*
                // (see sql.rs), in shared memory, when a post-commit
                // invalidation could not be delivered because the server was
                // unreachable. It has to live outside the store precisely
                // because the store is what was unreachable — the whole point
                // is to count the writes that never got here. Surfaced in INFO
                // so the RESP and SQL views report one number, not two.
                invalidations_lost,
                stats.keys,
            ))
        }
        "SCAN" => {
            if rest.is_empty() {
                return err_wrong_args("scan");
            }
            let Some(cursor) = std::str::from_utf8(&rest[0])
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
            else {
                return Reply::error("ERR invalid cursor");
            };
            let mut pattern: Option<Vec<u8>> = None;
            let mut count: usize = 10; // Valkey's own default (docs/refs/valkey-notes.md)
            let mut i = 1;
            while i < rest.len() {
                match upper(&rest[i]).as_str() {
                    "MATCH" => {
                        i += 1;
                        let Some(p) = rest.get(i) else {
                            return err_syntax();
                        };
                        pattern = Some(p.clone());
                    }
                    "COUNT" => {
                        i += 1;
                        let Some(n) = rest.get(i).and_then(|b| parse_i64(b)) else {
                            return err_not_integer();
                        };
                        if n <= 0 {
                            return err_syntax();
                        }
                        count = n as usize;
                    }
                    _ => return err_syntax(),
                }
                i += 1;
            }
            let (next_cursor, keys) = store.scan(mono_now, cursor, count);
            let filtered: Vec<Reply> = keys
                .into_iter()
                .filter(|k| pattern.as_deref().is_none_or(|p| glob_match(p, k)))
                .map(Reply::bulk)
                .collect();
            Reply::Array(Some(vec![
                Reply::bulk(next_cursor.to_string()),
                Reply::Array(Some(filtered)),
            ]))
        }
        "CLIENT" => {
            if rest.is_empty() {
                return err_wrong_args("client");
            }
            match upper(&rest[0]).as_str() {
                "SETINFO" | "SETNAME" => Reply::ok(),
                "GETNAME" => Reply::bulk(""), // no per-connection name tracking yet; empty, not nil (matches real Redis's default)
                _ => Reply::error(format!(
                    "ERR Unknown CLIENT subcommand or wrong number of arguments for '{}'",
                    String::from_utf8_lossy(&rest[0])
                )),
            }
        }
        "COMMAND" => {
            if rest.is_empty() {
                return Reply::Array(Some(vec![])); // bare COMMAND: stub, empty table
            }
            match upper(&rest[0]).as_str() {
                "COUNT" => Reply::Integer(0),
                "DOCS" => Reply::Array(Some(vec![])),
                _ => Reply::Array(Some(vec![])),
            }
        }
        "QUIT" => Reply::ok(), // lib.rs's caller closes the connection after sending this

        // Test-only failure injection for S6's panic policy. Compiled only
        // under the `debug_panic` feature, which is not in `default` — in a
        // shipping build this arm does not exist and `DEBUG` falls through to
        // `unknown command`, same as any other unimplemented command.
        //
        // It exists because S6's two panic paths have *different* blast radii
        // and the difference is the whole point: a per-connection panic must
        // cost exactly one connection, while a top-level panic must take the
        // process down so the postmaster restarts it. Neither claim is worth
        // anything unless it has actually been triggered on purpose.
        #[cfg(feature = "debug_panic")]
        "DEBUG" => {
            match rest.first().map(|a| upper(a)).as_deref() {
                Some("PANIC-CONNECTION") => panic!("deliberate per-connection panic (debug_panic)"),
                Some("PANIC-TOPLEVEL") => {
                    // Cannot panic here: this call site sits *inside* the
                    // per-connection fence, which would catch it. Instead ask
                    // the server loop to panic from its own body, outside the
                    // fence.
                    crate::DEBUG_PANIC_TOPLEVEL.store(true, std::sync::atomic::Ordering::SeqCst);
                    Reply::ok()
                }
                _ => Reply::error("ERR DEBUG subcommand not supported by pg_resp"),
            }
        }

        // --- T2 (bible §3.4, phase 2) ---
        "SETEX" => {
            if rest.len() != 3 {
                return err_wrong_args("setex");
            }
            let Some(secs) = parse_i64(&rest[1]) else {
                return err_not_integer();
            };
            if secs <= 0 {
                return err_invalid_expire("setex");
            }
            let deadline = mono_now + Duration::from_secs(secs as u64);
            store.set(
                mono_now,
                &rest[0],
                rest[2].clone(),
                Expiry::At(deadline),
                Condition::None,
                false,
            );
            Reply::ok()
        }
        // SETNX has its own reply shape (integer 1/0), distinct from `SET
        // key value NX` (nil/OK) — confirmed via docs/refs/valkey-notes.md,
        // not assumed from memory.
        "SETNX" => {
            if rest.len() != 2 {
                return err_wrong_args("setnx");
            }
            let outcome = store.set(
                mono_now,
                &rest[0],
                rest[1].clone(),
                Expiry::None,
                Condition::IfNotExists,
                false,
            );
            Reply::Integer(outcome.applied as i64)
        }
        "GETDEL" => match rest.len() {
            1 => match store.get_del(mono_now, &rest[0]) {
                Some(v) => Reply::bulk(v),
                None => Reply::nil(),
            },
            _ => err_wrong_args("getdel"),
        },
        "GETEX" => {
            if rest.is_empty() {
                return err_wrong_args("getex");
            }
            let expiry = if rest.len() == 1 {
                None
            } else {
                match upper(&rest[1]).as_str() {
                    "PERSIST" if rest.len() == 2 => Some(Expiry::None),
                    "EX" | "PX" | "EXAT" | "PXAT" if rest.len() == 3 => {
                        let Some(n) = parse_i64(&rest[2]) else {
                            return err_not_integer();
                        };
                        if n <= 0 {
                            return err_invalid_expire("getex");
                        }
                        let token = upper(&rest[1]);
                        let deadline = match token.as_str() {
                            "EX" => mono_now + Duration::from_secs(n as u64),
                            "PX" => mono_now + Duration::from_millis(n as u64),
                            "EXAT" => {
                                resolve_absolute_deadline(sys_now, mono_now, n.saturating_mul(1000))
                            }
                            "PXAT" => resolve_absolute_deadline(sys_now, mono_now, n),
                            _ => unreachable!(),
                        };
                        Some(Expiry::At(deadline))
                    }
                    _ => return err_syntax(),
                }
            };
            match store.get_ex(mono_now, &rest[0], expiry) {
                Some(v) => Reply::bulk(v),
                None => Reply::nil(),
            }
        }
        "PERSIST" => match rest.len() {
            1 => Reply::Integer(store.persist(mono_now, &rest[0]) as i64),
            _ => err_wrong_args("persist"),
        },
        "TYPE" => match rest.len() {
            1 => {
                if store.exists(mono_now, &[&rest[0]]) > 0 {
                    Reply::simple("string") // T0-T2 scope: only strings ever exist (bible D9)
                } else {
                    Reply::simple("none")
                }
            }
            _ => err_wrong_args("type"),
        },
        "RANDOMKEY" => {
            if !rest.is_empty() {
                return err_wrong_args("randomkey");
            }
            match store.random_key(mono_now) {
                Some(k) => Reply::bulk(k),
                None => Reply::nil(),
            }
        }
        "KEYS" => {
            if rest.len() != 1 {
                return err_wrong_args("keys");
            }
            let pattern = &rest[0];
            let matched: Vec<Reply> = store
                .all_keys(mono_now)
                .into_iter()
                .filter(|k| glob_match(pattern, k))
                .map(Reply::bulk)
                .collect();
            Reply::Array(Some(matched))
        }

        _ => Reply::error(format!(
            "ERR unknown command '{}'",
            String::from_utf8_lossy(&args[0])
        )),
    }
}

fn incr_reply(store: &mut Store, now: Instant, key: &[u8], delta: i64) -> Reply {
    match store.incr_by(now, key, delta) {
        Ok(n) => Reply::Integer(n),
        Err(IncrError::NotAnInteger) => err_not_integer(),
        Err(IncrError::Overflow) => Reply::error("ERR increment or decrement would overflow"),
    }
}

#[cfg(test)]
mod tests;
