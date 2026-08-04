# Phase 2 pre-work (Stage C of the 2026-08-04/05 overnight run)

**Not a phase close.** Phase 2 proper hasn't started (no `/kickoff 2` plan,
no gate table run against a live server, no soak test, no GUC wiring). This
is fast-loop-only pre-work in `resp-store`, done ahead of time per this
run's explicit instructions ("NO soak, NO memtier, NO new pgrx code").
`CLAUDE.md`'s phase line stays at **2** (set when Phase 1 closed) — this
file exists so the session that actually kicks off Phase 2 doesn't
re-derive or accidentally redo what's already here.

## What's done

In `crates/resp-store` (pure Rust, no PG deps, `cargo test -p resp-store`):

1. **Active expiry sweep** — `Store::active_expire_sweep(now, sample_size)`:
   samples up to `sample_size` keys, removes any expired as of `now`,
   returns the count removed. Bible §3.5: "active sweep from the event loop
   timer (N keys per tick, Redis-style)." **Not yet wired into pg_resp's
   event loop** — that wiring (calling this once per loop tick from the
   server thread) is real Phase 2 work, not pre-work; the mechanism exists
   and is tested in isolation.
2. **CLOCK-LRU eviction under a byte budget** — `Store::with_max_memory(n)`
   constructor; `used_bytes()`/`max_memory_bytes()` accessors; every
   insert/overwrite/remove keeps `used_bytes` accounted
   (`PER_ENTRY_OVERHEAD_BYTES = 64`, a documented **estimate**, not a
   measurement — bible §3.5 says the real constant needs to be "measured in
   Phase 2," which needs a live process and a memory profiler, i.e. real
   Phase 2, not pre-work). Eviction samples up to `DEFAULT_SAMPLE_SIZE = 20`
   keys per pass via `Entry.clock_bit` (set on every read/write access,
   cleared as a "second chance" during a sweep, evicted if still clear next
   time it's sampled) — approximates Redis's own sampled-LRU philosophy per
   bible §3.5, deliberately simpler. **Known simplification, documented in
   the code, not hidden**: sampling iterates via `HashMap`'s own (arbitrary
   but stable-between-calls) order rather than true randomness or a stable
   ring/cursor. Correct for the "never exceeds budget" property; sampling
   *quality* tuning against real access patterns is real Phase 2 work (needs
   a soak test to tune against).
3. **Three property tests** (bible §5 Phase 2 gate table, via `proptest`,
   `crates/resp-store/src/proptests.rs`):
   - `never_exceeds_budget_by_more_than_one_entry` — random SET sequences
     against a small fixed budget; asserts `used_bytes() <= budget + (the
     largest single entry seen so far)` after every write.
   - `expired_keys_never_returned` — random TTLs, all under 1 second;
     asserts every key returns `None` once queried past every generated
     deadline.
   - `incr_monotonic_under_churn` — a fixed target key is `INCR`'d by random
     deltas, interleaved with unrelated `SET`s on other ("churn") keys
     between every increment; asserts the target's value always equals the
     exact running sum of its own deltas, i.e. arithmetic on one key is
     never corrupted by unrelated map activity (inserts/removes reshaping
     the underlying `HashMap`) happening around it.
   All three green, plus 3 new concrete unit tests
   (`eviction_kicks_in_when_over_budget`,
   `eviction_never_evicts_the_key_just_written`,
   `active_expire_sweep_removes_expired_and_leaves_live_keys`) and all 28
   pre-existing `resp-store` tests — **34/34 total, `resp-store`**.
   `resp-proto` (28/28) and `pg_resp --lib` (27/27 dispatch tests)
   re-confirmed green in the same pass, unaffected by these changes.

## What's explicitly NOT done (real Phase 2 scope, not pre-work)

- Wiring `active_expire_sweep` into `pg_resp`'s event loop (a periodic tick).
- `pg_resp.max_memory` / `pg_resp.eviction` GUCs (Phase 1's report already
  noted these are deferred until the features they configure exist).
- The 30-minute memtier soak + RSS-plateau gate.
- Measuring the *real* per-entry overhead constant (needs a memory
  profiler against a live process, replacing the `PER_ENTRY_OVERHEAD_BYTES
  = 64` placeholder).
- `resp.stats()` (keys/hits/misses/evictions/used_bytes) — Phase 3 SQL
  surface scope per bible §3.3, not Phase 2's in-memory mechanism.
- Tuning `DEFAULT_SAMPLE_SIZE` or the sampling strategy against real
  workload shapes.
- SCAN cursor correctness under concurrent writes (bible §5 Phase 2 gate,
  T1-tier command — not touched, no SCAN command exists yet).

## Next session

Start Phase 2 properly: `/kickoff 2` (plans on Opus from bible §5 + this
file + `reports/phase1.md`), review, then execute — wiring the above into
the live server, GUCs, the soak test, and the remaining T1/T2 command tier.
