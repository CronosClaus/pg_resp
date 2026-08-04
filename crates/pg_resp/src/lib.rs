use pgrx::bgworkers::*;
use pgrx::callbacks::{register_xact_callback, PgXactCallbackEvent};
use pgrx::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

::pgrx::pg_module_magic!(name, version);

// Phase 0 spike S1 — socket lifecycle only. Hardcoded port, hardcoded +PONG\r\n
// reply to anything. No RESP parsing, no store, no GUCs: those are Phase 1.
//
// Bible §3.2 architecture: the registered bgworker's entry function is the ONLY
// thread allowed to touch PG FFI (BackgroundWorker::*, ereport, SPI, etc. — see
// docs/refs/pgrx-notes.md "Thread Safety & Restrictions" and
// docs/refs/postgres-notes.md §5's "Critical limitation for multi-threaded
// workers"). SetLatch()/wait_latch() only wake the PG-registered thread; a
// separate OS thread's own accept/read loop is never woken by SIGTERM. This
// spike proves out the simplest correct pattern: the server thread polls a
// shared AtomicBool on a short timeout instead of blocking indefinitely, so it
// notices shutdown within one poll interval. A self-pipe/eventfd wake (bible's
// production design for a real mio/epoll loop, sub-poll-interval latency) is a
// Phase 1 concern once there's a real event loop worth optimizing.
const SPIKE_PORT: &str = "127.0.0.1:6399";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[allow(non_snake_case)]
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
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

    log!("pg_resp S1 spike: starting, binding {SPIKE_PORT}");

    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);

    let listener = match TcpListener::bind(SPIKE_PORT) {
        Ok(l) => l,
        Err(e) => {
            log!("pg_resp S1 spike: bind failed: {e}, exiting");
            return;
        }
    };
    listener
        .set_nonblocking(true)
        .expect("failed to set listener non-blocking");

    let server_thread = std::thread::spawn(move || {
        server_loop(listener, server_shutdown);
    });

    // Main (registered) thread: PG lifecycle only, per the forbidden-list rule
    // in docs/refs/pgrx-notes.md. Never touches the socket directly.
    while BackgroundWorker::wait_latch(Some(Duration::from_secs(10))) {
        if BackgroundWorker::sighup_received() {
            log!("pg_resp S1 spike: SIGHUP received (no-op in this spike)");
        }
    }

    log!("pg_resp S1 spike: SIGTERM/shutdown latch fired, signaling server thread");
    shutdown.store(true, Ordering::SeqCst);
    server_thread.join().expect("server thread panicked");
    log!("pg_resp S1 spike: server thread joined, exiting cleanly");
}

fn server_loop(listener: TcpListener, shutdown: Arc<AtomicBool>) {
    let mut conns: Vec<std::net::TcpStream> = Vec::new();
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                stream
                    .set_nonblocking(true)
                    .expect("failed to set stream non-blocking");
                conns.push(stream);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                log!("pg_resp S1 spike: accept error: {e}");
            }
        }

        conns.retain_mut(|stream| {
            let mut buf = [0u8; 512];
            match stream.read(&mut buf) {
                Ok(0) => false, // peer closed
                Ok(_n) => {
                    // hardcoded reply per S1 spec: anything in -> +PONG\r\n out
                    let _ = stream.write_all(b"+PONG\r\n");
                    true
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(_) => false,
            }
        });

        std::thread::sleep(POLL_INTERVAL);
    }
}

// Phase 0 spike S2 — loopback RESP call + commit-vs-rollback xact callback.
// Bible §3.3: SQL-initiated mutations must queue transaction-locally and apply
// only on commit, never on abort. `register_xact_callback` fires exactly one
// of Commit/Abort per transaction (mutually exclusive, per pgrx/src/callbacks.rs)
// so registering only a Commit callback is sufficient to prove both halves: on
// commit the counter increments, on rollback the same call site never fires.
//
// These run inside a normal SQL backend process (a separate OS process per
// connection, not our bgworker's threads), so calling PG FFI / SPI here is
// unrelated to the bgworker main-thread-only rule above — completely different
// process context.
static SPIKE_COMMIT_COUNT: AtomicI64 = AtomicI64::new(0);

#[pg_extern]
fn resp_spike_queue_bump() {
    register_xact_callback(PgXactCallbackEvent::Commit, || {
        SPIKE_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    });
}

#[pg_extern]
fn resp_spike_counter() -> i64 {
    SPIKE_COMMIT_COUNT.load(Ordering::SeqCst)
}

/// Loopback RESP call per bible §3.3's SQL-surface design: connect to our own
/// bgworker's TCP port from a backend process and round-trip a command.
#[pg_extern]
fn resp_spike_loopback_ping() -> String {
    let mut stream = TcpStream::connect(SPIKE_PORT).expect("loopback connect failed");
    stream
        .write_all(b"PING\r\n")
        .expect("loopback write failed");
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).expect("loopback read failed");
    String::from_utf8_lossy(&buf[..n]).trim().to_string()
}
