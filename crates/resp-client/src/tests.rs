//! Tests run against a real `TcpListener` on 127.0.0.1:0 — a scripted mini
//! server, not a mock. That is the point of `D11`: every failure mode below
//! (dead reused connection, reply arriving one byte at a time, a peer that
//! accepts and then says nothing) is a real socket doing the real thing, in
//! milliseconds, with no Postgres anywhere near it.

use super::*;
use resp_proto::{parse_command, ParseOutcome};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

/// What a scripted server should do after reading one command.
enum Act {
    /// Write these exact bytes back.
    Reply(&'static [u8]),
    /// Write these bytes one at a time, flushing between each — simulates a
    /// reply that arrives across several `read()` calls.
    Dribble(&'static [u8]),
    /// Close the connection without answering.
    Close,
    /// Read the command, answer nothing, keep the connection open.
    Hang,
}

/// A scripted server. Serves connections sequentially; for each connection it
/// walks its own copy of the script, one entry per command received.
struct Server {
    addr: String,
    /// Commands the server actually received, in order, across all
    /// connections — lets a test assert on protocol-level facts like "AUTH was
    /// the first thing sent".
    seen: mpsc::Receiver<Vec<Vec<u8>>>,
    _handle: thread::JoinHandle<()>,
}

fn spawn_server(script: Vec<Vec<Act>>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let (tx, seen) = mpsc::channel();
    let handle = thread::spawn(move || {
        for (conn_idx, acts) in script.into_iter().enumerate() {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = conn_idx;
            let mut buf: Vec<u8> = Vec::new();
            for act in acts {
                // Read until one complete command is buffered.
                let args = loop {
                    match parse_command(&buf) {
                        ParseOutcome::Complete { args, consumed } => {
                            buf.drain(..consumed);
                            break args;
                        }
                        ParseOutcome::Invalid(m) => panic!("test server got bad command: {m}"),
                        ParseOutcome::Incomplete => {}
                    }
                    let mut chunk = [0u8; 4096];
                    match stream.read(&mut chunk) {
                        Ok(0) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(_) => return,
                    }
                };
                let _ = tx.send(args);
                match act {
                    Act::Reply(bytes) => {
                        if stream.write_all(bytes).is_err() {
                            return;
                        }
                        let _ = stream.flush();
                    }
                    Act::Dribble(bytes) => {
                        for byte in bytes {
                            if stream.write_all(&[*byte]).is_err() {
                                return;
                            }
                            let _ = stream.flush();
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                    Act::Close => {
                        drop(stream);
                        break;
                    }
                    Act::Hang => {
                        thread::sleep(Duration::from_millis(400));
                    }
                }
            }
        }
    });
    Server {
        addr,
        seen,
        _handle: handle,
    }
}

impl Server {
    fn client(&self) -> Client {
        Client::new(self.addr.clone(), None).with_timeout(Duration::from_millis(200))
    }
    fn next_seen(&self) -> Vec<Vec<u8>> {
        self.seen
            .recv_timeout(Duration::from_secs(2))
            .expect("server should have received a command")
    }
}

// --- happy path ---

#[test]
fn round_trip_ping() {
    let server = spawn_server(vec![vec![Act::Reply(b"+PONG\r\n")]]);
    let mut client = server.client();
    assert_eq!(
        client.command(&[b"PING"]).unwrap(),
        Reply::Simple(b"PONG".to_vec())
    );
    assert_eq!(server.next_seen(), vec![b"PING".to_vec()]);
}

#[test]
fn connection_is_lazy_then_reused() {
    let server = spawn_server(vec![vec![
        Act::Reply(b"+OK\r\n"),
        Act::Reply(b"$1\r\nv\r\n"),
    ]]);
    let mut client = server.client();
    // Constructing a Client must not have connected — a backend builds one of
    // these before anyone knows if the cache is up.
    assert!(!client.is_connected());
    client.command(&[b"SET", b"k", b"v"]).unwrap();
    assert!(client.is_connected());
    // Second command must reuse the same socket: the scripted server only
    // accepts ONE connection here, so a reconnect would hang and fail.
    assert_eq!(
        client.command(&[b"GET", b"k"]).unwrap(),
        Reply::Bulk(Some(b"v".to_vec()))
    );
}

#[test]
fn null_and_empty_bulk_survive_the_client() {
    let server = spawn_server(vec![vec![
        Act::Reply(b"$-1\r\n"),
        Act::Reply(b"$0\r\n\r\n"),
    ]]);
    let mut client = server.client();
    // The trap from resp-protocol §8, end to end through a socket: a missing
    // key and an empty-string value must not arrive as the same thing, because
    // resp.get() has to map one to SQL NULL and the other to ''.
    assert_eq!(
        client.command(&[b"GET", b"missing"]).unwrap(),
        Reply::Bulk(None)
    );
    assert_eq!(
        client.command(&[b"GET", b"empty"]).unwrap(),
        Reply::Bulk(Some(vec![]))
    );
}

#[test]
fn binary_safe_round_trip() {
    let server = spawn_server(vec![vec![Act::Reply(b"$6\r\na\r\nb\0c\r\n")]]);
    let mut client = server.client();
    let value: &[u8] = b"a\r\nb\0c";
    let reply = client.command(&[b"SET", b"k", value]).unwrap();
    assert_eq!(reply, Reply::Bulk(Some(value.to_vec())));
    // And the server saw the value intact, not truncated at the CRLF.
    assert_eq!(server.next_seen()[2], value.to_vec());
}

#[test]
fn reply_arriving_one_byte_at_a_time() {
    let server = spawn_server(vec![vec![Act::Dribble(b"*2\r\n$2\r\nk1\r\n$2\r\nk2\r\n")]]);
    let mut client = server.client();
    assert_eq!(
        client.command(&[b"KEYS", b"*"]).unwrap(),
        Reply::Array(Some(vec![
            Reply::Bulk(Some(b"k1".to_vec())),
            Reply::Bulk(Some(b"k2".to_vec())),
        ]))
    );
}

// --- error reporting ---

#[test]
fn server_error_reply_is_not_a_transport_failure() {
    let server = spawn_server(vec![vec![
        Act::Reply(b"-ERR unknown command 'NOPE'\r\n"),
        Act::Reply(b"+PONG\r\n"),
    ]]);
    let mut client = server.client();
    match client.command(&[b"NOPE"]) {
        Err(ClientError::Server(msg)) => assert_eq!(msg, "ERR unknown command 'NOPE'"),
        other => panic!("expected Server error, got {other:?}"),
    }
    // The connection must still be usable: the server answered in well-formed
    // RESP, so the stream is perfectly in sync. (Same single scripted
    // connection — a reconnect would hang.)
    assert!(client.is_connected());
    assert_eq!(
        client.command(&[b"PING"]).unwrap(),
        Reply::Simple(b"PONG".to_vec())
    );
}

#[test]
fn protocol_garbage_is_fatal_to_the_connection() {
    let server = spawn_server(vec![vec![Act::Reply(b"@nonsense\r\n")], vec![]]);
    let mut client = server.client();
    match client.command(&[b"PING"]) {
        Err(ClientError::Protocol(_)) => {}
        other => panic!("expected Protocol error, got {other:?}"),
    }
    // A desynced parser can't be trusted to find the next reply boundary, so
    // the socket must have been dropped rather than reused.
    assert!(!client.is_connected());
}

#[test]
fn timeout_when_server_never_answers() {
    let server = spawn_server(vec![vec![Act::Hang]]);
    let mut client = server.client();
    let started = std::time::Instant::now();
    match client.command(&[b"PING"]) {
        Err(ClientError::Timeout) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
    // The whole point is bounding how long a Postgres backend can be stuck.
    // 200ms timeout, retried at most once, so well under a second.
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
    assert!(!client.is_connected());
}

#[test]
fn connect_failure_on_a_closed_port() {
    // Bind, learn the port, drop the listener: nothing is listening there now.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let mut client = Client::new(addr, None).with_timeout(Duration::from_millis(200));
    match client.command(&[b"PING"]) {
        Err(ClientError::Connect(_)) => {}
        other => panic!("expected Connect error, got {other:?}"),
    }
}

// --- the reconnect rule (the reason this crate exists) ---

#[test]
fn reused_dead_connection_reconnects_transparently() {
    // Connection 1 answers once, then closes — this is the bgworker having
    // restarted underneath a backend that cached the fd. Connection 2 is the
    // reconnect, and the caller must never see the failure.
    let server = spawn_server(vec![
        vec![Act::Reply(b"+OK\r\n"), Act::Close],
        vec![Act::Reply(b"$1\r\nv\r\n")],
    ]);
    let mut client = server.client();
    client.command(&[b"SET", b"k", b"v"]).unwrap();
    assert_eq!(
        client.command(&[b"GET", b"k"]).unwrap(),
        Reply::Bulk(Some(b"v".to_vec())),
        "a dead reused connection must be retried on a fresh socket"
    );
}

#[test]
fn fresh_connection_that_dies_is_not_retried() {
    // Only ONE connection is scripted, and it closes without answering. If the
    // client retried a brand-new connection, it would block waiting for an
    // accept that never comes; the assertion is that it reports promptly
    // instead.
    let server = spawn_server(vec![vec![Act::Close]]);
    let mut client = server.client();
    let started = std::time::Instant::now();
    match client.command(&[b"PING"]) {
        Err(ClientError::Closed) | Err(ClientError::Connect(_)) => {}
        other => panic!("expected Closed/Connect, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "should fail fast, took {:?}",
        started.elapsed()
    );
}

#[test]
fn command_no_retry_does_not_reconnect() {
    // Same script as the transparent-reconnect test, but via the no-retry
    // entry point: the second call must surface the error instead of silently
    // re-issuing a command that may already have been applied.
    let server = spawn_server(vec![
        vec![Act::Reply(b"+OK\r\n"), Act::Close],
        vec![Act::Reply(b"+OK\r\n")],
    ]);
    let mut client = server.client();
    client.command(&[b"SET", b"k", b"v"]).unwrap();
    match client.command_no_retry(&[b"INCR", b"counter"]) {
        Err(ClientError::Closed) => {}
        other => panic!("expected Closed, got {other:?}"),
    }
}

#[test]
fn close_mid_reply_is_never_retried() {
    // The server answered *partially* and then died. Retrying would risk
    // double-applying a command we know reached the server, so this must
    // surface as a Protocol error rather than being quietly re-issued.
    let server = spawn_server(vec![
        vec![Act::Reply(b"+OK\r\n")],
        vec![Act::Reply(b"$5\r\nhel")], // truncated bulk, then the script ends
    ]);
    let mut client = server.client();
    client.command(&[b"PING"]).unwrap();
    drop(client);

    // Fresh client so connection #2 of the script is the one under test.
    let mut client2 =
        Client::new(server.addr.clone(), None).with_timeout(Duration::from_millis(200));
    match client2.command(&[b"GET", b"k"]) {
        Err(ClientError::Protocol(msg)) => assert!(
            msg.contains("mid-reply"),
            "expected a mid-reply protocol error, got: {msg}"
        ),
        // A slow scripted server can also trip the read timeout here; both
        // outcomes are non-retrying, which is the property under test.
        Err(ClientError::Timeout) => {}
        other => panic!("expected Protocol/Timeout, got {other:?}"),
    }
    assert!(!client2.is_connected());
}

// --- AUTH ---

#[test]
fn auth_is_sent_before_the_first_command() {
    let server = spawn_server(vec![vec![
        Act::Reply(b"+OK\r\n"), // AUTH
        Act::Reply(b"+PONG\r\n"),
    ]]);
    let mut client = Client::new(server.addr.clone(), Some(b"s3cret".to_vec()))
        .with_timeout(Duration::from_millis(200));
    client.command(&[b"PING"]).unwrap();
    assert_eq!(
        server.next_seen(),
        vec![b"AUTH".to_vec(), b"s3cret".to_vec()],
        "AUTH must be the first thing on a new connection"
    );
    assert_eq!(server.next_seen(), vec![b"PING".to_vec()]);
}

#[test]
fn auth_rejection_surfaces_as_auth_error() {
    let server = spawn_server(vec![vec![Act::Reply(b"-WRONGPASS invalid password\r\n")]]);
    let mut client = Client::new(server.addr.clone(), Some(b"wrong".to_vec()))
        .with_timeout(Duration::from_millis(200));
    match client.command(&[b"PING"]) {
        Err(ClientError::Auth(msg)) => assert!(msg.contains("WRONGPASS")),
        other => panic!("expected Auth error, got {other:?}"),
    }
    assert!(!client.is_connected());
}

#[test]
fn empty_password_means_no_auth() {
    let server = spawn_server(vec![vec![Act::Reply(b"+PONG\r\n")]]);
    // An unset pg_resp.password GUC reads back as an empty string, not as
    // absent — treating that as "AUTH with an empty password" would break
    // every default install.
    let mut client =
        Client::new(server.addr.clone(), Some(Vec::new())).with_timeout(Duration::from_millis(200));
    client.command(&[b"PING"]).unwrap();
    assert_eq!(server.next_seen(), vec![b"PING".to_vec()]);
}

#[test]
fn auth_replays_on_reconnect() {
    // The reconnect must re-authenticate; a fresh socket is unauthenticated no
    // matter what the previous one had done.
    let server = spawn_server(vec![
        vec![Act::Reply(b"+OK\r\n"), Act::Close],
        vec![Act::Reply(b"+OK\r\n"), Act::Reply(b"+PONG\r\n")],
    ]);
    let mut client = Client::new(server.addr.clone(), Some(b"pw".to_vec()))
        .with_timeout(Duration::from_millis(200));
    client.command(&[b"PING"]).unwrap_err(); // conn 1: AUTH ok, PING gets Close
    assert_eq!(server.next_seen(), vec![b"AUTH".to_vec(), b"pw".to_vec()]);
    assert_eq!(server.next_seen(), vec![b"PING".to_vec()]);
    assert_eq!(
        client.command(&[b"PING"]).unwrap(),
        Reply::Simple(b"PONG".to_vec())
    );
    assert_eq!(
        server.next_seen(),
        vec![b"AUTH".to_vec(), b"pw".to_vec()],
        "reconnect must re-send AUTH"
    );
}

// --- the S6 watchdog probe ---

#[test]
fn probe_treats_noauth_as_alive() {
    let server = spawn_server(vec![vec![Act::Reply(
        b"-NOAUTH Authentication required.\r\n",
    )]]);
    let mut client = server.client();
    // This is the case that would otherwise make the watchdog kill a healthy
    // worker in a loop whenever pg_resp.password is set.
    assert!(
        client.probe().is_ok(),
        "an error REPLY still proves the server parsed a command and framed an answer"
    );
}

#[test]
fn probe_fails_when_nothing_is_listening() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let mut client = Client::new(addr, None).with_timeout(Duration::from_millis(200));
    assert!(client.probe().is_err());
}

#[test]
fn probe_fails_when_port_accepts_but_never_answers() {
    // A half-dead server — accepting connections but not answering — is
    // exactly the silent-zombie state S6 exists to catch, and the one a plain
    // TCP-connect check would call healthy.
    let server = spawn_server(vec![vec![Act::Hang]]);
    let mut client = server.client();
    match client.probe() {
        Err(ClientError::Timeout) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
}

// --- desync guard ---

#[test]
fn unsolicited_extra_reply_bytes_are_caught() {
    // Server sends two replies to one command. The extra bytes must be
    // detected as a desync on the next command instead of being handed back as
    // that command's answer.
    let server = spawn_server(vec![vec![
        Act::Reply(b"+OK\r\n:99\r\n"),
        Act::Reply(b"+PONG\r\n"),
    ]]);
    let mut client = server.client();
    assert_eq!(
        client.command(&[b"SET", b"k", b"v"]).unwrap(),
        Reply::Simple(b"OK".to_vec())
    );
    match client.command(&[b"PING"]) {
        Err(ClientError::Protocol(msg)) => assert!(
            msg.contains("desynchronized"),
            "expected a desync error, got: {msg}"
        ),
        other => panic!("expected Protocol desync error, got {other:?}"),
    }
    assert!(!client.is_connected());
}

#[test]
fn stream_used_after_disconnect_reconnects() {
    let server = spawn_server(vec![
        vec![Act::Reply(b"+PONG\r\n")],
        vec![Act::Reply(b"+PONG\r\n")],
    ]);
    let mut client = server.client();
    client.command(&[b"PING"]).unwrap();
    client.disconnect();
    assert!(!client.is_connected());
    client.command(&[b"PING"]).unwrap();
    assert!(client.is_connected());
}

/// Keeps `TcpStream` imported for the scripted-server helpers above without
/// tripping the unused-import lint if a test is removed.
#[allow(dead_code)]
fn _assert_types(_s: Option<TcpStream>) {}
