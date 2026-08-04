use pgrx::bgworkers::*;
use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;
use resp_proto::{parse_command, ParseOutcome};
use resp_store::Store;
use std::ffi::CString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

mod dispatch;

::pgrx::pg_module_magic!(name, version);

// Phase 1: the real RESP2 event loop, replacing Phase 0's S1/S2 hardcoded
// spikes (proven and documented in reports/phase0.md and the pgrx-patterns
// skill). Architecture unchanged from the proven S1 pattern: the registered
// bgworker thread does PG lifecycle only; a spawned server thread owns the
// TCP listener, connections, and the resp-store `Store` (D4: single-threaded
// command execution, no locks on the hot path). Per pgrx-patterns skill
// §8.7 (Phase 0's kill-9 finding): dispatch must never panic — a panic here
// takes down the whole Postgres instance, not just this connection.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const READ_CHUNK: usize = 16 * 1024;

static BIND_ADDRESS: GucSetting<Option<CString>> =
    GucSetting::<Option<CString>>::new(Some(c"127.0.0.1"));
static PORT: GucSetting<i32> = GucSetting::<i32>::new(6379);

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

    let server_thread = std::thread::spawn(move || {
        server_loop(listener, server_shutdown);
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
    server_thread.join().expect("server thread panicked");
    log!("pg_resp: server thread joined, exiting cleanly");
}

struct Conn {
    stream: TcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

fn server_loop(listener: TcpListener, shutdown: Arc<AtomicBool>) {
    let mut store = Store::new();
    let mut conns: Vec<Conn> = Vec::new();

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                if stream.set_nonblocking(true).is_ok() {
                    conns.push(Conn {
                        stream,
                        read_buf: Vec::new(),
                        write_buf: Vec::new(),
                    });
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => log!("pg_resp: accept error: {e}"),
        }

        conns.retain_mut(|conn| service_connection(conn, &mut store));

        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Reads what's available, dispatches every complete command in the buffer,
/// writes replies. Returns false if the connection should be dropped
/// (peer closed, I/O error, or an unrecoverable protocol error).
fn service_connection(conn: &mut Conn, store: &mut Store) -> bool {
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
                    let reply = dispatch::dispatch(store, sys_now, mono_now, &args);
                    reply.write_to(&mut conn.write_buf);
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
        if conn.stream.write_all(&conn.write_buf).is_err() {
            return false;
        }
        conn.write_buf.clear();
    }

    true
}
