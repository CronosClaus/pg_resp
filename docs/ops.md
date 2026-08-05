# pg_resp — operations notes

**Status: living document, started in Phase 2.** Structured like a contrib
module page (overview → configuration → caveats) per bible §8; sections
grow as each phase adds surface. This is not yet the full Phase 4 docs
pass — it exists now specifically to carry the blast-radius caveat below,
which the project bible's own risk register (§13) treats as this project's
most important architectural fact to date, not something to leave
undocumented until later.

## Overview

pg_resp runs a RESP2-speaking TCP server inside a Postgres background
worker. It is ephemeral by design (bible D5): a cache, not a database.
Restarting the worker (or Postgres) empties it. There is no persistence.

## Configuration (GUCs)

| GUC | default | context | notes |
|---|---|---|---|
| `pg_resp.bind_address` | `127.0.0.1` | `postmaster` | requires a restart to take effect. **Widening this beyond localhost exposes the cache over the network — see the security caveat below before doing so.** |
| `pg_resp.port` | `6379` | `postmaster` | requires a restart to take effect; defaults to Redis's own default since pg_resp is meant as a drop-in replacement |

`pg_resp.max_memory`, `pg_resp.eviction`, and `pg_resp.password` are not
registered yet — they land alongside the features they configure
(eviction, AUTH) later in Phase 2/3. This table will grow.

## Caveats

### Security: default bind is localhost-only, on purpose

`pg_resp.bind_address` defaults to `127.0.0.1`. Widening it exposes a
RESP2 port with no TLS in v0.1 (bible D6) — the same posture as a
default-configured Redis. Do not widen it on a host reachable from an
untrusted network without also setting `pg_resp.password` (once that GUC
exists) and understanding that all cache data is cluster-visible to any
role granted the SQL functions (Phase 3).

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

**Mitigation in place**: every command dispatch is wrapped in
`std::panic::catch_unwind` at the per-connection level, so a bug in
handling *one* command drops only that one connection — the listener and
every other connection keep running undisturbed. A second, outer fence
around the whole event-loop thread exists as defense in depth and logs
loudly if a panic somehow escapes the per-connection fence (not expected in
practice). This reduces failure mode 2 from "the entire cache silently
dies forever" to "one client's connection resets" — but it does not
eliminate the underlying category: **a monitoring setup for pg_resp should
include an actual RESP-level health check (e.g. periodic `PING` against
the cache port), not just "is Postgres accepting connections."** Postgres
being healthy is necessary but not sufficient evidence that pg_resp is
serving traffic.

Full technical detail (exact reproduction, log output, the pgrx API
constraint behind failure mode 1): `.claude/skills/pgrx-patterns/SKILL.md`
§8.7-§8.8 and `reports/phase2.md`.
