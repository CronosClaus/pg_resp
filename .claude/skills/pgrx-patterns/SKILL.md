---
name: pgrx-patterns
description: pgrx 0.19.x recipes for pg_resp — background worker registration/lifecycle/signals, GUC registration, transaction callback pattern, the off-main-thread forbidden list. Consult before writing or modifying ANY code that touches pgrx or PG FFI.
---
# pgrx patterns

Filled from `docs/refs/pgrx-notes.md` + `docs/refs/postgres-notes.md` (digests)
and Phase 0 spikes S1 (socket lifecycle, `crates/pg_resp/src/lib.rs`) and S2
(loopback RESP call + xact callback), run against real pg16.14/pg17.10/pg18.4
instances. Everything below is either digest-sourced (marked) or
spike-confirmed against a running server (marked).

## 1. Bgworker registration

```rust
::pgrx::pg_module_magic!(name, version);   // NOT pg_module_magic!(c"...", ...) — see traps
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    BackgroundWorkerBuilder::new("pg_resp")
        .set_function("background_worker_main")   // must match #[unsafe(no_mangle)] fn name
        .set_library("pg_resp")
        .set_restart_time(Some(Duration::from_secs(1)))  // None = BGW_NEVER_RESTART
        .load();
}
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn background_worker_main(_arg: pg_sys::Datum) { /* ... */ }
```
- Requires `shared_preload_libraries = 'pg_resp.so'` in `postgresql.conf`,
  set **before** postmaster start — `_PG_init` runs at postmaster startup, a
  bgworker registered any other way never gets launched (spike-confirmed:
  forgetting this line means the worker's log lines simply never appear, no
  error).
- `BackgroundWorkerBuilder::new()` **unconditionally** sets
  `BGWORKER_SHMEM_ACCESS` in its `bgw_flags` — there is no builder method to
  clear it (pgrx/src/bgworkers.rs:562, "required since Postgres 15"). This is
  not optional and has a real consequence — see §5.
- `enable_spi_access()` additionally sets
  `BGWORKER_BACKEND_DATABASE_CONNECTION` and forces
  `bgw_start_time = RecoveryFinished`; skip it entirely if the worker never
  touches SPI (pg_resp's store is bgworker-local heap, D2 — S1/S2 never
  called `enable_spi_access()` and worked fine without it).

## 2. Signal handling & the main loop

```rust
BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
while BackgroundWorker::wait_latch(Some(Duration::from_secs(10))) {
    if BackgroundWorker::sighup_received() { /* reload config */ }
}
// wait_latch returned false: SIGTERM received or postmaster died. Clean up and return.
```
- `wait_latch` returns `false` on SIGTERM **or** postmaster death — it does
  not distinguish the two; if you need to tell them apart, check
  `BackgroundWorker::sigterm_received()` (digest-sourced,
  `docs/refs/pgrx-notes.md`).
- `sigterm_received()`/`sighup_received()` are atomic swap-and-reset: calling
  twice in a row returns `false` the second time even if nothing new
  happened. Call exactly once per loop iteration (digest-sourced).
- **Spike-confirmed shutdown timing:** `pg_ctl stop -m fast` → full clean
  exit in **~0.10s** (measured identically on pg16/17/18: 0.101s, 0.101s,
  0.102s), including our own server thread joining. Nowhere close to the
  gate's 2s ceiling.

## 3. Separate OS threads: the one rule that matters most

**`SetLatch()`/`wait_latch()` only wake the PG-registered bgworker thread. A
plain `std::thread::spawn`'d thread's own `accept()`/`epoll` loop is never
woken by SIGTERM** (digest-sourced from both `docs/refs/pgrx-notes.md` and
`docs/refs/postgres-notes.md` independently, and this is *the* fact that
shapes the whole event-loop design). Consequences, spike-validated:

- All `BackgroundWorker::*` static methods assert
  `!MyBgworkerEntry.is_null()` — callable **only** from the registered
  entry-point thread, never from a spawned thread (digest-sourced).
- S1's working pattern: main (registered) thread does PG lifecycle only
  (`wait_latch` loop); a spawned thread runs the actual `TcpListener`, polling
  a shared `Arc<AtomicBool>` shutdown flag on a short interval (50ms in the
  spike) instead of blocking indefinitely. On SIGTERM, the main thread sets
  the flag and `.join()`s the spawned thread before returning. This is a
  deliberate v0.1-simplicity choice, not the production design — a real
  mio/epoll event loop should register a self-pipe/eventfd in its own poll
  set so shutdown latency isn't bounded by a poll interval; PG itself doesn't
  expose one as public API (digest-sourced), so the pipe has to be
  hand-rolled by the Rust side, written to from the main thread right where
  S1 currently just flips the `AtomicBool`.

## 4. GUC registration (digest-sourced, not yet spiked — no GUCs exist until Phase 1)

```rust
static MY_GUC: GucSetting<i32> = GucSetting::<i32>::new(256);
// in _PG_init:
GucRegistry::define_int_guc(
    "pg_resp.max_memory", "short desc", "long desc",
    &MY_GUC, 0, i32::MAX, GucContext::Suset, GucFlags::UNIT_MB,
);
```
- `GucSetting<T>` must be a `static`; read with `.get()` on the static
  reference (not on a local copy).
- `GucContext` choices relevant to bible §3.5-3.6: `Suset` (settable by
  superuser via SQL, or at postmaster start/SIGHUP) is the right default for
  `pg_resp.max_memory`, `pg_resp.eviction`, `pg_resp.password` — none of
  these need to be `Postmaster`-only (no bind-address-style restart
  requirement) but should not be `Userset` (any-backend-changeable) either.
  `pg_resp.bind_address` should be `Postmaster` context since changing it
  requires a restart to actually rebind.
- Unit flags (`UNIT_MB`, `UNIT_S`, etc.) make `SHOW pg_resp.max_memory` look
  native — required by bible §8's "zero skepticism" checklist.

## 5. Transaction (xact) callbacks — spike S2, fully demonstrated

```rust
register_xact_callback(PgXactCallbackEvent::Commit, || {
    // apply the queued mutation
});
```
- **Commit and Abort are mutually exclusive per transaction** — exactly one
  fires. Registering only a `Commit` callback is sufficient to get
  commit-only semantics for free; you do not need a matching no-op `Abort`
  registration (spike-confirmed: `resp_spike_queue_bump()` registers only
  `Commit`, and a rolled-back transaction simply never calls it).
- Closure bound: `FnOnce() + UnwindSafe + RefUnwindSafe + 'static`
  (digest+source-confirmed, `pgrx/src/callbacks.rs:158-161`).
- **Spike-measured semantics**, same psql session, one static counter:
  `0 →(BEGIN;bump;COMMIT)→ 1 →(BEGIN;bump;ROLLBACK)→ 1 (unchanged)
  →(BEGIN;bump;COMMIT)→ 2`. Exactly bible §3.3's required contract.
- **Panic/ereport(ERROR) during Commit or Abort firing crashes the entire
  backend and — combined with §6 below — potentially triggers a full cluster
  crash-restart, not just this one backend** (digest+doc-comment-confirmed,
  `pgrx/src/callbacks.rs:150-152`, matches the Postgres internal docs quoted
  there: "the callback occurs post-commit or post-abort, so the callback
  functions can only do noncritical cleanup"). Anything the commit-queue
  applier does in Phase 3 must be infallible / already-validated before this
  point — no `.unwrap()` on I/O to the bgworker's loopback socket here.

## 6. Backend processes vs. the bgworker: no free shared memory

**A plain Rust `static` is per-OS-process, and every Postgres client
connection is a separate OS process** — this is not thread-local, it's
process-local. Spike-confirmed the hard way: `resp_spike_counter()` read `0`
after a `COMMIT` when tested via separate `psql -c` invocations (each opens
a fresh backend process with its own zeroed static); the exact same test
inside one continuous `psql` session showed the real `0→1→1→2` sequence.

This is precisely *why* bible D2/D3 route the SQL surface through a loopback
RESP call to the bgworker rather than a shared Rust static or a naive
in-memory map: **the only things that actually cross the backend↔bgworker
process boundary are PG shared memory (D2 explicitly avoids this for v0.1)
or a socket (D3's loopback design)**. Any Phase 1/3 code that assumes a
`static` visible from `#[pg_extern]` functions is also visible to the
bgworker's own store is wrong and will silently no-op exactly like this
spike's first (flawed) test did.

## 7. The forbidden list (off-main-thread)

Never call any of the following from a spawned OS thread inside the
bgworker — only from the registered entry-point thread:
- Any `BackgroundWorker::*` static method (asserts non-null
  `MyBgworkerEntry`, will panic/UB otherwise)
- `ereport!`/`log!`/pgrx's panic-guard machinery generally assumes PG's
  error-handling context, which is main-thread state
- SPI (`Spi::connect` etc.) — requires a valid PG transaction/memory context
- Any raw `pg_sys::*` FFI call — all of PG's internal state (memory
  contexts, catalog caches, WAL) is set up per-thread/per-process by PG
  itself for the registered entry point only
- Safe alternative for a spawned thread: plain Rust + std/socket APIs only,
  communicate with the main thread via a channel/`Arc<Atomic*>`/mutex — never
  reach for a `pg_sys` call "just this once" from the server thread.

## 8. Build environment traps (cost >30 min each — record so nobody re-derives this)

1. **`pg_module_magic!` invocation changed shape from what `cargo pgrx new`'s
   own generated template uses.** The scaffold's default `src/lib.rs`
   contained `pg_module_magic!(c"pg_resp", pgrx::pg_sys::PG_VERSION)`, which
   fails to compile against the actual pgrx 0.19.2 macro (`no rules expected
   `c"pg_resp"``) — the real macro only accepts `()`, bare
   `(name, version)` idents (pulls from `CARGO_PKG_NAME`/`CARGO_PKG_VERSION`
   automatically), or `(name = c"...", version = c"...")`. Use
   `pg_module_magic!(name, version);` unless a custom name/version string is
   actually needed.
2. **`bindgen` needs `libclang`, a *shared library*, not a `clang` binary —
   and this machine has neither, with no passwordless sudo.** Full
   workaround (no root, everything confined to `~/.cargo`) recorded in
   `reports/BLOCKED.md`: vendor `libclang.so` from the PyPI `libclang` wheel
   (`pip download libclang --no-deps`) into `~/.cargo/libclang-lib/`, then
   separately vendor clang's resource-dir builtin headers (`stddef.h` etc. —
   **not** included in that wheel) from the `libclang-common-14-dev` `.deb`
   via `apt-get download` (no root) + `dpkg-deb -x` (no root) into
   `~/.cargo/clang-resource/include/`. Both env vars
   (`LIBCLANG_PATH`, `BINDGEN_EXTRA_CLANG_ARGS=-isystem ...`) are set
   globally in `~/.cargo/config.toml` (machine-local, intentionally **not**
   committed to the repo — hardcodes this machine's home directory).
3. **Workspace `Cargo.toml` profile placement.** `cargo pgrx new` generates a
   standalone crate with its own `[profile.dev]`/`[profile.release]` in
   `crates/pg_resp/Cargo.toml`. The moment that crate becomes a workspace
   member (needed for the eventual `resp-proto`/`resp-store` sibling
   crates), those profile tables must move to the workspace-root
   `Cargo.toml` — cargo only honors profile tables at the workspace root and
   otherwise errors/ignores them.
4. **Switching PG version for a build/install is a real flag, not a
   default-feature edit each time**: `cargo build -p pg_resp
   --no-default-features --features pg16` (or `pg17`/`pg18`), and
   `cargo pgrx install --pg-config ~/.pgrx/<ver>/pgrx-install/bin/pg_config
   --no-default-features --features pg<ver>` for install. The crate's
   `Cargo.toml` `default = ["pg18"]` only picks the version for a bare
   `cargo build` with no `--features` flag.
5. **`shared_preload_libraries` must be edited in
   `~/.pgrx/data-<ver>/postgresql.conf` by hand** for a `cargo pgrx`-managed
   instance when using raw `pg_ctl`/`psql` directly instead of `cargo pgrx
   run`/`start` — it is not automatically wired up just because the crate
   was `cargo pgrx install`ed.
6. **Socket location varies**: some startup paths listen on
   `~/.pgrx/.s.PGSQL.<port>`, a plain `pg_ctl start -D <data>` (without going
   through `cargo pgrx`'s own wrapper) uses whatever
   `unix_socket_directories` says in `postgresql.conf` (`/tmp` by default in
   this install) — check the actual log line
   (`listening on Unix socket "..."`) or just connect over TCP
   (`-h 127.0.0.1 -p <port>`) to sidestep the ambiguity entirely.
7. **`kill -9` on the worker is not an isolated worker restart** — see §5's
   `BGWORKER_SHMEM_ACCESS`-is-mandatory fact. Spike-confirmed on both pg17
   and pg18: killing the worker process triggers Postgres's standard
   crashed-backend response — `"all server processes terminated;
   reinitializing"`, every other backend/connection dropped, a full WAL
   crash-recovery cycle runs, and *then* the bgworker restarts as part of
   that reinitialization (not gated by `bgw_restart_time`'s delay — the
   restart is immediate because it's riding along with full-instance
   startup, not the isolated-crash-recovery path). The postmaster process
   itself never dies and self-heals with no operator intervention — that
   part of the bible's gate text ("postmaster restarts it... PG itself
   unaffected") holds — but "PG itself unaffected" should be read as
   "the postmaster survives and self-heals," **not** "other connections are
   undisturbed." This row is specifically about an **external SIGKILL of the
   whole process** — see item 8 for the different (and, in its own way,
   worse) failure mode of an **in-process Rust panic**, which does *not*
   trigger this crash-recovery path at all.

8. **An in-process panic in the RESP command-dispatch path does not cause
   the item-7 crash-recovery cycle — it silently kills the entire server
   thread instead, while Postgres and the bgworker process itself stay
   completely healthy.** Deliberately triggered (Phase 2, a temporary panic
   command wired into `dispatch()`, removed after testing) and observed
   directly: `std::thread::spawn` catches the unwinding panic at the OS
   thread boundary as designed — the *process* doesn't crash — but D4's
   single-threaded model means that one thread owns the `mio` listener
   *and every connection*. Losing it means: every existing connection resets
   (empty read), no new connection is ever accepted again (`Connection
   refused`), yet `SELECT 1` against the same Postgres instance keeps
   working fine and the bgworker OS process never exits. There is no
   automatic recovery — the RESP service is simply, silently, and
   *permanently* dead until the next real restart (`pg_ctl restart` or an
   actual crash). An operator or monitoring system watching "is Postgres
   up" would see green the entire time.
   - **This is worse than item 7 in one specific way**: item 7's failure is
     loud (every client disconnects, logs show a crash-recovery cycle) and
     *self-healing* (postmaster relaunches everything automatically). Item
     8's failure is quiet and *permanent* — nothing self-heals it.
   - **Fix (implemented in `pg_resp/src/lib.rs`)**: wrap the per-connection
     dispatch call in `std::panic::catch_unwind` (`AssertUnwindSafe`) so a
     panic drops only that one connection — the thread, the listener, and
     every other connection keep running. Verified: after the fix, the
     *same* deliberate panic dropped only the triggering connection; a
     bystander connection opened beforehand kept working, and a brand-new
     connection after the panic succeeded immediately. A second, outer
     `catch_unwind` around the whole server-thread closure is defense in
     depth (logs loudly if a panic somehow escapes the per-connection fence,
     rather than vanishing) but is not itself sufficient — see the next trap.
   - **Self-inflicted trap while building the fix**: the first attempt
     called `log!()` inside the `catch_unwind` handler, *on the server
     thread* — violating this same skill's §7 forbidden-list rule (`log!`
     is main-thread-only). pgrx enforces that at **runtime**: it panicked
     with `"postgres FFI may not be called from multiple threads"`, and
     that second, unfenced panic escaped straight past both fences, killing
     the thread anyway and defeating the whole fix. Any logging inside a
     server-thread panic handler must use something thread-safe (plain
     `eprintln!` — PG's log collector still captures a bgworker's stderr
     into the same server log), never `log!`/`ereport!`.
