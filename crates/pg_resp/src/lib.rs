use mio::net::{TcpListener as MioTcpListener, TcpStream as MioTcpStream};
use mio::{Events, Interest, Poll, Token};
use pgrx::bgworkers::*;
use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;
use resp_proto::{parse_command, ParseOutcome};
use resp_store::Store;
use std::collections::HashMap;
use std::ffi::CString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

mod dispatch;
mod glob;

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

static BIND_ADDRESS: GucSetting<Option<CString>> =
    GucSetting::<Option<CString>>::new(Some(c"127.0.0.1"));
static PORT: GucSetting<i32> = GucSetting::<i32>::new(6379);
static MAX_MEMORY_MB: GucSetting<i32> = GucSetting::<i32>::new(256); // bible §3.5 default
static EVICTION: GucSetting<Option<CString>> = GucSetting::<Option<CString>>::new(Some(c"clock_lru"));
static PASSWORD: GucSetting<Option<CString>> = GucSetting::<Option<CString>>::new(None);

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
    let password: Option<Vec<u8>> = PASSWORD.get().map(|c| c.to_bytes().to_vec()).filter(|p| !p.is_empty());

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
    let server_thread = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            server_loop(listener, server_shutdown, max_memory_bytes, password);
        }));
        if let Err(payload) = result {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            server_log!(
                "FATAL — top-level loop panicked despite per-connection fencing: {msg}. \
                 The RESP service is now dead until the next bgworker restart; \
                 Postgres itself is unaffected."
            );
        }
    });

    // Main (registered) thread: PG lifecycle only — never touches the socket
    // or the store directly (pgrx-patterns skill §3/§7).
    while BackgroundWorker::wait_latch(Some(Duration::from_secs(10))) {
        if BackgroundWorker::sighup_received() {
            log!("pg_resp: SIGHUP received (config reload not yet implemented; GUCs here require a restart)");
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

struct Conn {
    stream: MioTcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    conn_state: dispatch::ConnState,
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
    if let Err(e) =
        poll.registry()
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
                            service_connection(conn, &mut store, password_ref)
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
                if !keep {
                    if let Some(mut conn) = conns.remove(&token) {
                        let _ = poll.registry().deregister(&mut conn.stream);
                    }
                }
            }
        }
    }
}

/// Reads what's available, dispatches every complete command in the buffer,
/// writes replies. Returns false if the connection should be dropped
/// (peer closed, I/O error, QUIT, or an unrecoverable protocol error).
fn service_connection(conn: &mut Conn, store: &mut Store, password: Option<&[u8]>) -> bool {
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        match conn.stream.read(&mut chunk) {
            Ok(0) => return false, // peer closed
            Ok(n) => conn.read_buf.extend_from_slice(&chunk[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => return false,
        }
    }

    let mut should_close = false;
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
                    );
                    reply.write_to(&mut conn.write_buf);
                    if is_quit {
                        should_close = true;
                        break; // stop processing any further pipelined commands
                    }
                }
                // empty args (blank inline / *0): no-op, no reply, keep going
            }
            ParseOutcome::Incomplete => break,
            ParseOutcome::Invalid(msg) => {
                let reply = resp_proto::Reply::error(format!("ERR Protocol error: {msg}"));
                reply.write_to(&mut conn.write_buf);
                let _ = conn.stream.write_all(&conn.write_buf);
                return false; // desynced parser: close per resp-protocol skill's guidance
            }
        }
    }

    if !conn.write_buf.is_empty() {
        let write_ok = conn.stream.write_all(&conn.write_buf).is_ok();
        conn.write_buf.clear();
        if !write_ok {
            return false;
        }
    }

    !should_close
}
