//! The `resp` SQL surface — bible §3.3 / `D3`, the Phase 3 deliverable.
//!
//! Two access paths with **deliberately different semantics**, both documented
//! in `docs/semantics.md`:
//!
//! - **Reads** (`resp.get`, `resp.exists`, `resp.ttl`, `resp.keys`,
//!   `resp.stats`) happen *immediately*, outside transaction control. A read is
//!   a question about cache state right now; deferring it to commit would be
//!   meaningless.
//! - **Writes** (`resp.set`, `resp.del`, and the `resp.evict` trigger) are
//!   *queued* and applied in a post-commit callback. A rolled-back transaction
//!   therefore never touches the cache. This is the moat: invalidation becomes
//!   a property of the schema instead of a discipline the application has to
//!   remember.
//!
//! **Where the process boundary is, and why it dictates everything here.**
//! Every SQL connection is its own OS process; the bgworker holding the store
//! is another. A plain Rust `static` is *not* shared between them
//! (pgrx-patterns §6 — this was learned the hard way in spike S2). So these
//! functions reach the store the only way a separate process can: over a
//! socket, via `resp-client`. That also means the cache is genuinely
//! unreachable sometimes, and the post-commit applier has to survive that
//! without taking the backend down with it — see [`apply_queue`].

use crate::INVALIDATIONS_LOST;
use pgrx::callbacks::{
    register_subxact_callback, register_xact_callback, PgSubXactCallbackEvent, PgXactCallbackEvent,
};
use pgrx::prelude::*;
use resp_client::{Client, ClientError};
use resp_proto::Reply;
use std::cell::{Cell, RefCell};
use std::sync::atomic::Ordering;

/// Raise a Postgres ERROR with a primary message and a real `errhint`.
///
/// Why not just `ereport!`: that macro's optional fourth argument sets
/// **errdetail**, not errhint. The two are different fields with different
/// jobs (detail explains what happened, hint suggests what to do about it) and
/// the PG style guide treats them as such, so going through the builder is the
/// only way to put advice where a reader expects to find it.
///
/// Primary messages here follow the PG convention: lowercase, no trailing
/// period (pg-conventions skill).
fn raise(code: PgSqlErrorCode, message: String, hint: &'static str) -> ! {
    pgrx::pg_sys::panic::ErrorReport::new(code, message, "pg_resp")
        .set_hint(hint)
        .report(PgLogLevel::ERROR);
    unreachable!("ERROR-level report does not return")
}

// ---------------------------------------------------------------------------
// Per-backend connection
// ---------------------------------------------------------------------------

thread_local! {
    /// The backend's pooled loopback connection (bible §3.3: "pooled per
    /// backend"). A `thread_local` is exactly right here and not a compromise:
    /// a Postgres backend is a single-threaded process, so per-thread and
    /// per-session are the same scope. The socket is closed by the OS when the
    /// backend exits.
    static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
}

/// Run `f` with the backend's pooled client, creating it on first use.
///
/// The `Client` reconnects internally (`resp-client`'s retry rule), so a
/// bgworker restart under a long-lived session heals without anything here
/// noticing.
fn with_client<T>(f: impl FnOnce(&mut Client) -> T) -> T {
    CLIENT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let (addr, password) = crate::loopback_target();
            *slot = Some(Client::new(addr, password));
        }
        f(slot.as_mut().expect("client was just initialized"))
    })
}

/// Turn a client error into a Postgres error, for the *read* path only.
///
/// Reads are synchronous SQL calls, so raising is correct: the caller asked a
/// question and we could not answer it, and silently returning NULL would be
/// indistinguishable from "the key is absent". The write path must never do
/// this — see [`apply_queue`].
fn read_error(err: ClientError) -> ! {
    match err {
        // The server answered, and said no. That is our own bug (a malformed
        // command from this module), not an operational condition.
        ClientError::Server(msg) => raise(
            PgSqlErrorCode::ERRCODE_INTERNAL_ERROR,
            format!("pg_resp cache rejected the request: {msg}"),
            "this is a pg_resp bug; please report it",
        ),
        ClientError::Auth(msg) => raise(
            PgSqlErrorCode::ERRCODE_INSUFFICIENT_PRIVILEGE,
            format!("pg_resp authentication failed: {msg}"),
            "pg_resp.password must match between the server and this backend; both read \
             the same GUC, so a mismatch means it changed without a restart",
        ),
        other => raise(
            PgSqlErrorCode::ERRCODE_CONNECTION_FAILURE,
            format!("could not reach the pg_resp cache: {other}"),
            "check that pg_resp is in shared_preload_libraries and that its background \
             worker is running; see the server log",
        ),
    }
}

/// Every reply shape this module does not expect is a bug in this module.
fn unexpected_reply(what: &str, reply: &Reply) -> ! {
    raise(
        PgSqlErrorCode::ERRCODE_INTERNAL_ERROR,
        format!("unexpected {what} reply from the pg_resp cache: {reply:?}"),
        "this is a pg_resp bug; please report it",
    )
}

/// Text out of a bulk reply.
///
/// The RESP surface is binary-safe but SQL `text` is not: it cannot hold
/// invalid UTF-8 (or embedded NULs). A value written as raw bytes over the
/// wire and read back through `resp.get()` therefore has to fail loudly rather
/// than be silently mangled. Documented in `docs/semantics.md`.
fn bulk_to_text(reply: Reply, key: &str) -> Option<String> {
    match reply {
        Reply::Bulk(None) => None,
        Reply::Bulk(Some(bytes)) => match String::from_utf8(bytes) {
            Ok(s) if !s.as_bytes().contains(&0) => Some(s),
            _ => raise(
                PgSqlErrorCode::ERRCODE_CHARACTER_NOT_IN_REPERTOIRE,
                format!("cached value for key \"{key}\" is not valid UTF-8 text"),
                "the RESP wire protocol is binary-safe but SQL text is not; read this key \
                 over the RESP port instead",
            ),
        },
        ref other => unexpected_reply("GET", other),
    }
}

// ---------------------------------------------------------------------------
// The post-commit queue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum QueuedOp {
    Set {
        key: String,
        value: String,
        ttl_seconds: Option<i64>,
    },
    Del {
        key: String,
    },
}

#[derive(Debug, Clone)]
struct QueuedItem {
    /// Which (sub)transaction enqueued this. Used to discard work belonging to
    /// a savepoint that later rolled back — see [`register_hooks`].
    subxid: pg_sys::SubTransactionId,
    op: QueuedOp,
}

thread_local! {
    static QUEUE: RefCell<Vec<QueuedItem>> = const { RefCell::new(Vec::new()) };
    /// Whether this transaction's callbacks are already registered. pgrx wipes
    /// its whole callback map at Commit/Abort, so this must be re-armed once
    /// per transaction, not once per backend.
    static HOOKS_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn enqueue(op: QueuedOp) {
    register_hooks();
    // SAFETY: plain PG FFI call on a backend's own (main) thread, which is
    // where every #[pg_extern] runs by definition. Returns the current
    // subtransaction id; no pointers involved.
    let subxid = unsafe { pg_sys::GetCurrentSubTransactionId() };
    QUEUE.with(|q| q.borrow_mut().push(QueuedItem { subxid, op }));
}

/// Arm this transaction's callbacks, exactly once per transaction.
fn register_hooks() {
    if HOOKS_REGISTERED.get() {
        return;
    }
    HOOKS_REGISTERED.set(true);

    // Commit and Abort are mutually exclusive — precisely one fires (pgrx
    // clears its callback map at either, which is also why HOOKS_REGISTERED
    // has to be reset by both).
    register_xact_callback(PgXactCallbackEvent::Commit, || {
        HOOKS_REGISTERED.set(false);
        let ops = QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
        apply_queue(ops);
    });
    register_xact_callback(PgXactCallbackEvent::Abort, || {
        HOOKS_REGISTERED.set(false);
        // The transaction rolled back, so its queued cache writes must simply
        // vanish. This registration is what makes bible §3.3's rollback
        // guarantee true; without it the queue would leak into whatever
        // transaction this backend ran next.
        QUEUE.with(|q| q.borrow_mut().clear());
    });

    // Savepoint / EXCEPTION-block handling. Unlike xact callbacks these are
    // `Fn`, so one registration serves every subtransaction event for the rest
    // of the transaction, and pgrx clears them at outer-transaction end.
    register_subxact_callback(PgSubXactCallbackEvent::AbortSub, |my_subid, _parent| {
        // This subtransaction (and anything nested inside it, which has a
        // higher id since subxids increase monotonically) is gone. Its queued
        // writes go with it.
        QUEUE.with(|q| q.borrow_mut().retain(|item| item.subxid < my_subid));
    });
    register_subxact_callback(PgSubXactCallbackEvent::CommitSub, |my_subid, parent| {
        // The savepoint committed, but the *outer* transaction may still roll
        // back. Re-parent so a later abort at that level still discards this
        // work. Without this, `SAVEPOINT; resp.del(); RELEASE; ROLLBACK` would
        // wrongly apply the delete.
        QUEUE.with(|q| {
            for item in q.borrow_mut().iter_mut() {
                if item.subxid == my_subid {
                    item.subxid = parent;
                }
            }
        });
    });
}

/// Apply the queued operations. Runs inside the post-commit callback.
///
/// **This function may not fail.** Postgres has already committed; the
/// transaction's fate is decided and unchangeable. A panic or `ereport(ERROR)`
/// here aborts the whole backend (pgrx says so in its own source: "They'll
/// cause the Postgres backend to Abort() if we're handling
/// XactEvent::Commit/Abort events") and, combined with the shmem access pgrx
/// mandates, can escalate to a cluster-wide restart. Losing a cache
/// invalidation is a bounded, countable problem; taking down the database
/// because the cache was unreachable is not a trade anyone would accept.
///
/// So every failure path here ends in a WARNING plus an increment of the
/// shared `invalidations_lost` counter, never an error. The honest consequence
/// is stated in `docs/semantics.md` and `docs/invalidation.md`: if the cache
/// is unreachable at commit time, that invalidation is lost and the stale
/// entry survives until its TTL.
fn apply_queue(ops: Vec<QueuedItem>) {
    if ops.is_empty() {
        return;
    }
    let total = ops.len();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut failed = 0usize;
        let mut first_error: Option<String> = None;
        with_client(|client| {
            for item in &ops {
                let result = match &item.op {
                    QueuedOp::Set {
                        key,
                        value,
                        ttl_seconds,
                    } => {
                        let ttl_text = ttl_seconds.map(|t| t.to_string());
                        let mut args: Vec<&[u8]> = vec![b"SET", key.as_bytes(), value.as_bytes()];
                        if let Some(ttl) = ttl_text.as_deref() {
                            args.push(b"EX");
                            args.push(ttl.as_bytes());
                        }
                        client.command(&args)
                    }
                    QueuedOp::Del { key } => client.command(&[b"DEL", key.as_bytes()]),
                };
                if let Err(err) = result {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(err.to_string());
                    }
                }
            }
        });
        (failed, first_error)
    }));

    let (failed, first_error) = match outcome {
        Ok(pair) => pair,
        // A panic escaped the applier. It was caught here rather than being
        // allowed to unwind into Postgres's commit path, where it would have
        // aborted the backend.
        Err(_) => (
            total,
            Some("internal panic while applying queued cache writes".to_string()),
        ),
    };

    if failed > 0 {
        INVALIDATIONS_LOST
            .get()
            .fetch_add(failed as u64, Ordering::Relaxed);
        // WARNING, never ERROR: see this function's doc comment. Built through
        // the builder so the advice lands in errhint rather than errdetail.
        pgrx::pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_WARNING,
            format!(
                "pg_resp lost {failed} of {total} cache invalidation(s) at commit: {}",
                first_error.unwrap_or_else(|| "unknown error".to_string())
            ),
            "pg_resp",
        )
        .set_hint(
            "the transaction committed successfully; only the cache update was lost, so \
             affected keys stay stale until their TTL expires. Count is exposed as \
             invalidations_lost in resp.stats() and INFO",
        )
        .report(PgLogLevel::WARNING);
    }
}

// ---------------------------------------------------------------------------
// The SQL functions
// ---------------------------------------------------------------------------

#[pg_schema]
mod resp {
    use super::*;

    /// Fetch a cached value. Returns NULL when the key is absent.
    ///
    /// Immediate and non-transactional: this reads cache state as it is right
    /// now, including values written by other sessions and *excluding* this
    /// transaction's own not-yet-committed `resp.set` calls.
    #[pg_extern]
    fn get(key: &str) -> Option<String> {
        let reply =
            with_client(|c| c.command(&[b"GET", key.as_bytes()])).unwrap_or_else(|e| read_error(e));
        bulk_to_text(reply, key)
    }

    /// Whether a key is currently present (and unexpired).
    #[pg_extern]
    fn exists(key: &str) -> bool {
        match with_client(|c| c.command(&[b"EXISTS", key.as_bytes()]))
            .unwrap_or_else(|e| read_error(e))
        {
            Reply::Integer(n) => n > 0,
            ref other => unexpected_reply("EXISTS", other),
        }
    }

    /// Remaining lifetime in seconds. `-1` = no expiry, `-2` = no such key —
    /// the RESP contract, preserved verbatim rather than translated to NULL,
    /// so the SQL surface answers the same question the wire does.
    #[pg_extern]
    fn ttl(key: &str) -> i64 {
        match with_client(|c| c.command(&[b"TTL", key.as_bytes()]))
            .unwrap_or_else(|e| read_error(e))
        {
            Reply::Integer(n) => n,
            ref other => unexpected_reply("TTL", other),
        }
    }

    /// Keys matching a glob pattern.
    ///
    /// Backed by `KEYS`, which walks the entire keyspace — same O(N) caveat
    /// the RESP command carries. Intended for introspection from psql, not for
    /// application code or triggers.
    #[pg_extern]
    fn keys(pattern: &str) -> SetOfIterator<'static, String> {
        let reply = with_client(|c| c.command(&[b"KEYS", pattern.as_bytes()]))
            .unwrap_or_else(|e| read_error(e));
        let items = match reply {
            Reply::Array(Some(items)) => items,
            Reply::Array(None) => Vec::new(),
            ref other => unexpected_reply("KEYS", other),
        };
        let keys: Vec<String> = items
            .into_iter()
            .map(|item| match item {
                Reply::Bulk(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
                _ => String::new(),
            })
            .collect();
        SetOfIterator::new(keys)
    }

    /// Queue a cache write, applied **after this transaction commits**.
    ///
    /// Rolling back means the write never happens. Note the corollary: within
    /// the same transaction, a subsequent `resp.get` on this key still returns
    /// the *old* value, because nothing has been sent yet.
    #[pg_extern]
    fn set(key: &str, value: &str, ttl_seconds: default!(Option<i64>, "NULL")) {
        if let Some(ttl) = ttl_seconds {
            if ttl <= 0 {
                raise(
                    PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                    format!("invalid expire time in resp.set(): {ttl}"),
                    "ttl_seconds must be positive, or NULL for no expiry",
                );
            }
        }
        enqueue(QueuedOp::Set {
            key: key.to_string(),
            value: value.to_string(),
            ttl_seconds,
        });
    }

    /// Queue a cache delete, applied **after this transaction commits**.
    #[pg_extern]
    fn del(key: &str) {
        enqueue(QueuedOp::Del {
            key: key.to_string(),
        });
    }

    /// Cache statistics, as one row.
    ///
    /// This is a **projection of the RESP `INFO` command**, not a second set of
    /// counters. That is deliberate: bible §5's Phase 3 "stats consistency"
    /// gate requires these numbers to be identical to `INFO`'s, and the way to
    /// guarantee that is to have exactly one source of truth rather than two
    /// implementations that agree today. The gate's test still compares both
    /// views — what it now protects against is a parsing mistake here, not
    /// counter drift, and `docs/semantics.md` says so plainly.
    #[pg_extern]
    fn stats() -> TableIterator<
        'static,
        (
            name!(keys, i64),
            name!(used_bytes, i64),
            name!(max_memory_bytes, i64),
            name!(keyspace_hits, i64),
            name!(keyspace_misses, i64),
            name!(evicted_keys, i64),
            name!(invalidations_lost, i64),
        ),
    > {
        let reply = with_client(|c| c.command(&[b"INFO"])).unwrap_or_else(|e| read_error(e));
        let text = match reply {
            Reply::Bulk(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            ref other => unexpected_reply("INFO", other),
        };
        let field = |name: &str| -> i64 { parse_info_field(&text, name).unwrap_or(0) };
        TableIterator::once((
            field("db0:keys"),
            field("used_memory"),
            field("maxmemory"),
            field("keyspace_hits"),
            field("keyspace_misses"),
            field("evicted_keys"),
            field("invalidations_lost"),
        ))
    }

    /// Trigger helper: evict one cache key derived from the row.
    ///
    /// ```sql
    /// CREATE TRIGGER products_cache_evict
    /// AFTER UPDATE OR DELETE ON products
    /// FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
    /// ```
    ///
    /// Argument 1 is a key prefix, argument 2 the column whose value completes
    /// the key. The eviction is queued on the commit queue like any other
    /// write, so it inherits the rollback guarantee for free.
    ///
    /// On `UPDATE`, if the key column itself changed, **both** the old and new
    /// keys are evicted — otherwise renaming a row's identity would strand the
    /// entry cached under its previous key.
    #[pg_trigger]
    fn evict<'a>(
        trigger: &'a pgrx::PgTrigger<'a>,
    ) -> Result<Option<PgHeapTuple<'a, impl WhoAllocated>>, PgHeapTupleError> {
        // Every failure below raises a real Postgres error via `raise()`
        // rather than returning `Err`. That is not a style preference: pgrx's
        // generated trigger wrapper finishes with
        // `trigger_fn_result.expect("Trigger function panic")`, and `expect`
        // formats with `Debug` — so a returned error surfaces to the user as
        // `ERROR: Trigger function panic: NotRowLevel`, with no errhint and no
        // way to say anything useful. Raising directly gives a properly styled
        // message plus a hint aimed at whoever wrote the CREATE TRIGGER, which
        // is who can actually fix it. (Verified empirically — the Debug-dump
        // form is what this code did before the fix.)
        let args = trigger.extra_args().unwrap_or_else(|_| {
            raise(
                PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                "resp.evict() could not read its trigger arguments".to_string(),
                "resp.evict() takes exactly two arguments, a key prefix and a column \
                 name, e.g. EXECUTE FUNCTION resp.evict('product:', 'id')",
            )
        });
        if args.len() != 2 {
            raise(
                PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                format!(
                    "resp.evict() requires exactly 2 trigger arguments, got {}",
                    args.len()
                ),
                "pass a key prefix and a column name, e.g. \
                 EXECUTE FUNCTION resp.evict('product:', 'id')",
            );
        }
        let prefix = &args[0];
        let column = &args[1];

        if matches!(trigger.level(), pgrx::PgTriggerLevel::Statement) {
            raise(
                PgSqlErrorCode::ERRCODE_E_R_I_E_TRIGGER_PROTOCOL_VIOLATED,
                "resp.evict() must be used in a FOR EACH ROW trigger".to_string(),
                "a statement-level trigger (including TRUNCATE) has no row to derive a \
                 cache key from; use resp.del() for whole-table invalidation",
            );
        }

        let old = trigger.old();
        let new = trigger.new();

        // Collect every key this row change could have invalidated. At most two,
        // so a linear dedup beats reaching for a HashSet.
        let mut keys: Vec<String> = Vec::new();
        for tuple in [old.as_ref(), new.as_ref()].into_iter().flatten() {
            // A NULL key column cannot identify a cache entry. Skipping is the
            // only sane reading — erroring would break legitimate rows.
            if let Some(value) = render_column_as_text(tuple, column) {
                let key = format!("{prefix}{value}");
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }

        for key in keys {
            enqueue(QueuedOp::Del { key });
        }

        // AFTER-trigger return values are ignored by Postgres, but a BEFORE
        // trigger must return the row to keep the operation alive. NEW (falling
        // back to OLD, for DELETE) is correct in both cases.
        Ok(new.or(old))
    }
}

/// Render one column of a tuple as text, for use as a cache key.
///
/// The key column is commonly `integer`, `bigint`, `uuid` or `text`, so all of
/// those go through pgrx's safe, typed accessors — no FFI, no memory-context
/// subtleties in a per-row trigger path.
///
/// Deliberately *not* supported: everything else. The tempting generalization
/// is to call Postgres's type output function (what `::text` does) for
/// arbitrary types, but `PgHeapTuple` does not expose the raw datum plus
/// tuple descriptor that needs, and reconstructing them by hand in a trigger
/// hot path buys a small convenience with a real risk of getting memory
/// contexts wrong. An unsupported column type gets a clear error naming the
/// type and the supported set instead, which the user can resolve with a
/// generated column or a cast.
fn render_column_as_text(
    tuple: &PgHeapTuple<'_, impl WhoAllocated>,
    column: &str,
) -> Option<String> {
    let Some((attno, attribute)) = tuple.get_attribute_by_name(column) else {
        raise(
            PgSqlErrorCode::ERRCODE_UNDEFINED_COLUMN,
            format!("resp.evict() was given column \"{column}\", which this table does not have"),
            "the second trigger argument must name a column of the table the trigger is on",
        );
    };
    let type_oid = attribute.type_oid().value();

    match type_oid {
        pg_sys::TEXTOID | pg_sys::VARCHAROID | pg_sys::BPCHAROID | pg_sys::NAMEOID => {
            tuple.get_by_index::<String>(attno).unwrap_or(None)
        }
        pg_sys::INT2OID => tuple
            .get_by_index::<i16>(attno)
            .unwrap_or(None)
            .map(|v| v.to_string()),
        pg_sys::INT4OID => tuple
            .get_by_index::<i32>(attno)
            .unwrap_or(None)
            .map(|v| v.to_string()),
        pg_sys::INT8OID => tuple
            .get_by_index::<i64>(attno)
            .unwrap_or(None)
            .map(|v| v.to_string()),
        pg_sys::UUIDOID => tuple
            .get_by_index::<pgrx::Uuid>(attno)
            .unwrap_or(None)
            .map(|v| v.to_string()),
        other => raise(
            PgSqlErrorCode::ERRCODE_DATATYPE_MISMATCH,
            format!(
                "resp.evict() cannot build a cache key from column \"{column}\" (type OID {})",
                other.to_u32()
            ),
            "supported key column types are text, varchar, char, name, smallint, integer, \
             bigint and uuid; for anything else add a generated column holding the key \
             text and point resp.evict() at that",
        ),
    }
}

/// Pull an integer field out of an `INFO` payload.
///
/// Handles both `field:value` (most of INFO) and `db0:keys=N` (the keyspace
/// section's own shape), which is why this is a hand-rolled scan rather than a
/// split on `:`.
fn parse_info_field(info: &str, field: &str) -> Option<i64> {
    for line in info.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix(field) {
            let rest = rest.trim_start_matches([':', '=']);
            if let Ok(value) = rest.trim().parse::<i64>() {
                return Some(value);
            }
        }
    }
    None
}

// Privileges (D12).
//
// The cache is a single, cluster-wide namespace with no per-role isolation
// (bible §3.6), so `resp.get` on a key any other role wrote is readable by
// anyone who can execute it. Default-open would therefore be the wrong
// posture for an extension asking a Postgres reviewer to trust it: revoke
// from PUBLIC and make granting an explicit, documented act.
//
// The exact grants a non-superuser needs — including USAGE ON SCHEMA resp,
// which is easy to forget and produces a confusing error when missing — are
// pinned by a test rather than assumed, and written up in docs/ops.md.
extension_sql!(
    r#"
REVOKE ALL ON SCHEMA resp FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.get(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.exists(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.ttl(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.keys(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.set(text, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.del(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.stats() FROM PUBLIC;
REVOKE ALL ON FUNCTION resp.evict() FROM PUBLIC;
"#,
    name = "resp_revoke_public",
    finalize
);

#[cfg(test)]
mod tests;
