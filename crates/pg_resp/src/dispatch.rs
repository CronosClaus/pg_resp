//! T0 command dispatch: translates parsed RESP args (resp-proto) into
//! resp-store calls and back into a Reply. Bible §3.4 T0 scope only —
//! T1/T2/T3 (AUTH, SELECT, INFO, CLIENT, COMMAND, data structures, ...) are
//! Phase 2+ per the phase table; unknown commands (including those) fall
//! through to a well-formed `-ERR unknown command` reply, never a hang.
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

use resp_proto::Reply;
use resp_store::{Condition, Expiry, IncrError, Store};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
fn resolve_absolute_deadline(sys_now: SystemTime, mono_now: Instant, target_epoch_ms: i64) -> Instant {
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
            if rest.is_empty() || rest.len() % 2 != 0 {
                return err_wrong_args("mset");
            }
            let pairs: Vec<(&[u8], Vec<u8>)> = rest
                .chunks_exact(2)
                .map(|pair| (pair[0].as_slice(), pair[1].clone()))
                .collect();
            store.mset(&pairs);
            Reply::ok()
        }
        _ => Reply::error(format!("ERR unknown command '{}'", String::from_utf8_lossy(&args[0]))),
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
