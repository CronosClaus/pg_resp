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
RAM as `shared_buffers`; size them together — worked below.

## Capacity planning: the 8 GB box, worked

Adding pg_resp to a database server creates a **third tenant** competing for the
same RAM, and unlike the other two it is invisible to every Postgres memory
view. `pg_stat_*`, `SHOW shared_buffers`, and `EXPLAIN (ANALYZE, BUFFERS)` will
never mention it. If you size the database as though the cache were free, the
kernel will eventually make the decision for you.

The three tenants:

| tenant | what it holds | who accounts for it |
|---|---|---|
| `shared_buffers` | PostgreSQL's page cache of its own relations | PostgreSQL |
| **`pg_resp.max_memory`** | **the RESP cache, in the worker's private heap (D2)** | **nobody — this page is the accounting** |
| OS page cache | everything read from disk, including PG files not in `shared_buffers` | the kernel, opportunistically |

### A concrete split on 8 GB

Assume 8192 MB total, PostgreSQL 18, `max_connections = 100`, and a cache
holding roughly 500k entries of ~1 KB.

| allocation | MB | reasoning |
|---|---|---|
| OS, kernel, sshd, monitoring | 512 | leave it; the box is not only a database |
| `shared_buffers` | 2048 | the conventional 25% of RAM |
| worst-case backend work memory | 400 | `100 × work_mem(4MB)`, and see the warning below |
| **`pg_resp.max_memory`** | **1024** | the cache budget you configure |
| **pg_resp actual RSS** | **~1140** | **what the process really costs — see the multiplier below** |
| left for OS page cache | ~4100 | headroom, and the shock absorber for everything above |

The line that matters is the difference between the fourth and fifth rows.

### `max_memory` is a budget for *accounted* bytes, not a promise about RSS

`pg_resp.max_memory` bounds `key.len() + value.len() + 96` summed over entries.
Two things live outside that sum:

1. **The 96-byte constant is an accounting figure, and real overhead is about
   twice it. Size from the measured number, not from the constant.** The
   constant was measured twice on a live worker by `VmRSS` delta over
   *100k-entry* loads: ~71 bytes per entry on the first batch, ~134 on the
   second. They disagree because Rust's `HashMap` doubles its bucket array, so a
   load crossing a resize threshold pays for capacity it has not filled. 96 sits
   between them.

   **Corrected against a 1M-entry measurement** (`bench/results/ENV.md` §26,
   1M x 1 KB entries on a dedicated box): real overhead is **~186 bytes per
   entry** above the value size, of which ~10 bytes is the key, leaving **~176
   bytes beyond key + value against the 96 accounted**. So the unaccounted excess
   is **~80 bytes per entry — roughly 80 MB at 1M entries**, about double the
   ~40 MB this section previously documented. The earlier figure was not wrong
   for the scale it was taken at; it was measured at 100k entries and does not
   extrapolate, because `HashMap` capacity, allocator arenas and fragmentation
   all grow with the table.

   **For sizing, plan on ~190 bytes of real overhead per entry, not 96.** An
   operator who sized from the accounted constant would under-provision by ~90 MB
   per million entries. Erring high here costs headroom; erring low costs an OOM.
2. **Allocator and process overhead.** The worker is a process: stacks, the
   `mio` poll state, read and write buffers (bounded by
   `MAX_PENDING_WRITE_BYTES`, 64 MB per connection with bytes owed), and the
   allocator's own arenas and fragmentation.

   **The allocator is glibc malloc, not jemalloc.** An earlier revision of this
   sentence said jemalloc; pg_resp has no `jemallocator` dependency and uses
   Rust's default system allocator. Redis and Valkey use jemalloc 5.3.0, so the
   mistake was in the flattering direction — jemalloc is generally stronger at
   this allocation pattern. Corrected here as it was in
   `bench/configs/README.md`. Moving pg_resp to jemalloc is a v0.2 item,
   deliberately not done mid-benchmark-phase.

Measured on the Phase 2 soak — 30 minutes, 1 KB values, `max_memory = 256MB`,
store warm and at its budget — the worker's `VmRSS` plateaued at **291,908 kB
(285 MiB)** against a 256 MiB budget:

```
RSS ÷ max_memory  =  285 MiB / 256 MiB  =  ~1.11
```

**Plan on RSS ≈ 1.15 × `pg_resp.max_memory`**, and treat that as a floor rather
than a guarantee. The 1M-entry measurement is consistent with this: 1,210,765,312
bytes of RSS against ~1,130,000,000 accounted (1M x [1024 value + ~10 key + 96])
is a ratio of **~1.07** — inside the 1.15 planning figure, so that figure stands.
It is the *per-entry overhead constant* above that was understated, not this
ratio.

The ratio remains one measurement on one workload, and it is
workload-dependent in a predictable direction: with 1 KB values the 96-byte
constant is ~9% of an entry, but with 64-byte values it is larger than the value
itself, so a cache of small entries carries proportionally far more overhead per
accounted byte. Raw data: `bench/results/soak-rss.log`,
`bench/results/ENV.md` §5.

### The OOM arithmetic rule

Apply this to your own box before enabling the extension:

```
  shared_buffers
+ (max_connections × work_mem)          # worst case; see the caveat
+ maintenance_work_mem × autovacuum_max_workers
+ (pg_resp.max_memory × 1.15)           # the measured multiplier above
+ 512 MB                                # OS and everything that is not PG
─────────────────────────────────────
= committed RAM;  keep this ≤ 75% of physical RAM
```

The remaining 25% is not slack. It is the OS page cache, which is what makes
the *misses* fast — and a cache exists to be missed sometimes.

Two ways this arithmetic understates reality, both worth knowing:

- **`work_mem` is per sort or hash node, not per connection.** A single complex
  query with several sorts can multiply its own allocation. `max_connections ×
  work_mem` is the conventional estimate, not a bound.
- **There is no `pg_resp` equivalent of `shared_buffers`' hard cap for the
  process as a whole.** `max_memory` caps the accounted cache contents; it does
  not cap the process. If you need a hard ceiling, set one outside the database
  — a cgroup memory limit on the postmaster's slice — and remember that a cgroup
  OOM kill of the worker is failure mode 1 below, i.e. cluster-wide crash
  recovery.

**If in doubt, start small.** `pg_resp.max_memory = 256MB` is the default for a
reason: an undersized cache costs hit rate, which is measurable and recoverable.
An oversized one costs an OOM kill, which is neither.

## Caveats

### Cold start: the cache is empty after every restart, and misses arrive all at once

This is the first caveat because it is the one most likely to surprise someone
in production, and it follows directly from ephemerality by design (bible D5).

pg_resp has no persistence. A worker restart, a PostgreSQL restart, a failover,
a `pg_ctl reload` that happens to cycle the worker, a panic-triggered restart
(measured at 2.5 s below) — every one of them leaves the cache **completely
empty**. The application does not know that. It carries on issuing the same read
volume it always did, and every single one of those reads is now a miss that
falls through to PostgreSQL simultaneously.

That is a **thundering herd on your database**, and it arrives precisely when the
database has just restarted or is already under stress. The steady-state
arithmetic makes the size of it obvious: a cache serving 10,000 reads/sec at a
95% hit rate is shielding PostgreSQL from 9,500 queries/sec. After a restart,
PostgreSQL receives all 10,000 — a 20× step change in load, from a component
whose entire job was to prevent exactly that.

Redis has the same exposure and mitigates it with persistence (RDB/AOF), which
pg_resp deliberately does not have (D5 — persistence is where scope and danger
live for a v0.1 cache). So the mitigations here are architectural rather than
configurable:

- **Assume it will happen and load-test for it.** Measure your application with
  the cache emptied under production read volume. If PostgreSQL cannot survive
  that, the cache is load-bearing infrastructure and needs to be treated as
  such, whichever cache you use.
- **Stagger re-population where the application allows it.** A jittered TTL on
  write (rather than a single fixed one) prevents a *second* herd later, when a
  whole generation of entries written during recovery expires together.
- **Prefer request coalescing in the application** for the hottest keys — one
  in-flight fill per key, others wait on it. This is worth more than any cache
  setting during a cold start.
- **A cold cache is fast to detect**: `resp.stats()` or `INFO` shows `keys` near
  zero with a miss rate near 100%. That is a better alert signal than
  process liveness, which stays green throughout (see the blast-radius section).

**When this makes pg_resp the wrong tool:** if your workload cannot tolerate an
empty cache and you have no way to shield the database, you want a cache with
persistence, and that is Redis or Valkey today. `docs/invalidation.md` has the
companion "when this is the wrong tool" discussion for the trigger path.

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
