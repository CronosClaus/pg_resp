# pgrx v0.19.2 Background Worker & GUC API Reference

## BackgroundWorkerBuilder Registration

**Struct**: `BackgroundWorkerBuilder` (pgrx/src/bgworkers.rs:537–564)

**Constructor & core methods**:
- `new(name: &str)` – initializes with bgw_name, bgw_type, default flags (BGWORKER_SHMEM_ACCESS for PG15+), start time PostmasterStart, no restart (pgrx/src/bgworkers.rs:558–572)
- `set_function(name: &str)` – bgw_function_name (must be `extern "C-unwind"` + `#[pg_guard]`) (pgrx/src/bgworkers.rs:642–645)
- `set_library(name: &str)` – bgw_library_name (pgrx/src/bgworkers.rs:619–622)
- `set_restart_time(Option<Duration>)` – bgw_restart_time in seconds; None = -1 (no restart) (pgrx/src/bgworkers.rs:611–614)
- `enable_spi_access()` – sets BGWORKER_BACKEND_DATABASE_CONNECTION flag + RecoveryFinished start time (pgrx/src/bgworkers.rs:594–600)
- `set_start_time(BgWorkerStartTime)` – PostmasterStart | ConsistentState | RecoveryFinished (pgrx/src/bgworkers.rs:603–606)
- `load()` – calls `pg_sys::RegisterBackgroundWorker()` + hooks shmem_startup_hook if needed (pgrx/src/bgworkers.rs:702–714)
- `load_dynamic()` – returns Result<DynamicBackgroundWorker, DynamicBackgroundWorkerLoadError> (pgrx/src/bgworkers.rs:718–730)

**Registration in _PG_init**:
- Use `#[pg_guard] pub extern "C-unwind" fn _PG_init()` (pgrx-examples/bgworker/src/lib.rs:31–42)
- Precondition: `shared_preload_libraries` must include extension name (pgrx-examples/bgworker/src/lib.rs:18–27)
- Call `pg_module_magic!(name, version)` macro (pgrx-examples/bgworker/src/lib.rs:29)
- Main function must be `#[pg_guard] #[unsafe(no_mangle)] pub extern "C-unwind" fn bgw_main(arg: pg_sys::Datum)` (pgrx-examples/bgworker/src/lib.rs:44–46)

## Signal Handling

**Primary API**: `BackgroundWorker::attach_signal_handlers(SignalWakeFlags)` (pgrx/src/bgworkers.rs:267–323)

**Signal wake flags** (bitflags, line 38):
- `SIGHUP` (0x1) – config reload
- `SIGTERM` (0x2) – shutdown
- `SIGINT` (0x4) – interrupt
- `SIGCHLD` (0x8) – child signal

**Polling for signals** (all reset flag after reading):
- `BackgroundWorker::sigterm_received()` – bool, atomically swaps GOT_SIGTERM to false (pgrx/src/bgworkers.rs:128–137)
- `BackgroundWorker::sighup_received()` – bool, swaps GOT_SIGHUP to false (pgrx/src/bgworkers.rs:113–122)
- `BackgroundWorker::sigint_received()` – bool, swaps GOT_SIGINT to false (pgrx/src/bgworkers.rs:139–152)
- `BackgroundWorker::sigchld_received()` – bool, swaps GOT_SIGCHLD to false (pgrx/src/bgworkers.rs:154–167)

**WaitLatch equivalent**: `BackgroundWorker::wait_latch(timeout: Option<Duration>) -> bool` (pgrx/src/bgworkers.rs:172–189)
- Returns false if SIGTERM received or postmaster died; true if still alive
- Wraps `pg_sys::WaitLatch()` with WL_LATCH_SET | WL_POSTMASTER_DEATH flags (pgrx/src/bgworkers.rs:766–779)
- Signals do not block; use polling loop with wait_latch timeout

## GUC Registration

**Context enum** (pgrx/src/guc.rs:19–57):
- `GucContext::Internal` – read-only (PGC_INTERNAL)
- `GucContext::Postmaster` – postmaster startup only
- `GucContext::Sighup` – postmaster startup or SIGHUP
- `GucContext::SuBackend` – superuser backend, startup or connection-time
- `GucContext::Backend` – any user backend, startup or connection-time
- `GucContext::Suset` – postmaster startup, SIGHUP, or SQL if superuser
- `GucContext::Userset` – anyone, any time

**Flags bitflags** (pgrx/src/guc.rs:59–105):
- `NO_SHOW_ALL` – exclude from SHOW ALL
- `NO_RESET_ALL` – exclude from RESET ALL
- `REPORT` – auto-report changes to client
- `SUPERUSER_ONLY` – visible to superuser only
- Unit flags: `UNIT_KB`, `UNIT_MB`, `UNIT_BYTE`, `UNIT_MS`, `UNIT_S`, `UNIT_MIN`, etc.

**Type-specific registration** (GucRegistry impl, pgrx/src/guc.rs:298–428):
- `GucRegistry::define_bool_guc(name, short, long, setting, context, flags)` – DefineCustomBoolVariable (line 300–322)
- `GucRegistry::define_int_guc(name, short, long, setting, min, max, context, flags)` – min/max bounds (line 324–350)
- `GucRegistry::define_string_guc(name, short, long, setting, context, flags)` – Option<CString> (line 352–374)
- `GucRegistry::define_float_guc(name, short, long, setting, min, max, context, flags)` – f64 (line 376–402)
- `GucRegistry::define_enum_guc<T: GucEnum>(name, short, long, setting, context, flags)` – custom enum (line 404–428)

**GucSetting wrapper** (pgrx/src/guc.rs:185–201):
- `GucSetting<T>::new(value)` – wraps value in Cell for interior mutability
- `GucSetting::<T>::get()` – reads current value
- Must be static, e.g., `static MY_GUC: GucSetting<i32> = GucSetting::new(42);`

## Transaction Callbacks (Xact)

**API**: `register_xact_callback(PgXactCallbackEvent, closure) -> XactCallbackReceipt` (pgrx/src/callbacks.rs:158–237)

**Event enum** (pgrx/src/callbacks.rs:23–58):
- `PgXactCallbackEvent::Commit` – post-commit, mutually exclusive with Abort
- `PgXactCallbackEvent::Abort` – post-abort, mutually exclusive with Commit
- `PgXactCallbackEvent::PreCommit` – before commit (last chance to abort)
- `PgXactCallbackEvent::ParallelCommit` – parallel worker commit
- `PgXactCallbackEvent::ParallelAbort` – parallel worker abort
- `PgXactCallbackEvent::ParallelPreCommit` – parallel worker pre-commit
- `PgXactCallbackEvent::Prepare` – prepare transaction commit
- `PgXactCallbackEvent::PrePrepare` – prepare transaction pre-commit

**Closure signature**: `FnOnce() + UnwindSafe + RefUnwindSafe + 'static`

**Receipt**: `XactCallbackReceipt` – call `.unregister_callback()` to remove before transaction ends (pgrx/src/callbacks.rs:80–101)

**Underlying call**: wraps unsafe `pg_sys::RegisterXactCallback(Some(callback), null_mut())` (pgrx/src/callbacks.rs:217)

**Critical safety note**: Panic or ereport(ERROR) during Commit/Abort fires immediately cause backend abort and cluster restart (pgrx/src/callbacks.rs:149–157)

## Shared Memory & Locking

**PgLwLock<T> API** (pgrx/src/lwlock.rs:33–111):
- `const unsafe fn new(name: &'static CStr)` – create guard (line 46–48)
- `fn share(&self) -> PgLwLockShareGuard<'_, T>` – acquire shared (read) lock; returns guard with `&T` (line 58–64)
- `fn exclusive(&self) -> PgLwLockExclusiveGuard<'_, T>` – acquire exclusive (write) lock; returns guard with `&mut T` (line 67–73)
- Guard drops release lock (pgrx/src/lwlock.rs:126–131, 165–170)
- Implements `PgSharedMemoryInitialization` – requests shmem + named LWLock tranche in shmem_request, initializes in shmem_startup (pgrx/src/lwlock.rs:76–111)

**PgAtomic<T> API** (pgrx/src/atomics.rs:15–46):
- `const unsafe fn new(name: &'static CStr)` – create wrapper for Rust atomics (line 28–30)
- `fn get(&self) -> &T` – access underlying atomic without locking (line 40–45)
- Suitable for `AtomicBool`, `AtomicI32`, etc. without additional locks (pgrx/src/atomics.rs:20)

**Initialization macro** (pgrx/src/shmem.rs:46–103):
- `pg_shmem_init!(STATIC_VAR)` – registers hooks for shmem_request and shmem_startup
- Must call from `_PG_init` for all shared memory statics
- Hooks chain with previous hooks via `PREV_SHMEM_*_HOOK` (pgrx/src/shmem.rs:66–80, 84–101)

## Thread Safety & Restrictions

**BackgroundWorker methods assertion** (pgrx/src/bgworkers.rs:86, 100, 115, etc.):
- All `BackgroundWorker::*()` static methods check `!pg_sys::MyBgworkerEntry.is_null()`
- **Callable only from the registered background worker's main thread** (main bgworker entry function)
- **NOT safe to call from spawned OS threads** or separate thread pools within the bgworker

**Recommended pattern for multi-threaded bgworker**:
- Entry function runs on registered thread, calls all BackgroundWorker methods (wait_latch, signal checking)
- Spawn separate `std::thread::spawn()` for work; use channel or `AtomicBool` + `condvar` to signal shutdown from registered thread
- Child threads must NOT call BackgroundWorker::*, only direct `pg_sys::` calls if needed (outside PG FFI, safest is to avoid)
- See bgworker example (pgrx-examples/bgworker/src/lib.rs:64–87) – wait_latch in single-threaded loop

## Traps

1. **No heap-allocated shared memory**: Vectors, Strings, etc. not allowed in shared statics. Use `heapless::Vec`, `heapless::Deque`, wrapped in `PgLwLock<AssertPGRXSharedMemory<T>>` (pgrx/src/shmem.rs:14–20, example pgrx-examples/shmem/src/lib.rs:32–38).

2. **`shared_preload_libraries` required**: Static bgworkers must be in that config key; will fail silently otherwise. Check at runtime with `process_shared_preload_libraries_in_progress` (pgrx-examples/bgworker/src/lib.rs:33–35).

3. **GUC setting lifetime**: Must be `static`, not stack or heap. `GucSetting<T>` wraps in `Cell`; call `.get()` on the static reference, not the setting itself (pgrx/src/guc.rs:185–201).

4. **Xact callback closure isolation**: Closures capture only what is safe; cannot panic during Commit/Abort without crashing the cluster. Use `PreCommit` to validate, not Commit/Abort for side effects (pgrx/src/callbacks.rs:149–157).

5. **LWLock poisoning**: Panic or ereport inside a lock guard may leave lock held during elog unwind; pgrx handles this by skipping LWLockRelease if `InterruptHoldoffCount > 0` (pgrx/src/lwlock.rs:180–185). Do not panic in custom code holding a guard.

6. **Signal handlers are atomic stores**: `BackgroundWorker::sigterm_received()` is a swap; calling twice returns false on the second call (pgrx/src/bgworkers.rs:112, 125). Design loop to call once per iteration.

7. **wait_latch returns bool, not event flags**: Returns `false` if SIGTERM or postmaster died; `true` if timeout expired and still running. To detect reason for wake, call signal checkers after (pgrx/src/bgworkers.rs:172–189).

8. **PG15+ requires BGWORKER_SHMEM_ACCESS**: pgrx sets this automatically in BackgroundWorkerBuilder::new() (pgrx/src/bgworkers.rs:562). Older PG13/14 do not require it but pgrx includes it anyway.

9. **enable_spi_access forces RecoveryFinished**: Cannot set start_time manually if enable_spi_access called; start time is overridden (pgrx/src/bgworkers.rs:598). Call set_start_time *after* enable_spi_access if override needed.

10. **pg_module_magic! must come before _PG_init**: Declares extension name/version; required for proper symbol export and metadata (pgrx-examples/bgworker/src/lib.rs:29).
