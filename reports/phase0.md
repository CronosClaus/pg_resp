# Phase 0 report — STOPPED-FOR-HUMAN (overnight autonomous run)

**Status:** Phase 0 is **not** closed. Items 1-4 of the kickoff plan (prior-art
sweep, reference clone, ref-digestion, resp-protocol skill fill, cargo-pgrx +
PG16/17/18 toolchain build) are done and green. Item 5 (spikes S1/S2, gate 0d)
is **blocked on a sudo-requiring system dependency** — see
`reports/BLOCKED.md` for the exact command and rationale. Items 6-7 (fill
pgrx-patterns from spike learnings, finalize this report) cannot proceed until
S1/S2 actually run. Stages B and C of this run's plan were not started.

This supersedes the prior interim `reports/phase0.md` (Windows/Git-Bash
session, item 1 only) — that session's content is preserved below in
"History" since it's still the accurate record of what happened before this
session.

## Gate table (bible §5 Phase 0)

| gate | pass condition | result |
|---|---|---|
| 0a prior-art sweep | verdict CLEAR/PIVOT/DEAD | **CLEAR** — `reports/prior-art.md` |
| 0b Spike S1 — socket lifecycle | PING→PONG; `pg_ctl stop -m fast` <2s clean; restart×20 zero failures; `kill -9` → postmaster restarts worker | **BLOCKED** — crate does not compile, see below |
| 0c Spike S2 — loopback + commit hook | queued op applies only on commit, not rollback, demonstrated in a pgrx test | **BLOCKED** — same cause, not attempted (S1 gates first per plan order) |
| 0d toolchain matrix | S1/S2 compile+pass on pg16/17/18 | **BLOCKED** — PG16.14/17.10/18.4 all built successfully via `cargo pgrx init`; the blocker is pgrx's own `bindgen` build step (needs libclang), independent of which PG version is targeted, so this would block identically on all three even once S1 exists |

## What's done this session (2026-08-04, WSL2, autonomous overnight run)

1. **Environment pre-flight:** `uname -r` confirms WSL2. `cargo` 1.97.1 on
   PATH. **`docker` CLI is not installed at all** in this WSL2 distro (not
   merely a stopped daemon) — pre-scopes Phase 1's compat-matrix and Valkey
   differential gates to `PARTIAL(docker)` per this run's pre-approved
   amendments; not re-litigated here since Stage B was never reached.
   `cargo-pgrx` was not installed and `~/.pgrx` did not exist at session
   start.
2. **Prior-art sweep (0a):** already CLEAR from the prior (Windows-side)
   session — no re-run needed, see `reports/prior-art.md`. This session added
   an addendum with Redka's own published throughput numbers (from the
   ref-digest below), which sharpens bible §10's "≥5× K-pg" gate: Redka-on-PG
   measures ~25k GET/s, ~11k SET/s in Redka's own docs, vs. ~139k/~133k for
   their Redis control — i.e. the ≥5× bar is calibrated against a real,
   source-confirmed number, not a guess.
3. **`scripts/clone-refs.sh`:** all 7 reference repos cloned and pinned
   (`docs/refs/PINS.md`). `pgrx` clone's `develop` branch tip happened to
   equal tag `v0.19.2` exactly (verified by fetching the tag and diffing
   commit hashes) — relabeled in PINS.md for clarity, no actual drift.
4. **ref-digester × 5 (7 repos, one combined run for the three smallest):**
   all digests written to `docs/refs/*-notes.md` — `postgres-notes.md`,
   `pgrx-notes.md`, `valkey-notes.md`, `redka-notes.md`, `omnigres-notes.md`,
   `pg_net-notes.md`, `memtier_benchmark-notes.md`. Notably, `pgrx-notes.md`
   and `postgres-notes.md` both independently surfaced the same critical
   fact needed for S1's design: **`SetLatch()`/`wait_latch()` only wake the
   PG-registered bgworker thread; a separate OS thread's own accept/epoll
   loop is never woken by SIGTERM.** This directly shaped the S1 code's
   design (a polled `AtomicBool` shutdown flag on a 50ms interval, documented
   in the source as a deliberate v0.1 simplification vs. the bible's
   production self-pipe/eventfd design for a real event loop).
5. **resp-protocol skill filled** (`.claude/skills/resp-protocol/SKILL.md`):
   RESP2 framing (all 5 types + null-vs-empty trap), inline command handling,
   error taxonomy (`ERR`/`WRONGTYPE`/`NOAUTH`/`NOPROTO`), SET option decision
   table, TTL/EXPIRE return-value contract, client-handshake-probe stub table
   (HELLO/CLIENT SETINFO/COMMAND/INFO/SELECT/PING), and 38 byte-level test
   vectors covering all T0 commands. Cross-checked line-by-line against
   `valkey-notes.md` once that digest landed — the `-2`/`-1` TTL ordering and
   the exact `value is not an integer or out of range` error string are
   **source-confirmed**, not recalled from memory. Also flagged: this Valkey
   version's `SET` has an `IFEQ` option not in bible §3.4's T0 scope — noted
   so it isn't mistaken for a missing feature later, not implemented.
6. **`cargo-pgrx` 0.19.2 installed**, then `cargo pgrx init --pg16 download
   --pg17 download --pg18 download` run to completion: PG 16.14, 17.10, 18.4
   all downloaded, configured, compiled, and data directories initialized
   under `~/.pgrx` (`~/.pgrx/config.toml` lists all three `pg_config` paths).
   **This part of the toolchain works correctly** — the later blocker is
   specific to pgrx's own FFI-binding generation step, not the PG builds.
7. **Workspace + S1 spike code written:** root `Cargo.toml` (workspace,
   `members = ["crates/pg_resp"]`, `[profile.dev]`/`[profile.release]` moved
   here per cargo's workspace-root-only profile rule), `crates/pg_resp/`
   scaffolded via `cargo pgrx new pg_resp --bgworker` and then hand-written
   for the actual S1 spike: binds `127.0.0.1:6399`, replies hardcoded
   `+PONG\r\n` to anything, registers via `BackgroundWorkerBuilder` with
   `set_restart_time(Some(Duration::from_secs(1)))` (so the `kill -9` /
   postmaster-restart gate is exercisable once this compiles), main
   (registered) thread does PG lifecycle only, server thread is a plain
   `std::thread::spawn` polling a shared `AtomicBool` shutdown flag every
   50ms. **Never successfully compiled** — see gate table and next section.

## Why blocked

`cargo build -p pg_resp` fails inside `pgrx-pg-sys`'s build script:
`bindgen` cannot find `libclang` (no `clang`, no `libclang*.so*` anywhere on
this machine — confirmed via `find / -iname "libclang*.so*"`, zero hits).
This is a hard requirement for compiling **any** pgrx extension, on **any**
targeted PG version — full detail, exact error text, and the two considered
fix paths (one needs sudo, one would mean writing a large binary outside this
run's allowed paths) are in `reports/BLOCKED.md`. Per this run's HARD STOPS
rule ("anything needing sudo → `reports/BLOCKED.md` with exact commands"),
this session stops here rather than improvising a workaround.

## Plan amendments (carried forward, unchanged from the prior interim report)

1. **Skill-fill timing.** `bench-harness` fills at the start of official
   Phase 2 bench work (not tonight); `pg-conventions` at the start of Phase 1
   proper. Confirmed, unchanged — neither was touched this session, correctly.
2. **PG18-drop contingency:** pre-approved as a `D<n>` decision **only if**
   gate 0d actually fails on pg18 specifically. Gate 0d has not actually run
   (blocked before any PG-version-specific behavior could be observed — the
   libclang gap is version-independent), so this contingency is **not
   triggered** and no new decision-log entry exists. If, after the sudo fix,
   0d fails specifically on pg18 (not 16/17), this contingency is still live
   and pre-approved.

## Next session (resume here)

1. Human runs `sudo apt-get update && sudo apt-get install -y libclang-dev`
   (see `reports/BLOCKED.md` for why this exact command is believed
   sufficient — a candidate package is confirmed present in apt).
2. `cd crates/pg_resp && cargo build` — confirm it compiles.
3. Continue Stage A item 5 from there: run the S1 gate table for real
   (`redis-cli PING`/inline `PING\r\n` against `127.0.0.1:6399`,
   `pg_ctl stop -m fast` timing, `pg_ctl restart` ×20 loop, `kill -9` +
   restart-time observation), then write spike S2 (loopback RESP call +
   `RegisterXactCallback`, commit vs. rollback — API already digested in
   `docs/refs/pgrx-notes.md`'s "Transaction Callbacks" section, not yet
   written as code).
4. Re-run S1/S2 across pg16/pg17 (`cargo build --no-default-features
   --features pg16`, etc.) for the real 0d matrix.
5. Fill `.claude/skills/pgrx-patterns/SKILL.md` from what S1/S2 actually
   showed (not just the digests) — the digests already contain most of the
   API-shape facts; this step should mainly add spike-specific traps (timing
   numbers, anything that took >30 min in practice, any drift between the
   digest and the compiled reality).
6. Rewrite this report's gate table with real numbers, close Phase 0, update
   `CLAUDE.md`'s phase line to 1, then proceed to Stage B (Phase 1 plan) per
   the original run instructions.

## History (prior session, Windows/Git-Bash, preserved verbatim)

Only item 1 (0a prior-art sweep) of the approved kickoff plan was executed
that session; items 2-11 were held back because that session's shell was
Git Bash on Windows (win32), not WSL2/Linux, and pgrx needs a real Linux
toolchain. The human explicitly held all other Phase 0 items back rather than
let `/ref` get cloned or digests get written on the Windows side (gitignored,
wouldn't transfer to WSL2; CRLF contamination risk). All of items 2-11 from
that report are now superseded by "What's done this session" above, except
6-11 (toolchain matrix runtime gates, S2, pgrx-patterns fill, final close-out)
which remain open for the reasons stated above.
