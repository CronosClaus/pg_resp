use mio::net::{TcpListener as MioTcpListener, TcpStream as MioTcpStream};
use mio::{Events, Interest, Poll, Token};
use pgrx::atomics::PgAtomic;
use pgrx::bgworkers::*;
use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;
use resp_proto::{parse_command, ParseOutcome};
use resp_store::Store;
use std::collections::HashMap;
use std::ffi::CString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

mod dispatch;
mod glob;
mod sql;

::pgrx::pg_module_magic!(name, version);

// Phase 1: the real RESP2 event loop, replacing Phase 0's S1/S2 hardcoded
// spikes (proven and documented in reports/phase0.md and the pgrx-patterns
// skill). Architecture unchanged from the proven S1 pattern: the registered
// bgworker thread does PG lifecycle only; a spawned server thread owns the
// TCP listener, connections, and the resp-store `Store` (D4: single-threaded
// command execution, no locks on the hot path). Per pgrx-patterns skill
// §8.8: without the per-connection panic fence below, a panic in one
// connection's dispatch kills the entire server thread — every connection,
// every future connection — silently, while Postgres itself stays healthy.
//
// Uses mio (bible §3.2 names it explicitly) for readiness-based I/O rather
// than S1's fixed-interval sleep-and-poll: the first working version of
// this loop reused S1's 20ms-sleep pattern directly and measured a ~20ms
// GET p50 (133x over the 150µs gate) — the sleep interval was adding its
// full duration to every request's latency, not just shutdown latency, since
// the thread was never blocked *on the sockets themselves*. `Poll::poll`'s
// timeout only bounds how long the loop can go with zero I/O activity before
// re-checking the shutdown flag; real traffic wakes it immediately.
const SHUTDOWN_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const LISTENER_TOKEN: Token = Token(0);
const READ_CHUNK: usize = 16 * 1024;

/// How often the main thread wakes to check the server thread's health (S6).
/// Also bounds how long a wedged service can go unnoticed.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(3);

/// Consecutive failed probes before the watchdog declares the service dead.
///
/// Not 1: a single probe can fail for reasons that are not "the service is
/// gone" — a transient EMFILE, a full accept backlog, or the loop being busy
/// with a long command for longer than the probe timeout. Three strikes at
/// `WATCHDOG_INTERVAL` means ~9s of continuous failure before we take the
/// process down, which is short enough to matter and long enough not to
/// restart a healthy worker over a hiccup.
const WATCHDOG_STRIKES: u32 = 3;

/// Per-connection cap on buffered-but-unsent reply bytes.
///
/// A client that pipelines aggressively and reads slowly makes the server
/// accumulate replies it cannot flush. Without a cap that is an unbounded
/// allocation driven entirely by the peer — the same hazard Redis addresses
/// with `client-output-buffer-limit`. Exceeding it drops that one connection;
/// every other connection is unaffected.
const MAX_PENDING_WRITE_BYTES: usize = 64 * 1024 * 1024;

/// Set by the `DEBUG PANIC-TOPLEVEL` command (only compiled with the
/// `debug_panic` feature) to make the server loop panic *outside* the
/// per-connection fence, which is the only way to exercise S6's top-level
/// policy on purpose. Never present in a normal build.
#[cfg(feature = "debug_panic")]
pub(crate) static DEBUG_PANIC_TOPLEVEL: AtomicBool = AtomicBool::new(false);

static BIND_ADDRESS: GucSetting<Option<CString>> =
    GucSetting::<Option<CString>>::new(Some(c"127.0.0.1"));
static PORT: GucSetting<i32> = GucSetting::<i32>::new(6379);
static MAX_MEMORY_MB: GucSetting<i32> = GucSetting::<i32>::new(256); // bible §3.5 default
static EVICTION: GucSetting<Option<CString>> =
    GucSetting::<Option<CString>>::new(Some(c"clock_lru"));
static PASSWORD: GucSetting<Option<CString>> = GucSetting::<Option<CString>>::new(None);

/// Count of post-commit cache invalidations that could not be delivered
/// (`D13`).
///
/// This is the one piece of pg_resp state that lives in **Postgres shared
/// memory**, and it has to, for a reason that is not a preference: the counter
/// is incremented by *backends* at commit time, precisely when the bgworker's
/// store was unreachable. Keeping it in the store would mean the number can
/// only be recorded when the thing that failed is working. Keeping it in a
/// backend-local static would mean it dies with the session. Shared memory is
/// the only place both the incrementing backends and the reporting server
/// thread can see it.
///
/// `D2` ("v0.1 store = bgworker-local heap, not PG shared memory") is not
/// violated: the *store* stays local. This is a single `u64`, which is what
/// pgrx's shmem support is genuinely safe at.
///
/// The server thread reads it through a plain `&'static AtomicU64` captured on
/// the main thread before the thread is spawned (see
/// `background_worker_main`) — an atomic load through an inherited shmem
/// pointer, not a PG function call, so iron rule 1 holds.
static INVALIDATIONS_LOST: PgAtomic<AtomicU64> =
    unsafe { PgAtomic::new(c"pg_resp_invalidations_lost") };

/// Where a *backend* should connect to reach the RESP server, and with what
/// password.
///
/// Callable only from a backend's or the bgworker's main thread —
/// `GucSetting::get()` enforces that at runtime (pgrx-patterns §4).
///
/// `bind_address` is the server's *listen* address, which is not always a
/// usable *connect* address: `0.0.0.0` and `::` mean "every interface" to
/// `bind()` and are useless to `connect()`. Loopback is the right target in
/// both of those cases, and is also the correct choice on principle — a
/// backend and the bgworker are always on the same host, so cache traffic
/// should never leave it even when the listener is deliberately exposed.
pub(crate) fn loopback_target() -> (String, Option<Vec<u8>>) {
    let bind = BIND_ADDRESS
        .get()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let host = match bind.as_str() {
        "0.0.0.0" | "" => "127.0.0.1".to_string(),
        "::" | "[::]" | "::0" => "::1".to_string(),
        other => other.to_string(),
    };
    let port = PORT.get();
    let addr = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let password = PASSWORD
        .get()
        .map(|c| c.to_bytes().to_vec())
        .filter(|p| !p.is_empty());
    (addr, password)
}

/// `log!()` (and any other pgrx/PG-FFI-backed macro) is forbidden off the
/// main bgworker thread — pgrx enforces this at *runtime* and panics on
/// violation, it doesn't just silently misbehave. Found the hard way: the
/// panic-fence handlers below originally used `log!()` inside their
/// `catch_unwind` arms, on this (server) thread — pgrx's own guard then
/// panicked on *that* call ("postgres FFI may not be called from multiple
/// threads"), a second, unfenced panic that escaped straight past both
/// fences and killed the thread anyway, defeating the whole point. Plain
/// `eprintln!` is safe from any thread, and PG's log collector captures a
/// bgworker's stderr into the same server log anyway, so nothing is lost.
macro_rules! server_log {
    ($($arg:tt)*) => {
        eprintln!("pg_resp[server thread]: {}", format!($($arg)*))
    };
}

#[allow(non_snake_case)]
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    GucRegistry::define_string_guc(
        c"pg_resp.bind_address",
        c"Address the RESP server listens on.",
        c"Widening this beyond 127.0.0.1 exposes the cache over the network; \
          see docs/ops.md before doing so. Requires a restart to take effect.",
        &BIND_ADDRESS,
        GucContext::Postmaster,
        GucFlags::empty(),
    );
    GucRegistry::define_int_guc(
        c"pg_resp.port",
        c"TCP port the RESP server listens on.",
        c"Defaults to Redis's own default (6379) since pg_resp is meant as a \
          drop-in replacement. Requires a restart to take effect.",
        &PORT,
        1,
        65535,
        GucContext::Postmaster,
        GucFlags::empty(),
    );
    GucRegistry::define_int_guc(
        c"pg_resp.max_memory",
        c"Approximate byte budget for the cache (key+value bytes plus a fixed per-entry overhead estimate).",
        c"0 means unbounded (no eviction). Accounting and CLOCK-LRU eviction are \
          implemented in crates/resp-store; the real per-entry overhead constant \
          is not yet measured (see reports/phase2.md) — treat this as approximate. \
          Requires a restart to take effect (read once at bgworker startup, not \
          re-read live — see pgrx-patterns skill's GucSetting::get() trap).",
        &MAX_MEMORY_MB,
        0,
        i32::MAX,
        GucContext::Postmaster,
        GucFlags::UNIT_MB,
    );
    GucRegistry::define_string_guc(
        c"pg_resp.eviction",
        c"Eviction policy: 'clock_lru' or 'noeviction'.",
        c"'clock_lru' (default) evicts approximately-least-recently-used entries \
          when pg_resp.max_memory is exceeded. 'noeviction' disables the byte \
          budget entirely in this version (v0.1 does not yet implement \
          reject-writes-when-full semantics — see docs/ops.md). Requires a \
          restart to take effect.",
        &EVICTION,
        GucContext::Postmaster,
        GucFlags::empty(),
    );
    GucRegistry::define_string_guc(
        c"pg_resp.password",
        c"Password required via the RESP AUTH command. Empty (default) means no authentication required.",
        c"Constant-time compared (bible §3.6). Read once at bgworker startup \
          (requires a restart to change, like every other GUC here — see \
          pgrx-patterns skill's GucSetting::get() main-thread-only trap). No \
          username support: single-password model only, matching bible §3.6's \
          single-tenant design.",
        &PASSWORD,
        GucContext::Postmaster,
        GucFlags::empty(),
    );

    // Must happen in _PG_init (which only runs at all because pg_resp is in
    // shared_preload_libraries) — the shmem request hook it installs is only
    // consulted during postmaster startup.
    pgrx::pg_shmem_init!(INVALIDATIONS_LOST);

    BackgroundWorkerBuilder::new("pg_resp")
        .set_function("background_worker_main")
        .set_library("pg_resp")
        .set_restart_time(Some(Duration::from_secs(1)))
        .load();
}

#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn background_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);

    let bind_address = BIND_ADDRESS
        .get()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = PORT.get();
    let addr = format!("{bind_address}:{port}");

    // All GUC reads happen here, on the main (registered) thread, and only
    // here — GucSetting::get() has the exact same main-thread-only runtime
    // check as log!() (pgrx-patterns skill's trap for both), so none of
    // these can be read again from the spawned server thread. Every GUC
    // here is therefore effectively read-once-at-startup in this
    // architecture regardless of its declared context; all are registered
    // `Postmaster` (requires a restart) to be honest about that.
    let max_memory_bytes: Option<usize> = {
        let mb = MAX_MEMORY_MB.get();
        if mb <= 0 {
            None
        } else {
            Some(mb as usize * 1024 * 1024)
        }
    };
    let eviction = EVICTION
        .get()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clock_lru".to_string());
    // 'noeviction' in v0.1: no reject-writes-when-full semantics yet (see
    // docs/ops.md) — simplest honest behavior is "no budget enforced at
    // all", not silently falling back to clock_lru's cap.
    let max_memory_bytes = if eviction == "noeviction" {
        None
    } else {
        max_memory_bytes
    };
    let password: Option<Vec<u8>> = PASSWORD
        .get()
        .map(|c| c.to_bytes().to_vec())
        .filter(|p| !p.is_empty());

    // Resolve the shmem counter to a plain reference here, on the main thread,
    // and hand that to the server thread. `&'static AtomicU64` is Send + Sync,
    // and loading it is an atomic read of inherited shared memory rather than
    // a PG call — so the server thread can report `invalidations_lost` in INFO
    // without ever touching pgrx (iron rule 1). Resolving it *here* also
    // sidesteps reading `PgAtomic`'s own `UnsafeCell` pointer from two threads.
    let invalidations_lost: &'static AtomicU64 = INVALIDATIONS_LOST.get();

    log!("pg_resp: starting, binding {addr}");

    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);

    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            log!("pg_resp: bind failed on {addr}: {e}, exiting");
            return;
        }
    };
    listener
        .set_nonblocking(true)
        .expect("failed to set listener non-blocking");

    // Panic fence (outer, defense-in-depth): std::thread already catches an
    // unwinding panic at the OS-thread boundary — the process doesn't crash
    // — but empirically (see pgrx-patterns skill §8.8 / reports/phase2.md)
    // that alone means the *entire* server thread dies silently: every
    // connection it owned resets, no new connection is ever accepted again,
    // yet the bgworker process keeps running and Postgres itself stays
    // completely healthy. That combination — RESP service permanently and
    // silently dead, everything else reporting green — is worse than a
    // loud crash. This explicit catch_unwind is belt-and-suspenders around
    // the real fix (the per-connection fence in `server_loop` below, which
    // should make reaching this outer boundary essentially unreachable in
    // practice); it exists so a panic anywhere else in the loop (e.g. in
    // `accept()` handling) is at least logged loudly instead of vanishing.
    let server_died = Arc::new(AtomicBool::new(false));
    let server_died_flag = Arc::clone(&server_died);
    let server_thread = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            server_loop(
                listener,
                server_shutdown,
                max_memory_bytes,
                password,
                invalidations_lost,
            );
        }));
        if let Err(payload) = result {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            server_log!(
                "FATAL — top-level loop panicked despite per-connection fencing: {msg}. \
                 Flagging the worker for restart."
            );
        }
        // S6: whether the loop panicked or merely returned, reaching this point
        // without a requested shutdown means the RESP service is gone. Flag it;
        // the main thread turns that into a process exit.
        //
        // This replaces catch-and-linger, which was the worst of both worlds:
        // the process stayed healthy, `SELECT 1` kept working, the worker was
        // still listed as running — and no connection would ever be served
        // again, with nothing anywhere reporting a problem. Exiting is louder
        // and, crucially, self-healing: bgw_restart_time brings up a fresh
        // worker with a working listener.
        server_died_flag.store(true, Ordering::SeqCst);
    });

    // Main (registered) thread: PG lifecycle plus the S6 watchdog. It never
    // touches the socket or the store directly (pgrx-patterns skill §3/§7) —
    // the watchdog reaches the server the same way any client would, over TCP,
    // which is exactly why it can tell that the service is *answering* rather
    // than merely that a thread exists.
    //
    // resp-client has no pgrx dependency so calling it here is fine; and this
    // thread may call PG FFI, so `log!`/`ereport!` work here (they do not on
    // the server thread).
    let (probe_addr, probe_password) = loopback_target();
    let mut probe = resp_client::Client::new(probe_addr.clone(), probe_password)
        .with_timeout(WATCHDOG_INTERVAL);
    let mut strikes: u32 = 0;

    while BackgroundWorker::wait_latch(Some(WATCHDOG_INTERVAL)) {
        if BackgroundWorker::sighup_received() {
            log!("pg_resp: SIGHUP received (config reload not yet implemented; GUCs here require a restart)");
        }

        // 1. Did the server thread die outright?
        if server_died.load(Ordering::SeqCst) {
            log!("pg_resp: server thread is gone; exiting so the postmaster restarts this worker");
            fatal_restart("pg_resp server thread exited unexpectedly");
        }

        // 2. Is it still answering? A thread can be alive and wedged — that is
        //    the silent-zombie state this exists to catch, and the one a mere
        //    "is the process running" check reports as healthy.
        match probe.probe() {
            Ok(()) => strikes = 0,
            Err(e) => {
                strikes += 1;
                log!(
                    "pg_resp: watchdog probe of {probe_addr} failed ({e}); strike {strikes} of {WATCHDOG_STRIKES}"
                );
                // Force a fresh connection next time: a cached socket that has
                // gone bad would keep failing for reasons of its own.
                probe.disconnect();
                if strikes >= WATCHDOG_STRIKES {
                    fatal_restart("pg_resp RESP service stopped answering");
                }
            }
        }
    }

    log!("pg_resp: SIGTERM/shutdown latch fired, signaling server thread");
    shutdown.store(true, Ordering::SeqCst);
    // Use match, not .expect(): if the server thread already died (panic
    // caught above), .join() returns Err, and .expect() would itself panic
    // here — on the PG-registered main thread this time, during shutdown.
    if server_thread.join().is_err() {
        log!("pg_resp: server thread had already exited abnormally; shutdown continues anyway");
    }
    log!("pg_resp: exiting cleanly");
}

/// Exit the worker so the postmaster restarts it (S6).
///
/// `ereport!(FATAL)` rather than `std::process::exit`, for two reasons. It is
/// the PG-native way to end a worker: it logs at FATAL through the normal
/// channel and then calls `proc_exit(1)`, running Postgres's own shmem-detach
/// and cleanup instead of abandoning them. And it keeps every PG FFI call on
/// the registered main thread where iron rule 1 requires it — which is exactly
/// why the server thread flags the condition rather than exiting by itself.
///
/// Exit status 1 is what Postgres reads as "child exited on a FATAL error": the
/// worker is restarted after `bgw_restart_time` *without* the cluster-wide
/// crash-recovery cycle that an abnormal (signal, or any other status) death
/// triggers. Verified empirically rather than assumed — see
/// `tests/lifecycle/README.md`.
fn fatal_restart(reason: &str) -> ! {
    ereport!(
        FATAL,
        PgSqlErrorCode::ERRCODE_INTERNAL_ERROR,
        format!("{reason}; restarting the pg_resp background worker")
    );
}

struct Conn {
    stream: MioTcpStream,
    read_buf: Vec<u8>,
    /// Replies produced but not yet fully written to the socket.
    write_buf: Vec<u8>,
    /// How many bytes of `write_buf` have already reached the kernel. Tracked
    /// as a cursor rather than by draining the front of the buffer, so a
    /// partial write costs nothing extra.
    write_pos: usize,
    /// Whether this connection is currently registered for WRITABLE as well as
    /// READABLE. Re-registering on every event would be a wasted syscall, so
    /// the current state is remembered.
    want_write: bool,
    /// Set by QUIT (or a protocol error): close as soon as the pending reply
    /// has been flushed, never before, or the client loses the answer it is
    /// still waiting for.
    close_after_flush: bool,
    conn_state: dispatch::ConnState,
}

/// Outcome of trying to push `write_buf` to the socket.
enum FlushOutcome {
    /// Everything buffered has reached the kernel.
    Drained,
    /// The socket is full; the rest is still buffered and WRITABLE interest is
    /// needed to finish.
    Pending,
    /// Unrecoverable; drop the connection.
    Failed,
}

/// Write as much of `conn.write_buf` as the socket will take.
///
/// This is the fix for a real bug, not a hypothetical one: the previous code
/// called `write_all` on a **non-blocking** socket. `write_all` treats
/// `WouldBlock` as a hard error and returns, so the moment a reply did not fit
/// in the socket buffer the remaining bytes were silently dropped and the
/// client was left with a truncated reply on a desynchronized stream. It never
/// showed up in Phase 1/2 testing because localhost socket buffers swallow
/// small replies whole — it needs a big reply or a slow reader, which is
/// exactly what bible §10's 16KB-value / pipeline-16 benchmark arms produce.
fn flush_writes(conn: &mut Conn) -> FlushOutcome {
    while conn.write_pos < conn.write_buf.len() {
        match conn.stream.write(&conn.write_buf[conn.write_pos..]) {
            Ok(0) => return FlushOutcome::Failed,
            Ok(n) => conn.write_pos += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                return FlushOutcome::Pending
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return FlushOutcome::Failed,
        }
    }
    conn.write_buf.clear();
    conn.write_pos = 0;
    FlushOutcome::Drained
}

/// How often the server thread runs an active TTL sweep (bible §3.5: "active
/// sweep from the event loop timer... so memory is actually reclaimed
/// without a read"), piggybacked on the same poll-timeout tick that already
/// exists for the shutdown check — no extra timer needed.
const ACTIVE_EXPIRE_SAMPLE_SIZE: usize = 20;

fn server_loop(
    listener: TcpListener,
    shutdown: Arc<AtomicBool>,
    max_memory_bytes: Option<usize>,
    password: Option<Vec<u8>>,
    invalidations_lost: &'static AtomicU64,
) {
    let mut store = match max_memory_bytes {
        Some(bytes) => Store::with_max_memory(bytes),
        None => Store::new(),
    };
    let mut mio_listener = MioTcpListener::from_std(listener);

    let mut poll = match Poll::new() {
        Ok(p) => p,
        Err(e) => {
            server_log!("mio::Poll::new failed: {e}, exiting server thread");
            return;
        }
    };
    if let Err(e) = poll
        .registry()
        .register(&mut mio_listener, LISTENER_TOKEN, Interest::READABLE)
    {
        server_log!("failed to register listener with poll: {e}, exiting server thread");
        return;
    }

    let mut events = Events::with_capacity(1024);
    let mut conns: HashMap<Token, Conn> = HashMap::new();
    let mut next_token: usize = 1; // Token(0) is reserved for the listener

    while !shutdown.load(Ordering::SeqCst) {
        if let Err(e) = poll.poll(&mut events, Some(SHUTDOWN_CHECK_INTERVAL)) {
            if e.kind() != std::io::ErrorKind::Interrupted {
                server_log!("poll error: {e}");
            }
            continue;
        }

        // Deliberate top-level panic for S6 testing. Checked here, outside the
        // per-connection fence, because that fence is what stops an ordinary
        // dispatch panic from reaching the top level — so exercising the
        // top-level path needs a panic raised from the loop body itself.
        #[cfg(feature = "debug_panic")]
        if DEBUG_PANIC_TOPLEVEL.swap(false, Ordering::SeqCst) {
            panic!("deliberate top-level panic (debug_panic feature)");
        }

        // Active TTL sweep (bible §3.5), piggybacked on this same poll tick
        // — runs whether this iteration was woken by real I/O or just the
        // timeout, so it still happens on an idle server.
        store.active_expire_sweep(Instant::now(), ACTIVE_EXPIRE_SAMPLE_SIZE);

        for event in events.iter() {
            if event.token() == LISTENER_TOKEN {
                loop {
                    match mio_listener.accept() {
                        Ok((mut stream, _addr)) => {
                            let token = Token(next_token);
                            next_token += 1;
                            if poll
                                .registry()
                                .register(&mut stream, token, Interest::READABLE)
                                .is_ok()
                            {
                                conns.insert(
                                    token,
                                    Conn {
                                        stream,
                                        read_buf: Vec::new(),
                                        write_buf: Vec::new(),
                                        write_pos: 0,
                                        want_write: false,
                                        close_after_flush: false,
                                        conn_state: dispatch::ConnState::default(),
                                    },
                                );
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            server_log!("accept error: {e}");
                            break;
                        }
                    }
                }
            } else {
                let token = event.token();
                // A WRITABLE-only wakeup means "the socket has room again" —
                // finish the pending reply, and do not try to read.
                if event.is_writable() && !event.is_readable() {
                    let keep = match conns.get_mut(&token) {
                        Some(conn) => match flush_writes(conn) {
                            FlushOutcome::Drained => !conn.close_after_flush,
                            FlushOutcome::Pending => true,
                            FlushOutcome::Failed => false,
                        },
                        None => false,
                    };
                    if keep {
                        if let Some(conn) = conns.get_mut(&token) {
                            reconcile_interest(&poll, token, conn);
                        }
                    } else if let Some(mut conn) = conns.remove(&token) {
                        let _ = poll.registry().deregister(&mut conn.stream);
                    }
                    continue;
                }
                // Primary panic fence (pgrx-patterns skill §8.8): D4's
                // single-threaded model means this one thread serves every
                // connection. Without this, a panic dispatching one bad
                // command on one connection unwinds straight out of the
                // thread's entry point and kills the *entire* server thread
                // — every other connection reset, no new connection ever
                // accepted again, silently, while Postgres itself and even
                // the bgworker process stay completely healthy (confirmed
                // by deliberately triggering one). Catching it here instead
                // means only this one connection is lost; the thread, the
                // listener, and every other connection keep running.
                let keep = match conns.get_mut(&token) {
                    Some(conn) => {
                        let password_ref = password.as_deref();
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            service_connection(conn, &mut store, password_ref, invalidations_lost)
                        })) {
                            Ok(keep) => keep,
                            Err(_) => {
                                server_log!(
                                    "connection handler panicked; dropping only \
                                     this connection, server thread continues"
                                );
                                false
                            }
                        }
                    }
                    None => false,
                };
                if keep {
                    // Registering WRITABLE only while a reply is actually
                    // pending keeps an idle connection from waking the loop on
                    // every spurious writability notification.
                    if let Some(conn) = conns.get_mut(&token) {
                        reconcile_interest(&poll, token, conn);
                    }
                } else if let Some(mut conn) = conns.remove(&token) {
                    let _ = poll.registry().deregister(&mut conn.stream);
                }
            }
        }
    }
}

/// Reads what's available, dispatches every complete command in the buffer,
/// writes replies. Returns false if the connection should be dropped
/// (peer closed, I/O error, QUIT, or an unrecoverable protocol error).
fn service_connection(
    conn: &mut Conn,
    store: &mut Store,
    password: Option<&[u8]>,
    invalidations_lost: &AtomicU64,
) -> bool {
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        match conn.stream.read(&mut chunk) {
            Ok(0) => return false, // peer closed
            Ok(n) => conn.read_buf.extend_from_slice(&chunk[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => return false,
        }
    }

    loop {
        match parse_command(&conn.read_buf) {
            ParseOutcome::Complete { args, consumed } => {
                conn.read_buf.drain(..consumed);
                if !args.is_empty() {
                    let sys_now = SystemTime::now();
                    let mono_now = Instant::now();
                    let is_quit = args[0].eq_ignore_ascii_case(b"QUIT");
                    let reply = dispatch::dispatch(
                        store,
                        sys_now,
                        mono_now,
                        &args,
                        &mut conn.conn_state,
                        password,
                        invalidations_lost.load(Ordering::Relaxed),
                    );
                    reply.write_to(&mut conn.write_buf);
                    if is_quit {
                        // Close only once the +OK has actually been flushed —
                        // see close_after_flush.
                        conn.close_after_flush = true;
                        break; // stop processing any further pipelined commands
                    }
                }
                // empty args (blank inline / *0): no-op, no reply, keep going
            }
            ParseOutcome::Incomplete => break,
            ParseOutcome::Invalid(msg) => {
                let reply = resp_proto::Reply::error(format!("ERR Protocol error: {msg}"));
                reply.write_to(&mut conn.write_buf);
                // Desynced parser: close per the resp-protocol skill — but
                // still give the error reply its best chance to reach the
                // client first, and let the WRITABLE path finish the job if it
                // does not fit right now.
                conn.close_after_flush = true;
                break;
            }
        }
    }

    // A peer that pipelines faster than it reads would otherwise make this
    // buffer grow without bound.
    if conn.write_buf.len() - conn.write_pos > MAX_PENDING_WRITE_BYTES {
        server_log!(
            "dropping a connection with {} bytes of unflushed replies (exceeds \
             MAX_PENDING_WRITE_BYTES); the client is not reading its results",
            conn.write_buf.len() - conn.write_pos
        );
        return false;
    }

    match flush_writes(conn) {
        FlushOutcome::Drained => !conn.close_after_flush,
        // Still owed bytes: keep the connection, and the caller will register
        // WRITABLE so the rest goes out when the socket drains. This is the
        // whole point of the fix — the loop must never sit and spin on one
        // slow reader, and must never abandon the tail of a reply.
        FlushOutcome::Pending => true,
        FlushOutcome::Failed => false,
    }
}

/// Keep a connection's poll registration in step with whether it owes bytes.
fn reconcile_interest(poll: &Poll, token: Token, conn: &mut Conn) {
    let needs_write = conn.write_pos < conn.write_buf.len();
    if needs_write == conn.want_write {
        return;
    }
    let interest = if needs_write {
        Interest::READABLE | Interest::WRITABLE
    } else {
        Interest::READABLE
    };
    if poll
        .registry()
        .reregister(&mut conn.stream, token, interest)
        .is_ok()
    {
        conn.want_write = needs_write;
    }
}
