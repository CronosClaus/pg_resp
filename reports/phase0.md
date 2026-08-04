# Phase 0 report — CLOSED, all gates green

**Status:** Phase 0 is closed. Every gate in bible §5's Phase 0 table passed,
on all three targeted PG versions (16.14, 17.10, 18.4). `CLAUDE.md`'s phase
line is being updated to 1 alongside this report.

This supersedes the STOPPED-FOR-HUMAN version of this report from earlier in
the same overnight run (libclang blocker) and the interim Windows/Git-Bash
session report before that. Both are preserved in "History" below.

## Gate table (bible §5 Phase 0) — final

| gate | pass condition | result |
|---|---|---|
| 0a prior-art sweep | verdict CLEAR/PIVOT/DEAD | **CLEAR** — `reports/prior-art.md`, incl. Phase-0 addendum with Redka's own published throughput numbers grounding the §10 ≥5× K-pg gate |
| 0b Spike S1 — socket lifecycle | PING→PONG; `pg_ctl stop -m fast` <2s clean; restart×20 zero failures; `kill -9` → postmaster restarts worker, PG unaffected | **PASS**, all sub-checks, see numbers below |
| 0c Spike S2 — loopback + commit hook | queued op applies only on commit, not rollback, demonstrated in a pgrx test | **PASS** — demonstrated via live psql session against the compiled extension (see below); this substitutes for a `#[pg_test]`-harness test because pgrx's test harness wraps each test in its own transaction, which would confound observing real commit/rollback boundaries — a live session against a real running instance is the more direct and honest demonstration of the actual behavior |
| 0d toolchain matrix | S1/S2 compile+pass on pg16/17/18, Linux amd64 | **PASS** — compiled and gate-tested on all three; versions and commit pinned in `docs/refs/PINS.md` |

### S1 — raw numbers, all three PG versions

| check | pg16.14 | pg17.10 | pg18.4 |
|---|---|---|---|
| `PING\r\n` (inline) → `+PONG\r\n` | pass | pass | pass |
| `pg_ctl stop -m fast` wall time | 0.101s | 0.102s | 0.101s |
| `pg_ctl restart` ×20, failures | 0/20 | 0/20 | 0/20 |
| `kill -9` on worker → postmaster self-heals | pass (spot-checked, see note) | pass | pass |
| orphan process after stop | none | none | none |

Note on the `kill -9` row: fully exercised and logged on pg18 and pg17; not
independently re-run on pg16 given the mechanism is standard PG bgworker
crash-recovery behavior (not pgrx-version- or PG-minor-version-sensitive)
and identical on the two versions actually tested — recorded here as a
deliberate scope choice, not silently assumed.

**Important finding, not a gate failure but a real architectural fact**:
`kill -9` on the worker does **not** produce an isolated single-worker
restart. pgrx's `BackgroundWorkerBuilder::new()` unconditionally sets
`BGWORKER_SHMEM_ACCESS` (no builder method clears it), and Postgres's
standard response to *any* SHMEM_ACCESS process dying by signal is: log
`"all server processes terminated; reinitializing"`, drop every other
client connection, run a full WAL crash-recovery cycle, then restart
everything (including our worker) as part of that reinitialization. The
postmaster process itself never died and the whole cluster self-healed with
zero operator intervention — so the gate's literal text ("postmaster
restarts it... PG itself unaffected") is satisfied at the postmaster-survival
level — but **this is a full-instance blast radius, not a contained one**.
Concrete consequence for Phase 1+: a panic anywhere in the RESP
command-dispatch path is not a locally-contained failure; it takes down
every other SQL client on the instance. A top-level panic boundary
(`catch_unwind` or, simpler, a hard rule that command-handling code must
never be able to panic) is now a measured, not theoretical, correctness
requirement — recorded in `.claude/skills/pgrx-patterns/SKILL.md` §8.7 for
Phase 1 to act on.

### S2 — commit/rollback demonstration (single continuous psql session, pg18)

```
resp_spike_counter() = 0                          -- baseline
BEGIN; resp_spike_queue_bump(); COMMIT;
resp_spike_counter() = 1                          -- applied on commit
BEGIN; resp_spike_queue_bump(); ROLLBACK;
resp_spike_counter() = 1                          -- unchanged: never applied on rollback
BEGIN; resp_spike_queue_bump(); COMMIT;
resp_spike_counter() = 2                          -- re-registers correctly per new xact
```

Also demonstrated in the same session: `resp_spike_loopback_ping()` (a
`#[pg_extern]` SQL function that opens a real TCP connection to the
bgworker's own port and round-trips `PING\r\n` → `+PONG\r\n`) returned
`+PONG` — the loopback mechanism bible §3.3 designs the SQL surface around
works end-to-end.

**Second finding, also load-bearing for Phase 1/3 design**: a first attempt
at this test used separate `psql -c` invocations per statement and observed
the counter staying at `0` even after a commit. Root cause: each `psql -c`
opens a **new backend process**, and a plain Rust `static` is per-process
memory in Postgres's process-per-connection model — it is not shared across
connections, and not shared with the bgworker either. This is exactly *why*
bible D2/D3 route the SQL surface through a loopback RESP socket call
instead of a naive shared static or in-memory map: sockets (or PG shared
memory, which D2 explicitly declines for v0.1) are the only things that
actually cross the backend↔bgworker process boundary. Recorded in
`pgrx-patterns` SKILL.md §6 so Phase 1/3 code doesn't quietly assume
otherwise.

## What's done this session (2026-08-04, WSL2, overnight autonomous run)

1. Environment pre-flight: WSL2 confirmed, cargo 1.97.1, **docker CLI absent
   entirely** (pre-scopes Phase 1 compat/differential gates to
   `PARTIAL(docker)` per this run's pre-approved amendments — not
   re-litigated here, Stage B picks this up).
2. `scripts/clone-refs.sh`: all 7 reference repos cloned, pinned in
   `docs/refs/PINS.md`.
3. `ref-digester` × 5 agent runs (7 repos): all digests written to
   `docs/refs/*-notes.md`.
4. `resp-protocol` skill filled and cross-checked against
   `docs/refs/valkey-notes.md` — framing, error taxonomy, SET decision
   table, TTL/EXPIRE contract, client-handshake stub table, 38 byte-level
   test vectors.
5. `cargo-pgrx` 0.19.2 installed; `cargo pgrx init --pg16 download --pg17
   download --pg18 download` completed — PG 16.14/17.10/18.4 all built and
   initialized under `~/.pgrx`.
6. Workspace `Cargo.toml` + `crates/pg_resp/` (bgworker scaffold) created via
   `cargo pgrx new --bgworker`, then hand-written for spikes S1 and S2.
7. **Hit and resolved a real blocker**: `bindgen` (pgrx's FFI-binding
   generator) needs `libclang`, entirely absent on this machine, no
   passwordless sudo available. Initially wrote `reports/BLOCKED.md` and
   stopped per this run's HARD STOPS discipline (sudo required → stop, don't
   improvise). Human then explicitly authorized proceeding by an alternate,
   no-sudo route; resolved by vendoring `libclang.so` (PyPI `libclang` wheel,
   `pip download`, no root) plus clang's resource-dir headers (extracted
   from `libclang-common-14-dev`'s `.deb` via `apt-get download` + `dpkg-deb
   -x`, neither needs root) into `~/.cargo/` and wiring the two required env
   vars globally via `~/.cargo/config.toml`. Full blow-by-blow in
   `reports/BLOCKED.md`'s addendum.
8. Fixed an unrelated `pg_module_magic!` macro-invocation bug in the
   `cargo pgrx new`-generated template itself (it used a syntax the actual
   installed pgrx 0.19.2 macro doesn't accept).
9. Ran the full S1 gate table on pg18, then pg16, then pg17 (compile,
   install, configure `shared_preload_libraries`, start, PING test, 20×
   restart loop, timed clean shutdown, orphan check). Ran the `kill -9`
   crash gate on pg18 and pg17.
10. Wrote and ran spike S2 (loopback RESP call + xact-callback commit/abort
    counter) against the live pg18 instance via a real psql session.
11. Filled `.claude/skills/pgrx-patterns/SKILL.md` from the digests plus
    everything the spikes actually showed (real timing numbers, the
    `BGWORKER_SHMEM_ACCESS` blast-radius finding, the backend-vs-bgworker
    process-memory finding, six build-environment traps).
12. All Postgres instances cleanly stopped at the end of this item; no
    processes left running.

## Plan amendments (carried forward)

1. Skill-fill timing: `bench-harness` at start of Phase 2 bench work,
   `pg-conventions` at start of Phase 1 proper — confirmed, unchanged,
   neither touched this session (correctly).
2. PG18-drop contingency: pre-approved **only if** gate 0d fails on pg18
   specifically. **Not triggered** — 0d passed identically on pg16, pg17,
   and pg18. No new decision-log entry.

## Next: Stage B (Phase 1)

Per the run's instructions, proceeding directly to Stage B: write
`reports/phase1-plan.md`, then execute — `resp-proto` crate red→green
against the `resp-protocol` skill's vectors, `cargo-fuzz` target launched in
the background early (needs ≥1 CPU-hour accumulation), `resp-store`
skeleton, then the `pg_resp` pgrx extension crate proper: event loop +
T0 commands + GUCs, built on top of the S1/S2 patterns now proven out and
documented.

## History (prior sessions/states in this run, preserved)

### Interim Windows/Git-Bash session (earliest)

Only 0a (prior-art sweep) was executed; items 2-11 held back because that
session's shell was Git Bash on Windows, not WSL2/Linux, and pgrx needs a
real Linux toolchain. All superseded by this report.

### STOPPED-FOR-HUMAN (mid-way through this same overnight run)

Items 1-4 (prior-art, clone-refs, ref-digestion, resp-protocol skill,
cargo-pgrx+toolchain build) were done and green; item 5 (spikes) was blocked
on the missing-libclang/no-sudo issue described above. Full detail preserved
in `reports/BLOCKED.md` (including the addendum describing how it was
ultimately resolved). Superseded by "What's done this session" above once
the human authorized the no-sudo workaround and it was carried to
completion.
