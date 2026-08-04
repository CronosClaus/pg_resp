# Omnigres omni_httpd: Socket Server in Extension + Bgworker Architecture

## Focus: Bgworker registration, thread model (master + HTTP worker threads), PG lifecycle/shutdown, event loop integration

Omnigres omni_httpd (C, sparse checkout to extensions/) runs an HTTP server *inside* a Postgres extension using master bgworker + worker threads. Precedent for: bgworker setup, shmem coordination, signal handling, async I/O integration.

### Bgworker Registration & Initialization

- **Entry point**: `_Omni_init(const omni_handle *handle)` called by omni framework (omnigres' module system) at startup
  - File: `omni_httpd.c:212` (lines 212–268)
  - Registers: `omni_guc_variable` for `omni_httpd.temp_dir` (boot value `/tmp`), `omni_httpd.http_workers` (default = sysconf(_SC_NPROCESSORS_ONLN), capped by max_worker_processes)
  - Allocates shmem for: control struct, reload semaphore, bgworker handle
- **Master worker registration**: `start_master_worker()` at line 170
  - Fills `BackgroundWorker` struct: bgw_name="omni_httpd", bgw_function_name="master_worker", bgw_restart_time=BGW_NEVER_RESTART
  - Flags: `BGWORKER_SHMEM_ACCESS | BGWORKER_BACKEND_DATABASE_CONNECTION`
  - Calls `handle->request_bgworker_start()` (omnigres framework wrapper) with timing=omni_timing_after_commit
  - Triggered on first database initialization (via `register_start_master_worker` shmem callback, line 203)

### Master Worker Function & Lifecycle

- **Entry point**: `master_worker(Datum db_oid)` at `master_worker.c:203`
  - Unblocks signals: SIGHUP, SIGTERM; registers handlers
  - Initializes connection: `BackgroundWorkerInitializeConnectionByOid(db_oid, InvalidOid, 0)`
  - Waits for stale workers to terminate (10-sec timeout, line 260)
  - Creates temp directory, sets up UNIX socket for worker communication
  - Initializes h2o event loop: `event_loop = h2o_new_evloop()`

### Thread Model: Master + HTTP Worker Threads

- **Master**: Single bgworker process, event loop runs in main thread (not spawned thread)
  - Uses h2o_evloop_t (h2o library's epoll/kqueue wrapper, not libuv)
  - Listens on TCP/UNIX socket, accepts connections
  - Communicates with HTTP worker bgworkers via UNIX socket (FD sharing)
- **HTTP workers**: Separate bgworker processes (spawned by postmaster), each has:
  - Postgres backend connection for SPI queries
  - h2o HTTP handler threads spawned by worker via pthread (separate from main bgworker thread)
  - Message queue: `h2o_multithread_queue_t` for cross-thread communication (handlers → event loop)
- **Message passing**: Send message struct → `h2o_multithread_send_message()` → event loop processes via receiver callback
  - Event loop thread does NOT call PG FFI; handlers (in bgworker main) do via SPI

### Event Loop & Socket Handling

- **File**: `event_loop.c:1–20` declares:
  - `h2o_evloop_t *worker_event_loop` (per HTTP worker)
  - `h2o_multithread_receiver_t event_loop_receiver` (for cross-thread queue)
  - Atomic flags: `worker_running`, `worker_reload`
- **H2O integration**: h2o_queue_send() (line 64) packs response buffers into message, sends to receiver
  - h2o does NOT link to Postgres; event loop runs pure C (no FFI during I/O)
- **Signal handling** in master: SIGHUP→reload config, SIGTERM→shutdown (http_worker.c + master_worker.c patterns)

### PG Lifecycle & Shutdown

- **Stop on deinit**: `_Omni_deinit()` at `omni_httpd.c:290`
  - Calls `stop_master_worker(false)` → requests termination with omni_timing_after_commit
  - Clears control->started flag
- **Graceful shutdown**: BGW_NEVER_RESTART + explicit termination request means: when extension drops, master worker does not restart
- **Worker attachment**: HTTP workers check for master attachment (master_worker.c:237–251). If master is gone, workers terminate (prevents orphan workers using postmaster slots)
- **Signals**: SIGTERM wakes worker via latch + sets shutdown flag; worker loop checks flag and exits

### GUCs & Configuration

- `omni_httpd.temp_dir` (PGC_SIGHUP, string) — validated via check_temp_dir() callback; must exist, not in data dir
- `omni_httpd.http_workers` (PGC_SIGHUP, int) — defaults to CPU count, min 1
- Both via omni framework's `handle->declare_guc_variable()` (not standard DefineCustom*)
- GUC context PGC_SIGHUP allows config reload without restart

### Traps & Design Notes

1. **No PG FFI in event loop thread**: h2o event loop runs in bgworker main thread (NOT spawned pthread). HTTP handler threads exist but do not touch Postgres directly. Master thread handles latch wakeups, signal dispatch, SPI calls. Design prevents FFI reentrancy.
2. **Shared memory synchronization**: Uses LWLock (control->lock) to guard bgworker start/stop (lines 178, 287). Template database check prevents startup in system DBs.
3. **Worker attachment semantics**: HTTP workers poll ProcArray looking for master with matching type "omni_httpd" (master_worker.c:241). If not found within 10 sec, proceeds anyway. Edge case: if master crashes and is restarted, old workers may hang.
4. **UNIX socket for FD passing**: Master creates socket, binds to temp dir path. Workers connect and receive listening socket FD via `recvmsg()`. Allows hot reload without reconnecting clients (FD passed to new worker).
5. **Config reload coordination**: Reload semaphore (`omni_atomic_uint32`) allows workers to detect reload requests. Workers watch semaphore and reinitialize routes.
6. **HTTP/2 support**: h2o library includes http/2.h, h2o_websocket.h. omni_httpd advertises HTTP/1.1, HTTP/2, WebSocket capability.

### File Pointers (C function signatures)

- `omni_httpd.c:212` — `void _Omni_init(const omni_handle *handle)`
- `omni_httpd.c:290` — `void _Omni_deinit(const omni_handle *handle)`
- `omni_httpd.c:170` — `static void start_master_worker(const omni_handle *handle, omni_bgworker_handle *bgw_handle, omni_timing timing)`
- `omni_httpd.c:181` — BackgroundWorker struct initialization
- `master_worker.c:203` — `void master_worker(Datum db_oid)`
- `event_loop.c:13` — `h2o_evloop_t *worker_event_loop`
- `event_loop.c:64` — `static void h2o_queue_send(request_message_t *msg, h2o_iovec_t *bufs, size_t bufcnt, h2o_send_state_t state)`
- `http_worker.c:1–70` — HTTP worker file header, signal handlers, handler queue setup
- `master_worker.c:230–275` — Worker attachment polling (ProcArray inspection)
