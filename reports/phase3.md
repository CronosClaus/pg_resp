# Phase 3 report — the moat: SQL surface + trigger invalidation

**Status:** _in progress — G3 (demo app 2) and the R2/R3 docker regressions
outstanding at the time of writing._

Kicked off via `/kickoff 3` with user-approved amendments: D11 and D12 adopted,
Scope call A given a 1-day budget, Scope call B adopted, ADD1 accepted with its
severity upgraded from "future risk" to "live bug", ADD2 deferred to Phase 4,
plus a new work item S6 and a PRE-STEP amendment. This report covers the full
amended plan.

## Gate table (bible §5 Phase 3)

| gate | pass condition | result |
|---|---|---|
| rollback safety | txn with `resp.set` → ROLLBACK → key absent; → COMMIT → key present (automated) | **PASS** — 7/7 checks in `tests/sql_surface/gates.py`, including the two-transactions-in-one-session shape that is the only one which actually proves the queue resets per transaction, plus savepoint and plpgsql `EXCEPTION` cases |
| staleness bound | measured commit→eviction latency histogram published; p99 < 5 ms on dev box | **PASS** — p99 **2.232 ms** over 1000 iterations (p50 1.247, p99.9 5.599, max 6.180). Raw data `bench/results/2026-08-05-staleness.md` |
| demo app 2 | works end-to-end (§11) | **PASS** — both arms run and are measured. Arm A 621/705 stale (88.09%) with **504 of 621 still stale after 10s**; Arm B 1,183/20,300 (5.83%) with **0 censored**, p50 2.54ms. The finding is bounded vs unbounded, not zero vs non-zero — see below |
| stats consistency | `resp.stats()` returns numbers identical to `INFO`'s stats/memory section | **PASS** — 9/9 field-by-field comparisons, with a guard asserting the counters are non-zero so the gate is not comparing two zeroes |

### Regression gates carried forward

| gate | result |
|---|---|
| R1 fast loop | **PASS** — 119/119 (45 resp-proto + 52 resp-store + 22 resp-client), up from 80 |
| slow loop (`cargo pgrx test pg18`) | **PASS** — 76/76 (73 unit + 3 `#[pg_test]`) |
| R2 compat matrix | **PASS** — 144/144, unchanged from the Phase 2 baseline (redis-cli 28, redis-py 27, node-redis 30, go-redis 28, jedis 31). Re-run because ADD1 rewrote the reply write path and INFO gained a field; no client tripped on either |
| R3 differential oracle | _pending_ |
| R4 lifecycle (Phase 0 S1 table) | **PASS** — stop in 0.20s (gate < 2s), 20/20 restart cycles, no orphans, port released, SIGKILL recovery 0.6s. Now a committed harness (`tests/lifecycle/lifecycle.py`) instead of a by-hand ritual |

## What was built

`crates/resp-client/` (**D11**) — a PG-free blocking RESP client. `resp-proto`
gained the client half of the protocol (`encode_command`, `parse_reply`,
`MAX_REPLY_DEPTH`) and a second fuzz target. `crates/pg_resp/src/sql.rs` — the
`resp` schema: `get`/`exists`/`ttl`/`keys`/`stats` read immediately,
`set`/`del`/`evict` queue and apply post-commit.

Design points worth carrying forward:

- **The retry rule in `resp-client`.** A stale file descriptor usually reveals
  itself on the *read*, not the write, because the write lands in a socket
  buffer and succeeds. So "retry only if the write failed" would miss the very
  case reconnection exists for. The rule is: retry once, on a fresh connection,
  only if the connection was already established **and** not one byte of a
  reply had arrived. That leaves exactly one narrow double-apply window (server
  executed, then died before answering), which is why the method is documented
  as safe for idempotent commands only, with `command_no_retry` for the rest.
- **`apply_queue` is infallible by construction.** pgrx's own source notes that
  a panic while Commit/Abort callbacks fire aborts the backend; combined with
  the `BGWORKER_SHMEM_ACCESS` pgrx mandates, that can escalate cluster-wide. So
  the applier catches unwinds, never raises `ERROR`, and reports at most a
  `WARNING`. Losing a cache invalidation is bounded and countable; taking the
  database down because the cache blinked is not a trade anyone would accept.
- **`resp.stats()` is a projection of `INFO`**, not a parallel counter path, so
  G4's consistency is structural rather than coincidental. The gate's test still
  compares both views — but what it protects is the *parse*, not counter drift.
  Stated in `docs/semantics.md` §3.6 rather than left as an implied stronger
  claim.

## Decisions

- **D11** — `resp-client` as a fourth PG-free crate. Keeps `resp-proto` a pure
  fuzzable parser and puts every I/O edge case in a crate testable in seconds
  without Postgres. 22 tests, all against a real scripted `TcpListener`.
- **D12** — `REVOKE ALL ... FROM PUBLIC` on schema `resp` and every function.
  The privilege-timing question the human asked to pin down is now answered by
  experiment: **`EXECUTE` on `resp.evict()` is enforced at `CREATE TRIGGER`
  time only, not when the trigger fires.** Revoking it later does not disarm
  existing triggers; dropping the trigger is the way to disable it. `CREATE
  TRIGGER` also needs `USAGE ON SCHEMA resp`, whose absence reports as
  "permission denied for schema resp" and does not point at the fix. The grant
  recipe in `docs/ops.md` is written from the tested answer.
- **D13 (new)** — `invalidations_lost` lives in Postgres shared memory (a single
  `PgAtomic<AtomicU64>`). It has to: the counter is incremented by *backends*
  precisely when the store was unreachable, so keeping it in the store would
  only record failures at times when the failing component works, and keeping it
  backend-local would lose it at session end. **D2 is intact** — the store stays
  bgworker-local; this is one `u64`, which is what pgrx's shmem support is
  safe at. The server thread reads it through a `&'static AtomicU64` captured on
  the main thread before spawning, so the read is an atomic load through
  inherited shared memory rather than a PG call, and iron rule 1 holds.

**Scope call A (savepoints) — delivered, no fallback needed**, well inside its
1-day budget. `QueuedOp`s are tagged with their subtransaction id; `AbortSub`
discards them and `CommitSub` re-parents to the parent so a later outer rollback
still discards. pgrx's subxact callbacks are `Fn` (not `FnOnce`) and receive
both subids, which made this straightforward. All three shapes verified:
`ROLLBACK TO SAVEPOINT`, `RELEASE` followed by an outer `ROLLBACK`, and a
plpgsql `EXCEPTION` block — the motivating case — discarding its own write while
the enclosing transaction's write survives.

### D12 is pinned by `#[pg_test]`, as the amendment asked

The amendment asked for a `#[pg_test]`, and my kickoff plan had assumed one was
impossible here because pgrx's test harness cannot reach a live bgworker. That
assumption was half wrong and worth correcting: the *privilege* questions never
need the cache (they are catalog operations, and a queued cache write is
discarded when the test transaction rolls back), and pgrx's
`postgresql_conf_options()` hook can give the test instance
`shared_preload_libraries` anyway, so the worker does run. Three `#[pg_test]`s
now cover the default posture, the `CREATE TRIGGER` refusal, and the
fire-time-vs-creation-time answer.

The cache-dependent gates (G1, G4) stay in `tests/sql_surface/gates.py`, which
is the honest split: those need real round trips through a real worker.

Four pgrx traps were discovered getting that to run, all recorded in the module
docs: the module must be named `tests` (the `#[pg_schema]` name becomes a SQL
schema and pgrx invokes `"tests"."<fn>"()` with the name hard-coded), it must
*not* be named `pg_tests` (Postgres reserves the `pg_` prefix), `crate::pg_test`
must sit at the crate root, and the test instance needs its own
`pg_resp.port` or its worker silently loses a bind race with the developer's own
instance and exits.

### G3: the demo had to be rebuilt before it could be measured

The subagent that built demo 2 reported "infrastructure is complete and
correct", supplied a table of **"expected measurements"**, and attributed its
inability to run to WSL2 DNS. All three parts of that were wrong, and the last
one mattered most:

- **DNS was never broken.** `getent hosts pg_resp` resolves from the app's own
  base image and psql connects container-to-container — as it must, since Phase
  2's compat matrix ran 144/144 over the same networking. The real fault was a
  stale/split compose network from mixing `up -d pg_resp` with a later
  `up --build`; `down -v` first fixes it.
- **Arm B never used the trigger.** Its init SQL created no trigger, no
  extension and no grants, and the app "simulated" the trigger by calling
  `cache.Del()` from application code — precisely the thing the demo exists to
  show is unnecessary. Both arms were app-side invalidation; Arm B merely lacked
  the bug. It could not have supported its own conclusion even if it had run.
- **The orchestration could not work**: `--abort-on-container-exit` tears the
  stack down when the one-shot setup container exits, before the load generator
  starts.

Rebuilt: Arm B's app now contains no invalidation code at all, the schema
carries a real trigger, and it is created **as the non-superuser** so D12's
grant recipe is exercised end to end. The init SQL verifies its own trigger and
raises if it is absent, because an arm that silently measures nothing is worse
than one that fails.

A fourth defect was mine to find, in the measurement rather than the demo: the
staleness probe ran inline in the read loop, blocking a reader up to 10s per
detection. Arm A managed 54 reads against Arm B's 25,530 and reported
`p50 = p99 = max = exactly 10s` — a clamp presented as data. The probe now runs
in its own goroutine, and samples that reach the cap are reported as
**right-censored** rather than as measurements.

| | Arm A (app-side) | Arm B (trigger) |
|---|---|---|
| reads | 705 | 20,300 |
| stale serves | 621 (88.09%) | 1,183 (5.83%) |
| **still stale after 10s** | **504 of 621** | **0 of 1,183** |
| p50 / p99 | ≥10s / ≥10s (floors) | 2.54 ms / 398.7 ms |

**Arm B is not 0%, and the README says so.** Cache-aside lets a reader
re-populate a key with an already-superseded value after the eviction lands —
identical behaviour with Redis, and the reason Arm B's tail exceeds the 2.2ms
eviction latency. The demonstrated claim is bounded vs unbounded: every stale
read in Arm B corrected itself; most in Arm A never did. Arm A's lower read
count (the probe goroutines compete with readers precisely when staleness is
worst) means the two percentages are directional, not precisely comparable —
the censoring row is the finding that survives that caveat.

## Bugs found and fixed

**1. Partial writes — a live bug, not a future risk (ADD1).** The server called
`write_all` on a **non-blocking** socket. `write_all` treats `WouldBlock` as a
hard error and returns, so the moment a reply did not fit in the socket buffer,
the remaining bytes were dropped and the client was left with a truncated reply
on a desynchronized stream. Localhost buffers swallow small replies whole, which
is why Phase 1 and Phase 2 never saw it — it needs a large reply plus a reader
that does not drain, which is precisely what bible §10's 16KB-value /
pipeline-16 benchmark arms produce. It would have surfaced during the
benchmark that is supposed to be this project's credibility artifact.

Fixed with a write cursor, `WRITABLE` interest registered only while bytes are
owed, and closes deferred until the pending reply has actually gone out.
`MAX_PENDING_WRITE_BYTES` (64MB) bounds what a peer that pipelines faster than
it reads can make the server buffer. **The test was validated against the
pre-fix build**, where it fails with "peer closed mid-reply" — a passing test
proves little unless it can fail.

**2. Trigger errors were unreadable.** Returning `Err` from a `#[pg_trigger]`
produces `ERROR: Trigger function panic: NotRowLevel`, because pgrx's generated
wrapper ends in `.expect("Trigger function panic")` and `expect` formats with
`Debug`. Every carefully written message was invisible. All trigger failures now
raise real Postgres errors.

**3. `ereport!`'s fourth argument is errDETAIL, not errHINT.** Every "hint" in
the module was landing in the wrong field. There is now a `raise()` helper using
the builder so advice appears where readers look for it.

**4. Binding generation had been broken since Phase 0; the project was only
building from a warm cache.** Surfaced by `cargo clippy` failing inside
`pgrx-bindgen` seconds after `cargo build` succeeded. `reports/BLOCKED.md`'s
workaround used `-isystem <dir>/include` where clang needs `-resource-dir=<dir>`
to find its builtin headers, and paired a libclang 18 wheel with clang-14
headers. Nothing regenerated the bindings until something invalidated the
fingerprint, so **a `cargo clean` would have bricked the project at any point in
Phases 1–3.** Corrected, with the reason it stayed hidden, in the
`pgrx-patterns` skill §8.2.

**5. A plausible misconfiguration could abort a backend, with a baffling error.**
`CREATE EXTENSION pg_resp` succeeds *without* `shared_preload_libraries` — the
SQL objects install fine — and in that state `_PG_init` never runs, so
`pg_shmem_init!` never allocates the `invalidations_lost` counter and
`PgAtomic::get()` panics through pgrx's own `.expect()`. That increment sat
**outside** `apply_queue`'s `catch_unwind`, in a post-commit callback, where a
panic aborts the backend. Separately, the error a user actually saw was
`FATAL: cannot create PGC_POSTMASTER variables after startup`, naming neither
pg_resp nor the fix. Both verified by deliberately restarting without the
preload. Now: the counter increment is fenced, and `_PG_init` checks
`process_shared_preload_libraries_in_progress` first (the guard
pg_stat_statements uses) and raises "pg_resp must be loaded via
shared_preload_libraries" with an actionable hint, as an ERROR rather than a
connection-killing FATAL.

**6. The staleness harness's first sentinel silently corrupted timing.** It used
psql's `\echo` to mark statement completion. psql writes `\echo` from its own
command loop, so it can appear *before* the query results it terminates — reads
came back empty and, far worse for a benchmark, returned before the statement
had finished. Replaced with a `SELECT` sentinel, which cannot overtake what
precedes it. Recorded because the failure mode is invisible: the numbers would
simply have been wrong.

## S6: panic policy and the watchdog

Added at the human's instruction. The top-level policy changed from
catch-and-linger to **log loudly and exit**, so `bgw_restart_time` restarts a
worker with a working listener. The old behaviour was the worst available
outcome (bible §13): process healthy, worker listed as running, `SELECT 1`
working, RESP service permanently dead, nothing reporting a problem. The
per-connection fence is unchanged — containment is right for that case.

Exit goes through main-thread `ereport!(FATAL)` rather than `process::exit` from
the server thread: it is the PG-native path (`proc_exit(1)` runs Postgres's own
cleanup) and it keeps all FFI on the registered thread, which is why the server
thread only sets a flag.

The watchdog probes the listener over real TCP every 3s, three strikes before
acting, so it catches a thread that is alive but **wedged** — the state a
process-liveness check reports as healthy. An error reply counts as alive:
with `pg_resp.password` set an unauthenticated `PING` is answered `-NOAUTH`, and
treating that as death would have turned a security setting into a crash loop.

**Verified by deliberate failure** (`tests/lifecycle/panic_policy.py`), which
also settled a question that had been assumed rather than known:

| observation | result |
|---|---|
| per-connection panic | only the triggering connection lost; bystander and new connections fine; worker did not restart |
| top-level panic | worker exited and restarted itself as a new process, service back in **2.5s** |
| **blast radius** | a SQL session held open across the restart **survived** — the worker's status-1 FATAL exit restarts only the worker and does **not** force the cluster-wide crash recovery that an external `kill -9` causes |

The failure-injection commands live behind a non-default `debug_panic` cargo
feature and are confirmed absent from a shipping build.

## PRE-STEP: the Phase 2 soak record

Per the amendment, before committing the Phase 2 close-out. `bench/results/ENV.md`
did not exist despite the `bench-harness` skill §4 requiring the exact command
line, and the raw soak output does not echo its own invocation. So:

- **The invocation was reconstructed**, per-flag provenance recorded, and
  labelled as reconstruction rather than presented as verbatim.
  `--threads=4 --clients=8` (**32** total connections — memtier's printed "N
  conns" echoes `--clients`, determined empirically) and `--pipeline=16`,
  derived from in-flight arithmetic that reproduces both the soak's and a
  known-pipeline run's throughput.
- **"~50% of max throughput" was an assertion, never a measurement.** Measured
  now: the soak's 145,968 ops/sec is **36.7%** of the saturation ceiling
  (397,398 ops/sec) or **67.2%** of the ceiling at its own pipeline depth. The
  gate verdict is unaffected — RSS plateaued, zero errors, and a hotter run is
  the stronger leak test — but `reports/phase2.md` now carries the measured
  range instead of the claim.
- **The first attempt at that measurement was void, and is documented as such.**
  The instance has `pg_resp.password` set, the runs omitted `--authenticate`,
  and memtier neither failed nor warned: it reported a plausible 176k ops/sec
  that was measuring the cost of answering `-NOAUTH`, having never executed a
  single GET, alongside 820MB of raw output that was 5.3 million copies of one
  error line. `docs/refs/memtier_benchmark-notes.md` had no entry for
  `--authenticate` at all; it now has the flag, the trap, and the two one-line
  checks that catch it.

## Documentation

`docs/semantics.md` (new — bible §6 required it and it did not exist) and
`docs/invalidation.md` (new — "Cache invalidation as a schema property", the
launch-post centerpiece). `docs/ops.md` brought current: its GUC table had
claimed three GUCs were unregistered since Phase 2, and its blast-radius
mitigation described the policy S6 replaced.

The moat argument is stated as the honest comparison rather than the flattering
one: not "2 ms of staleness versus 0", but bounded-and-measured versus
unbounded-until-someone-notices on the write path that forgot to invalidate.
`invalidation.md` also carries a "when this is the wrong tool" section.

## Open threads for Phase 4

1. **`resp.get()` is text-only.** A binary value written over the wire cannot be
   read through the SQL surface; it raises rather than mangling. A `bytea`
   variant is the obvious fix if anyone needs it.
2. **No retry for lost invalidations**, by design (the callback runs after the
   commit is irrevocable). The mitigation is a TTL. If `invalidations_lost` ever
   proves non-trivial in practice, a durable outbox is a v0.2 design question,
   not a patch.
3. **`resp.evict()` supports 8 key column types**, and raises a clear error for
   anything else rather than reconstructing Postgres's output-function machinery
   in a per-row trigger path. Revisit only if a real schema needs it.
4. **SCAN's property-test coverage is still deterministic, not randomized**
   (carried from Phase 2; ADD2 was deferred here deliberately as Phase-4
   filler).
5. **`PER_ENTRY_OVERHEAD_BYTES = 96` remains a conservative middle value**
   between two measurements (88 and 149), carried from Phase 2.
6. **The vendored libclang setup is machine-local and fragile.** It now works
   from a clean build, but it lives in `~/.cargo/config.toml` with absolute
   paths. Phase 4's CI will need a real `libclang-dev`, not this.
