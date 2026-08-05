# pg_resp — operations notes

**Status: living document, started in Phase 2, extended in Phase 3.**
Structured like a contrib module page (overview → configuration → functions →
caveats) per bible §8; sections grow as each phase adds surface. Not yet the
full Phase 4 docs pass. Companion pages:
[`semantics.md`](semantics.md) catalogues every divergence from Redis
behaviour, and [`invalidation.md`](invalidation.md) covers the trigger-based
invalidation design.

## Overview

pg_resp runs a RESP2-speaking TCP server inside a Postgres background
worker. It is ephemeral by design (bible D5): a cache, not a database.
Restarting the worker (or Postgres) empties it. There is no persistence.

## Configuration (GUCs)

| GUC | default | context | notes |
|---|---|---|---|
| `pg_resp.bind_address` | `127.0.0.1` | `postmaster` | **Widening this beyond localhost exposes the cache over the network — see the security caveat below before doing so.** |
| `pg_resp.port` | `6379` | `postmaster` | Redis's own default, since pg_resp is meant as a drop-in replacement |
| `pg_resp.max_memory` | `256MB` | `postmaster` | approximate byte budget: key + value bytes plus a measured 96-byte per-entry constant. `0` means unbounded |
| `pg_resp.eviction` | `clock_lru` | `postmaster` | `clock_lru` or `noeviction`. **`noeviction` disables the byte budget entirely in v0.1** rather than rejecting writes when full |
| `pg_resp.password` | empty | `postmaster` | empty means no authentication. Compared in constant time. Single password, no ACL users |

**Every GUC requires a restart**, and that is not laziness — it is honest.
pgrx's `GucSetting::get()` may only be called from the worker's registered main
thread, so the event-loop thread that actually uses these values can never
re-read them; they are read once at worker startup and passed down. Declaring
any of them `SUSET` would imply that `SET` takes effect without a restart, which
would be a lie. (Discovered by testing in Phase 2; recorded in the
`pgrx-patterns` skill.)

This memory is **outside** `shared_buffers` and outside Postgres's own
accounting. When capacity planning, `pg_resp.max_memory` competes for the same
RAM as `shared_buffers`; size them together.

## Caveats

### Security: default bind is localhost-only, on purpose

`pg_resp.bind_address` defaults to `127.0.0.1`. Widening it exposes a RESP2
port with **no TLS** in v0.1 (bible D6) — the same posture as a
default-configured Redis. Do not widen it on a host reachable from an untrusted
network without also setting `pg_resp.password`, and without understanding that
cache data is a single cluster-wide namespace: there is no per-role or
per-database isolation inside the cache, so any role granted the `resp.*`
functions can read anything any other role cached. See
[granting access](#granting-access-to-the-resp-functions).

Installing the extension requires superuser and `shared_preload_libraries`,
which is standard for a background-worker extension.

### Blast radius: a crash in the RESP path is not one uniform failure mode

This is the single most important operational fact about pg_resp's
architecture so far (bible §13 risk register), and it has two genuinely
different shapes depending on *how* the worker fails. Confirmed by
deliberately triggering both, not just reasoned about:

**1. The worker process itself is killed** (`kill -9`, an OOM kill, a
genuine segfault in unsafe code) — **loud, self-healing.** pgrx mandates
`BGWORKER_SHMEM_ACCESS` on every registered bgworker (no way to opt out in
the current pgrx version), so Postgres treats this exactly like any other
crashed backend: it logs `"all server processes terminated;
reinitializing"`, drops **every** SQL connection on the instance (not just
RESP clients), runs a full WAL crash-recovery cycle, then restarts
everything — including pg_resp — as part of that reinitialization. The
postmaster process itself never dies and this recovers with zero operator
intervention, but it is disruptive to every other application using the
same Postgres instance for the duration.

**2. A Rust `panic!()` happens somewhere in the RESP command-dispatch
path** (a bug in a command handler, an unexpected edge case) — **quiet,
permanent, and invisible from SQL.** `std::thread::spawn` catches the
unwinding panic at the OS-thread boundary, so the *process* does not crash
— but pg_resp's single-threaded event loop (bible D4) means one thread
owns the TCP listener and every open connection. Losing that thread means:
every existing RESP connection resets, no new RESP connection is ever
accepted again, and this does **not** self-heal — yet `SELECT 1` against
the same Postgres instance keeps working, and the pg_resp bgworker's own
OS process doesn't even exit. A health check that only pings Postgres
itself will report everything green while the cache has been completely
and silently dead since the panic.

**Mitigation in place, and it changed in Phase 3.** There are now two fences
with deliberately *different* policies, because the two failures deserve
different responses:

- **Per-connection fence.** Every command dispatch runs inside
  `catch_unwind`, so a bug handling one command drops only that one
  connection. The listener and every other connection carry on undisturbed.
  This is the common case and containment is the right answer.
- **Top-level policy: exit, and let Postgres restart us.** If a panic somehow
  escapes the per-connection fence, or the event-loop thread ends for any
  other reason, the worker now **logs at FATAL and exits**, and the postmaster
  restarts it after `bgw_restart_time` (1s) with a fresh, working listener.
  Earlier versions caught that panic and kept running, which was the worst
  available outcome: the process stayed healthy, the worker was still listed
  as running, `SELECT 1` still worked, and the cache was permanently dead with
  nothing anywhere reporting a problem. Exiting is louder *and* self-healing.

**Watchdog.** The worker's main thread probes its own listener over TCP every
3 seconds and requires three consecutive failures before acting. It is a real
RESP round trip, not a "is the thread alive" check, so it detects a thread that
is alive but wedged — the state that a process-liveness check reports as
healthy. (An error reply counts as alive: with `pg_resp.password` set, an
unauthenticated probe is answered `-NOAUTH`, and treating that as death would
turn a security setting into a crash loop.)

**How disruptive is the restart?** Measured, not assumed: the worker's FATAL
exit uses status 1, which Postgres treats as "child exited on a FATAL error"
and restarts on its own — it does **not** trigger the cluster-wide crash
recovery that failure mode 1 causes. Verified by holding a SQL session open
across a deliberately triggered restart: the session survived, and unrelated
SQL clients were unaffected. End-to-end recovery measured at 2.5 seconds. The
harness is `tests/lifecycle/panic_policy.py`, which exercises both fences by
deliberate failure and re-derives this answer every time it runs.

**What operators should still do.** RESP clients will see their connections
reset during the restart and must reconnect — most client libraries do this
automatically, but a client with no retry logic will surface an error. And
although the watchdog now covers the silent-death case, monitoring pg_resp
should still include a RESP-level check (a periodic `PING` against the cache
port) rather than only "is Postgres accepting connections": Postgres being
healthy remains necessary but not sufficient evidence that the cache is serving.

Full technical detail (exact reproduction, log output, the pgrx API
constraint behind failure mode 1): `.claude/skills/pgrx-patterns/SKILL.md`
§8.7-§8.8, `reports/phase2.md`, and `reports/phase3.md`.

## The SQL surface

Two access paths with different, deliberate semantics. Reads are immediate;
writes are queued and applied after the transaction commits. Every divergence
is catalogued in [`semantics.md`](semantics.md); the design rationale and the
trigger recipe are in [`invalidation.md`](invalidation.md).

| function | returns | when it takes effect |
|---|---|---|
| `resp.get(key text)` | `text`, NULL if absent | immediately |
| `resp.exists(key text)` | `boolean` | immediately |
| `resp.ttl(key text)` | `bigint` — seconds, `-1` no expiry, `-2` no such key | immediately |
| `resp.keys(pattern text)` | `setof text` | immediately |
| `resp.stats()` | one row: `keys`, `used_bytes`, `max_memory_bytes`, `keyspace_hits`, `keyspace_misses`, `evicted_keys`, `invalidations_lost` | immediately |
| `resp.set(key text, value text, ttl_seconds bigint DEFAULT NULL)` | `void` | **after commit** |
| `resp.del(key text)` | `void` | **after commit** |
| `resp.evict(prefix, column)` | trigger function | **after commit** |

`resp.keys()` walks the entire keyspace, same O(N) cost as the `KEYS` command.
It is for looking around in psql, not for application code.

### Granting access to the resp functions

Everything in schema `resp` is revoked from `PUBLIC` at install time. That is
deliberate: cache contents are a single cluster-wide namespace with no per-role
isolation, so anyone who can call `resp.get` can read anything any other role
cached. Granting is an explicit act.

The recipe below is what a non-superuser application role actually needs. It
was determined by testing rather than assumption (`tests/sql_surface/gates.py`
re-verifies it), because the failure mode when it is wrong is a confusing
error rather than an obvious one:

```sql
-- The half that is easy to forget. Without it, every call fails with
-- "permission denied for schema resp", which does not point at this line.
GRANT USAGE ON SCHEMA resp TO myapp;

-- Read-only access to the cache.
GRANT EXECUTE ON FUNCTION resp.get(text), resp.exists(text),
                          resp.ttl(text), resp.stats() TO myapp;

-- Add these if the application also writes to the cache directly.
GRANT EXECUTE ON FUNCTION resp.set(text, text, bigint),
                          resp.del(text) TO myapp;

-- Needed only to CREATE a trigger using resp.evict() as this role.
GRANT EXECUTE ON FUNCTION resp.evict() TO myapp;
```

Two tested facts about the trigger helper's privileges:

- `CREATE TRIGGER ... EXECUTE FUNCTION resp.evict(...)` requires both
  `USAGE ON SCHEMA resp` and `EXECUTE ON FUNCTION resp.evict()` **at creation
  time**.
- That `EXECUTE` privilege is **not re-checked when the trigger fires**.
  Revoking it later does not disarm triggers that already exist — the trigger
  keeps evicting. To disable it, drop the trigger.

### Lost invalidations

If the RESP server is unreachable when a transaction's post-commit callback
runs, that invalidation is **lost**: a `WARNING` is emitted and
`invalidations_lost` is incremented (visible in both `resp.stats()` and
`INFO`). It is not retried, and it cannot be — the callback runs after the
commit is irrevocable, and raising an error there would abort the backend.

Operationally: alert on `invalidations_lost` increasing, and set a TTL on
cached entries so that every entry is self-healing on a bounded horizon rather
than depending on an invalidation that might not arrive.
