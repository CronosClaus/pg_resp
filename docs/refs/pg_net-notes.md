# pg_net: Async HTTP Client + Boring Extension Packaging

## Focus: Bgworker registration, GUC structure (naming, units, context), packaging (control file, versioned SQL scripts), lifecycle

pg_net (C, ~18K LOC in src/) provides async HTTP requests from SQL via standalone bgworker. Precedent for minimal, production-grade PG extension: standard packaging, GUCs, signal handling, shared memory coordination.

### Bgworker Registration & Lifecycle

- **Entry point**: `void _PG_init(void)` at `src/worker.c:466`
  - Enforces: must be in shared_preload_libraries (line 471–474)
  - Registers single bgworker: `RegisterBackgroundWorker()` with:
    - bgw_flags: `BGWORKER_SHMEM_ACCESS | BGWORKER_BACKEND_DATABASE_CONNECTION`
    - bgw_start_time: `BgWorkerStart_RecoveryFinished`
    - bgw_name: "pg_net 0.20.4 worker" (includes version)
    - bgw_function_name: "pg_net_worker"
    - bgw_restart_time: `net_worker_restart_time_sec` (1 second)
  - Lines 477–484
- **Worker entry**: `void pg_net_worker(Datum main_arg)` at line 249
  - Unblocks signals: SIGTERM, SIGHUP, SIGUSR1
  - Initializes connection: `BackgroundWorkerInitializeConnection(guc_database_name, guc_username, 0)`
  - Reports appname: "pg_net 0.20.4" via pgstat_report_appname()
  - Initializes curl_global_init, epoll FD, curl_multi handle
  - Publishes state: WS_RUNNING (atomic write to shared state)
  - Main loop: wait for wake → process batch → sleep (lines 286–292)

### GUC Structure & Naming Convention

Four GUCs defined in _PG_init (lines 496–509), all prefixed `pg_net.`:

1. **pg_net.ttl** (PGC_SIGHUP, string)
   - Default: "6 hours" (interval type)
   - Purpose: cleanup retention for request/response rows
   - No check hook; relies on Postgres interval parser

2. **pg_net.batch_size** (PGC_SIGHUP, int)
   - Default: 200 requests per iteration
   - Range: 0 to PG_INT16_MAX (32767)
   - No unit suffix (count, not bytes/ms)

3. **pg_net.database_name** (PGC_SU_BACKEND, string)
   - Default: "postgres"
   - Purpose: which DB the worker connects to
   - Context PGC_SU_BACKEND: only superuser can set, requires restart

4. **pg_net.username** (PGC_SU_BACKEND, string)
   - Default: NULL
   - Purpose: role for worker connection
   - Context PGC_SU_BACKEND: superuser-only, restart-required

**Naming pattern**: `extension_name.setting_name` (kebab or underscore). All use DefineCustom* (not omni framework).

### Control File & Extension Packaging

- **pg_net.control.in** (2 lines):
  ```
  comment = 'Async HTTP'
  default_version = '@EXTVERSION@'
  relocatable = false
  module_pathname = '$libdir/pg_net'
  ```
  - No schema spec (implicit: public)
  - relocatable = false (worker hard-wired to single DB)
  - module_pathname standard libdir reference

- **SQL versioning**: `sql/pg_net.sql` + `pg_net--*--*.sql` upgrade scripts
  - 30+ upgrade scripts tracking 0.1 → 0.20.4 (lines visible in sql/ listing)
  - Pattern: `pg_net--0.19.6--0.19.7.sql` (minor bumps), `pg_net--0.18.0--0.19.0.sql` (feature)
  - Most bumps are ~28 bytes (mostly no-op or comment bumps)
  - Larger changes: e.g. 0.18.0→0.19.0 (4273 bytes) likely adds functions/tables

### Shared Memory & Startup Coordination

- **WorkerState struct**: Allocated in shmem via `ShmemInitStruct("pg_net worker state", ...)` at line 450
  - Fields: got_restart (atomic_u32), status (atomic_u32, values WS_NOT_YET/WS_RUNNING), should_wake (atomic_u32), shared_latch, epfd, curl_mhandle
  - Status polling: `wait_until_state()` uses ConditionVariable (line 72), not busy-wait
- **Hook pattern**: 
  - If PG15+: registers shmem_request_hook via `prev_shmem_request_hook` (line 487)
  - If PG14 or earlier: calls `RequestAddinShmemSpace()` directly (line 490)
  - Registers shmem_startup_hook (line 494) for initialization

### Request/Response Model & Lifecycle

- **Request queue**: Unlogged table `net.http_request_queue` (sql/pg_net.sql:12)
  - Columns: id, method, url, headers (jsonb), body (bytea), timeout_milliseconds
  - User inserts via `net.http_get()`, `net.http_post()`, etc. (wrapper functions)
- **Worker main loop** (lines 286–397):
  - Waits for wake at commit (XactCallback registered, line 127)
  - Consumes batch from queue (up to guc_batch_size, default 200)
  - Initializes curl handles, adds to curl_multi
  - Runs event loop: `wait_event(epfd, events, ...)` polls socket activity
  - On completion, inserts response to `net._http_response` table
- **Wake mechanism**: Transaction callback (line 94) registers on first insert; wakes at COMMIT via SetLatch (line 107)

### Signal Handling & Restart

- **SIGTERM**: handle_sigterm() (line 134) sets got_restart atomic, wakes latch
- **SIGHUP**: handle_sighup() (line 142) sets got_sighup flag, wakes latch
- **SIGUSR1**: handle_sigusr1() (line 149+) — handled explicitly (mentioned but code offset not in read range)
- **Restart function**: `worker_restart()` (line 63) calls pg_reload_conf(), writes got_restart flag
- **Exit handler**: on_proc_exit callback cleans up curl, closes epfd

### Traps & Design Notes

1. **Shared_preload_libraries enforcement**: Hard check in _PG_init (line 471). Cannot be loaded via CREATE EXTENSION in running database — must be in postgresql.conf and server restarted. Prevents accidental misconfiguration.

2. **GUC context PGC_SU_BACKEND on database/username**: Changes require superuser + server restart, not reload. Prevents security footgun of worker switching DBs on-the-fly without coordination.

3. **Unlogged tables for queue**: Both http_request_queue and _http_response are unlogged. Survives crash within session, but not across shutdown. Users must design for potential loss (documented in README).

4. **Batch size is count, not bytes**: Curl multi-handle scales with count. Batch of 200 is reasonable for 100ms+ timeouts; may cause memory spike if all responses are large.

5. **Curl event loop coordination**: Worker polls with epoll; adds socket FD wake to epfd; curl_multi_socket_action() drives state machine. No libuv; vanilla epoll/kqueue. Means tight coupling to curl's async API.

6. **Condition variable vs latch**: Uses both: ConditionVariable for shmem startup (line 77), SetLatch for wake-ups (line 107). ConditionVariable slower but works in shmem; latch is fast inter-process signal.

7. **No retry on worker crash**: bgw_restart_time = 1 second. If worker crashes, postmaster restarts after 1 sec. Queued requests will hang until restart (no automatic retry). User must implement timeout in app.

### File Pointers (C function signatures)

- `src/worker.c:466` — `void _PG_init(void)`
- `src/worker.c:249` — `void pg_net_worker(Datum main_arg)`
- `src/worker.c:477` — RegisterBackgroundWorker() call
- `src/worker.c:496–509` — DefineCustomString/Int calls (4 GUCs)
- `src/worker.c:63` — `Datum worker_restart(PG_FUNCTION_ARGS)`
- `src/worker.c:72` — `static void wait_until_state(WorkerState *ws, WorkerStatus expected_status)`
- `src/worker.c:94` — `static void wake_at_commit(XactEvent event, void *arg)`
- `src/worker.c:134` — `static void handle_sigterm(PG_SIGNAL_PARAMS)`
- `src/worker.c:142` — `static void handle_sighup(PG_SIGNAL_PARAMS)`
- `src/core.h:1` — WorkerState struct (not fully read, inferred from usage)
- `sql/pg_net.sql:12` — net.http_request_queue table definition
- `sql/pg_net.sql:35` — net._http_response table definition
- `pg_net.control.in` — Extension control file
