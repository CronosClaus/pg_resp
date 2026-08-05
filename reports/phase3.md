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
| demo app 2 | works end-to-end (§11) | _pending_ |
| stats consistency | `resp.stats()` returns numbers identical to `INFO`'s stats/memory section | **PASS** — 9/9 field-by-field comparisons, with a guard asserting the counters are non-zero so the gate is not comparing two zeroes |

### Regression gates carried forward

| gate | result |
|---|---|
| R1 fast loop | **PASS** — 119/119 (45 resp-proto + 52 resp-store + 22 resp-client), up from 80. Plus 73/73 `pg_resp` lib tests |
| R2 compat matrix | _pending_ |
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

**5. The staleness harness's first sentinel silently corrupted timing.** It used
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
