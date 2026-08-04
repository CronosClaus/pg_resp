# PostgreSQL Background Worker Lifecycle & Signal Handling (PG 18)

**Focus:** Canonical bgworker lifecycle for a pgrx-based worker: registration via shared_preload_libraries, TCP socket + separate event loop thread, clean SIGTERM shutdown within 2s, recovery from SIGKILL, latching/waking primitives for multi-threaded designs.

---

## 1. Worker Registration (Static, via shared_preload_libraries)

**File:** `src/test/modules/worker_spi/worker_spi.c:362–385`

BackgroundWorker struct fields set by worker_spi:
- **bgw_flags = BGWORKER_SHMEM_ACCESS | BGWORKER_BACKEND_DATABASE_CONNECTION** — both required for DB-connected workers (SHMEM_ACCESS is mandatory as of PG 10+, per bgworker.c:642–649)
- **bgw_start_time = BgWorkerStart_RecoveryFinished** — worker starts only after recovery is complete; alternative values: `BgWorkerStart_PostmasterStart` (no DB access allowed), `BgWorkerStart_ConsistentState` (during recovery)
- **bgw_restart_time = BGW_NEVER_RESTART** (-1) or positive integer (restart delay in seconds); if negative and not -1, registration is rejected (bgworker.c:665–667)
- **bgw_library_name, bgw_function_name** — loaded via dlopen + dlsym; for external .so modules, PG uses load_external_function() (bgworker.c:1359–1360)
- **bgw_name, bgw_type** — display names; if bgw_type is empty string, copy is made from bgw_name (bgworker.c:694–695)
- **bgw_notify_pid = 0** for static workers; only dynamic workers can set this to get SIGUSR1 notifications on startup/shutdown (bgworker.c:985–991)
- **bgw_main_arg** — opaque Datum passed to worker's main function; worker_spi uses this as a worker index
- **bgw_extra[BGW_EXTRALEN=128]** — opaque context; worker_spi stores database OID, role OID, flags here

Registration timing: must call RegisterBackgroundWorker() from _PG_init() during postmaster startup (checks `process_shared_preload_libraries_in_progress` flag), not from any child process (bgworker.c:949–969).

---

## 2. Main Thread Signal Handlers & Shutdown Protocol

**File:** `src/test/modules/worker_spi/worker_spi.c:158–163`

```c
pqsignal(SIGHUP, SignalHandlerForConfigReload);
pqsignal(SIGTERM, die);
BackgroundWorkerUnblockSignals();
```

Handler registration must happen **before** BackgroundWorkerUnblockSignals() (which restores signal mask).

**SIGTERM flow (die handler):**
- Handler just sets global `QueryCancelPending` flag (and throws FATAL via ereport if in query context)
- SIGTERM is caught during WaitLatch() and poll/epoll returns
- Main loop calls CHECK_FOR_INTERRUPTS() which checks `QueryCancelPending` → raises ERROR/FATAL
- Worker's main() function exits with code 1 (or 0 on clean shutdown)
- **Gotcha:** SIGTERM handler in bgworker.c:703–712 (`bgworker_die`) directly calls ereport(FATAL, ADMIN_SHUTDOWN), but user-supplied handlers like worker_spi's `die` must NOT do blocking I/O or anything unsafe — just set flags

**SIGHUP handler (SignalHandlerForConfigReload):**
- Sets `ConfigReloadPending` flag
- Main loop checks this flag after waking and calls ProcessConfigFile(PGC_SIGHUP)
- GUC values can change; hot reload of worker_spi.naptime etc. supported

---

## 3. Main Loop: WaitLatch / Shutdown Distinction

**File:** `src/test/modules/worker_spi/worker_spi.c:206–224`

```c
for (;;) {
    WaitLatch(MyLatch,
              WL_LATCH_SET | WL_TIMEOUT | WL_EXIT_ON_PM_DEATH,
              worker_spi_naptime * 1000L,  // timeout in milliseconds
              wait_event_id);
    ResetLatch(MyLatch);
    CHECK_FOR_INTERRUPTS();
    // ... do work ...
}
```

**Wait semantics:**
- **WL_LATCH_SET** — wake on latch set (via SetLatch from postmaster or signal handler)
- **WL_TIMEOUT** — wake after timeout milliseconds
- **WL_EXIT_ON_PM_DEATH** — if postmaster dies, exit immediately (never return to loop); WL_POSTMASTER_DEATH would return a flag instead
- WaitLatch returns immediately if latch is already set (no spurious wake guarantee, so CHECK_FOR_INTERRUPTS() after every WaitLatch is critical)
- ResetLatch() must be called to clear the flag before next WaitLatch (race-proof pattern: check condition, ResetLatch, check again, WaitLatch)

**Distinguishing timeout from postmaster death:**
- Timeout: WaitLatch() returns, CHECK_FOR_INTERRUPTS() is called, no pending signals → loop continues
- SIGTERM from postmaster: signal handler sets flag → CHECK_FOR_INTERRUPTS() raises FATAL → main() exits with code 1
- Postmaster death: WL_EXIT_ON_PM_DEATH causes immediate exit of WaitLatch (no return), worker process terminates
- **Gotcha:** timing window: if SIGTERM arrives just after WaitLatch() returns (during work), it may take until next CHECK_FOR_INTERRUPTS() call (e.g., at SPI_execute, StartTransactionCommand) to exit; PG includes this in graceful shutdown strategy with timeout (SIGKILL after ~180s per postmaster.c)

---

## 4. Worker Restart Semantics & Postmaster Scheduling

**Files:** `src/backend/postmaster/bgworker.c:500–506, 570–624` and `src/backend/postmaster/postmaster.c:1545–1618`

**bgw_restart_time interpretation:**
- **BGW_NEVER_RESTART** (-1): worker is never restarted; when it exits, slot is freed for reuse (ForgetBackgroundWorker called)
- **0 or negative (except -1)**: rejected at registration time as invalid (bgworker.c:665–667)
- **positive integer N (seconds):** worker is restarted N seconds after crash
  - Exit code 0 → always treated as permanent stop, never restarted (ReportBackgroundWorkerExit: line 500–502)
  - Exit code 1, crash, or SIGKILL → eligible for restart
  - rw_crashed_at timestamp is recorded when postmaster detects worker death (parent receives SIGCHLD)

**Postmaster restart scheduling (DetermineSleepTime):**
- Postmaster's main loop (ServerLoop) waits on poll/epoll with timeout from DetermineSleepTime()
- If crashed workers exist and restart_time hasn't elapsed: sleep only until earliest restart deadline (postmaster.c:1573–1604)
- Calculation: `next_wakeup = rw_crashed_at + (bgw_restart_time * 1000 milliseconds)`, clamped to min 60s
- LaunchMissingBackgroundProcesses() is called at bottom of ServerLoop to fork new child processes when time elapses
- **Gotcha:** clock skew: if system time moves backward, restart may be delayed indefinitely; PG does not handle this (no NTP sync assumptions)
- **Gotcha:** postmaster is single-threaded event loop; while it waits for a crashed worker to restart, it accepts no new client connections (select/epoll blocks on timeout)

**After postmaster crashes/restarts (ResetBackgroundWorkerCrashTimes):**
- Workers with BGW_NEVER_RESTART are ForgetBackgroundWorker'd (line 589–598)
- Other workers have rw_crashed_at zeroed → eligible for immediate restart on next ServerLoop iteration (line 615)
- Workers marked with BGWORKER_CLASS_PARALLEL must have bgw_restart_time = BGW_NEVER_RESTART; this prevents accounting bugs (line 681–688)

---

## 5. Signal Waking Primitives: Latch / Self-Pipe / Signalfd

**Files:** `src/backend/storage/ipc/latch.c:289–346` and `src/backend/storage/ipc/waiteventset.c:179–195, 2020–2036`

**SetLatch() on Unix (non-WIN32):**
- Sets latch->is_set flag atomically (with memory barrier)
- If owner is sleeping (maybe_sleeping flag true) and same PID as caller: calls WakeupMyProc()
- If owner is another process: calls WakeupOtherProc(owner_pid)

**WakeupMyProc() (same process, called from signal handler):**
```c
#if defined(WAIT_USE_SELF_PIPE)
    if (waiting)  // volatile sig_atomic_t flag set during WaitLatch()
        sendSelfPipeByte();  // write(selfpipe_writefd, &dummy, 1)
#else
    if (waiting)
        kill(MyProcPid, SIGURG);  // on signalfd systems
#endif
```

**Self-pipe trick (waiteventset.c:243–314):**
- On poll()-based systems: creates a non-blocking pipe pair during InitializeWaitEventSupport()
- Signal handler writes 1 byte to pipe; pipe FD is registered with poll/epoll
- Reading from pipe in WaitLatch() clears the signaled state (drain() function)
- Pipe is inherited by child processes; each child recreates its own pipe to avoid races

**Signalfd alternative (on Linux with epoll + signalfd support):**
- SIGURG is blocked system-wide (sigaddset &UnBlockSig, SIGURG)
- signalfd(-1, SIGURG_mask, SFD_NONBLOCK) creates an FD that becomes readable when SIGURG is pending
- epoll waits on signalfd FD; when signalfd reads, SIGURG signal is consumed atomically
- No self-pipe needed; atomic signal delivery via FD

**Critical limitation for multi-threaded workers:**
- Latches are process-private; SetLatch() only wakes the PG event loop thread waiting in WaitLatch/WaitEventSetWait
- A separate OS thread (e.g., mio-based event loop) running epoll on its own FDs is NOT woken by SetLatch()
- To wake a separate thread's epoll loop from a signal handler, the signal handler would need to write to a pipe or eventfd that the thread's epoll watches
- PG does not expose eventfd primitives; only self-pipe is available (and even that is not public API)
- **Gotcha for pgrx:** if you spawn a server thread with its own epoll loop, you must either:
  1. Have the thread manage its own self-pipe (same pattern as PG)
  2. Use a user-space condition variable protected by a spinlock (unsafe in signal handler)
  3. Accept that the thread won't be woken by SIGTERM until its epoll timeout fires (latency acceptable?)

---

## 6. Dynamic Worker Launch (Startup Notification)

**File:** `src/backend/postmaster/bgworker.c:1045–1138`

For TCP-socket bgworkers that need to signal a launcher process when ready:
- Set bgw_notify_pid = MyProcPid in registration (caller's PID)
- RegisterDynamicBackgroundWorker() returns handle; caller can use WaitForBackgroundWorkerStartup(handle, &pid)
- When postmaster detects worker has started (SIGCHLD handler → ReportBackgroundWorkerPID): sends SIGUSR1 to bgw_notify_pid
- Caller wakes from WaitLatch listening for SIGUSR1, polls GetBackgroundWorkerPid(handle) to get worker's PID
- **Gotcha:** if launcher dies before worker starts, its PID slot is marked invalid; worker still starts but notify goes nowhere

---

## Traps for First-Time Implementer

1. **SIGTERM is not a clean shutdown signal; it's an interrupt flag:**
   - Signal handler does NOT call close() or cleanup directly
   - Signal handler only sets a flag; CHECK_FOR_INTERRUPTS() later in main thread performs the shutdown
   - SIGTERM arrives async; you must assume it can interrupt any PG FFI call
   - If worker holds an exclusive lock during shutdown, postmaster will SIGKILL it after ~180s (SIGKILL_CHILDREN_AFTER_SECS)

2. **ResetLatch must be called before every WaitLatch, even if you think latch is not set:**
   - Latch can be set between your check and your WaitLatch() call (race window)
   - Safe pattern: check condition → ResetLatch() → check condition again → WaitLatch()

3. **WaitLatch is not a yield; it's a sleep:**
   - If you set WL_LATCH_SET and latch is already set, WaitLatch returns immediately
   - But if timeout is -1 (infinite), WaitLatch blocks until latch is set or postmaster dies
   - Default worker_spi timeout is naptime*1000 ms; a config-reload won't wake it sooner unless latch is manually set

4. **bgw_restart_time = 0 is invalid; use BGW_NEVER_RESTART or positive seconds:**
   - 0 is rejected at sanity-check time (bgworker.c:665–667)
   - If you want "restart immediately after crash", use 1 (second)

5. **Separate server threads don't inherit latch waking:**
   - If your pgrx worker spawns a thread with its own mio/epoll loop, SetLatch() won't wake it
   - SIGTERM will interrupt the thread only if it's in a syscall (epoll_wait); otherwise it keeps looping
   - Solution: thread must poll a shared atomic flag or manage its own self-pipe

6. **PostmasterMain cannot be safely called a second time; no idempotent shutdown:**
   - Forked child processes run worker main, not postmaster loop
   - If you spawn a child process from your worker, you are responsible for reaping it (SIGCHLD handler)
   - PostmasterMain in parent (postmaster itself) will handle SIGCHLD and attempt to manage it

7. **Database connection is lazy; must be explicitly established:**
   - BGWORKER_BACKEND_DATABASE_CONNECTION only allows it; it does not auto-connect
   - Call BackgroundWorkerInitializeConnection() or BackgroundWorkerInitializeConnectionByOid() in main before using SPI

8. **bgw_extra payload is passed by value; size is BGW_EXTRALEN (128 bytes):**
   - No pointers; copy data into it (offsets, OIDs, flags)
   - Worker extracts it from MyBgworkerEntry->bgw_extra in its main() (worker_spi.c:151–156)

9. **Postmaster health is not monitored by worker; only one-way signal:**
   - WL_EXIT_ON_PM_DEATH causes worker to exit if postmaster dies
   - Worker cannot query postmaster state; it must rely on this signal
   - If WL_EXIT_ON_PM_DEATH is missing from WaitLatch flags, worker runs as a zombie after postmaster death (until manually killed)

10. **Shared memory access is provided, but not enforced isolated:**
    - BGWORKER_SHMEM_ACCESS does not serialize access; you must use LWLocks for concurrent data
    - LWLock API is in src/include/storage/lwlock.h; allocate from shmem, guard with LWLockAcquire/Release
    - If you use only process-local memory (heap), ignore this; no other worker will touch it

---

## References in Clone

- Worker lifecycle spec: `src/include/postmaster/bgworker.h:1–164`
- Signal handler semantics: `src/backend/postmaster/bgworker.c:703–712` (bgworker_die)
- Latch & WaitEventSet internals: `src/backend/storage/ipc/latch.c` + `src/backend/storage/ipc/waiteventset.c`
- Postmaster restart timing: `src/backend/postmaster/postmaster.c:1545–1618` (DetermineSleepTime)
- Full worker_spi example: `src/test/modules/worker_spi/worker_spi.c`
