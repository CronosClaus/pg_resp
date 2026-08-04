# RESOLVED — see addendum at bottom. Kept for the record of what happened and why.

# BLOCKED — sudo required to unblock Phase 0 spikes S1/S2

**Date:** 2026-08-04 (overnight autonomous run, Stage A / Phase 0)
**Blocks:** Stage A item 5 (spikes S1/S2, gate 0d) and everything downstream of it
(item 6 pgrx-patterns skill fill from spike learnings, item 7 final phase0.md,
Stage B, Stage C). Everything upstream of this (items 1-4) is done and green.

## What's blocked

`cargo build -p pg_resp` (the pgrx extension crate, scaffolded via
`cargo pgrx new --bgworker`) fails during `pgrx-pg-sys`'s build script:

```
thread '<unnamed>' panicked at bindgen-0.72.1/lib.rs:616:27:
Unable to find libclang: "couldn't find any valid shared libraries matching:
['libclang.so', 'libclang-*.so', 'libclang.so.*', 'libclang-*.so.*'],
set the `LIBCLANG_PATH` environment variable to a path where one of these
files can be found (invalid: [])"
```

pgrx's `pgrx-pg-sys` crate generates Postgres FFI bindings at build time via
`bindgen`, which requires `libclang` (a *shared library*, not just a `clang`
binary) to parse the PG C headers. This machine has neither `clang` nor
`libclang` anywhere (`find / -iname "libclang*.so*"` — zero hits; no
`/usr/lib/llvm*`), and this is a hard requirement for compiling **any** pgrx
extension crate on **any** PG version (16/17/18 all hit the identical error) —
this is not a per-version problem, it blocks the entire pgrx toolchain matrix
(0d), not just pg18.

## Why this needs a human

Two options exist to fix it, and both are outside what this run is authorized
to do unilaterally:

1. **`sudo apt-get update && sudo apt-get install -y libclang-dev`** — a
   candidate package exists in this machine's apt sources
   (`apt-cache policy libclang-dev` → `Candidate: 1:14.0-55~exp2`), so this is
   almost certainly a one-command fix. Requires sudo; `sudo -n true` confirms
   a password is required (no passwordless sudo configured), so this run
   cannot silently proceed.
2. Download a prebuilt/portable `libclang.so` into `~/.cargo` (an allowed
   write path) and set `LIBCLANG_PATH` for the build. Technically avoids
   sudo, but this run's HARD STOPS explicitly forbid "any write outside this
   repo, `~/.pgrx`, `~/.cargo`" *without it being a clearly-scoped, intended
   action* — silently vendoring a multi-hundred-MB LLVM binary into
   `~/.cargo` to route around a system-package gap is exactly the kind of
   improvisation the run's rules say to avoid; it's also fragile (version/ABI
   matching against whatever `bindgen`/`clang-sys` expects) and would need to
   be re-justified every time the toolchain changes. Flagging as a possible
   fallback if sudo is truly unavailable, not taking it unilaterally.

## Recommended action (for the human)

```bash
sudo apt-get update
sudo apt-get install -y libclang-dev
```

Then resume this run at Stage A item 5: `cd crates/pg_resp && cargo build` (or
re-invoke the phase 0 kickoff/session) — everything needed for the S1/S2
spikes (crate scaffold, bgworker code, workspace Cargo.toml) is already
written and committed; it has simply never successfully compiled.

## State of everything else (not blocked)

- 0a prior-art sweep: **CLEAR** (`reports/prior-art.md`, includes a Phase-0
  addendum with Redka's own published throughput numbers).
- Reference corpus: all 7 repos cloned and pinned (`docs/refs/PINS.md`).
- 7 ref-digests written (`docs/refs/*-notes.md`) including a detailed
  `pgrx-notes.md` and `postgres-notes.md` that already answer most of what
  S1/S2 need to know architecturally (bgworker registration API, signal
  handling, the "separate OS thread is never woken by SetLatch" limitation,
  xact-callback API) — the code is written from this, just unbuilt.
- `resp-protocol` skill: fully filled (framing, error taxonomy, SET decision
  table, 38 byte-level test vectors), cross-checked against
  `docs/refs/valkey-notes.md`.
- `cargo-pgrx` 0.19.2 installed; pg16.14/pg17.10/pg18.4 downloaded, configured,
  compiled, and initialized under `~/.pgrx` (`~/.pgrx/config.toml` has all
  three `pg_config` paths) — **this part of the toolchain is fully working**,
  the gap is specifically pgrx's own bindgen step needing libclang, not the
  PG builds themselves.
- Workspace `Cargo.toml` + `crates/pg_resp/` (bgworker scaffold, S1 spike code
  binding `127.0.0.1:6399`, hardcoded `+PONG\r\n`, shutdown via a polled
  `AtomicBool` since a separate OS thread can't be latch-woken) are written
  and committed, just never compiled successfully.
- Docker: confirmed absent from this WSL2 distro entirely (not just daemon
  down) — this was already pre-scoped as a PARTIAL(docker) condition for
  Phase 1's compat-matrix/differential gates per the run's amendments, so it
  is not a new blocker, just re-confirmed here.

No destructive or irreversible action was taken. `reports/phase0.md` is being
updated to STOPPED-FOR-HUMAN alongside this file, per the run's HARD STOPS
discipline ("write state, commit, end — never improvise past").

## Addendum — resolved without sudo, human explicitly authorized proceeding

The human confirmed `sudo` was not going to be available (no password to
give) and explicitly said to push through to completion using option 2 from
above (previously flagged but not taken unilaterally). Path taken, no system
or Anaconda writes, everything under `~/.cargo` (an allowed path):

1. `pip download libclang --no-deps -d /tmp/libclang-dl` — the PyPI `libclang`
   wheel bundles a prebuilt `libclang.so` (this is the package clang's own
   Python bindings use; it ships only the shared library, not clang's
   resource-dir headers).
2. Unzipped the wheel, copied
   `libclang-18.1.1.data/platlib/clang/native/libclang.so` to
   `~/.cargo/libclang-lib/libclang.so`.
3. First retry with `LIBCLANG_PATH` set got further (bindgen found libclang,
   detected "clang version 18.1.1") but failed on
   `/usr/include/stdio.h:33:10: fatal error: 'stddef.h' file not found` —
   clang needs its own bundled freestanding headers (stddef.h, stdarg.h,
   etc.), which the pip wheel does not include.
4. `apt-get download libclang-common-14-dev` — **`apt-get download` (unlike
   `apt-get install`) does not need root**, it just fetches the `.deb` to the
   working directory. Package chosen because `apt-cache policy` showed it as
   an installable candidate on this machine's configured sources
   (Ubuntu 22.04 jammy/jammy-updates).
5. `dpkg-deb -x <deb> extracted/` — extracts a `.deb`'s file contents without
   installing or touching any system database; also does not need root.
6. Found `extracted/usr/lib/llvm-14/lib/clang/14.0.0/include/stddef.h` (and
   everything else clang's resource dir needs) — copied the whole `include/`
   directory to `~/.cargo/clang-resource/include/`.
7. Rebuilt with `LIBCLANG_PATH=~/.cargo/libclang-lib
   BINDGEN_EXTRA_CLANG_ARGS="-isystem ~/.cargo/clang-resource/include"` —
   **bindings generated successfully, `cargo build -p pg_resp` finished**
   (after also fixing an unrelated `pg_module_magic!` macro-invocation bug in
   the hand-written S1 code — see the commit that closes this out).
8. Made this durable: `~/.cargo/config.toml` (global, machine-local, **not**
   committed to the repo since it hardcodes this machine's home directory
   path) sets `[env] LIBCLANG_PATH` / `BINDGEN_EXTRA_CLANG_ARGS` so every
   future `cargo`/`cargo pgrx` invocation picks this up automatically without
   manual prefixing.

**Why this is recorded as an addendum rather than a rewrite:** the original
blocked state was the correct call at the time — sudo genuinely wasn't
available and this run's rules are explicit about not silently routing
around a sudo requirement. The human's follow-up instruction changed the
authorization, not the facts; this addendum documents exactly what was done
so a future session (or a differently-configured machine) can either repeat
it or, better, just run the one real `apt-get install libclang-dev` command
if sudo ever becomes available, and delete `~/.cargo/config.toml`'s `[env]`
block since it becomes unnecessary at that point.
