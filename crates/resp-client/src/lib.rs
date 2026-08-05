//! Blocking loopback RESP2 client (`D11`).
//!
//! Bible `D3` implements pg_resp's SQL surface as a loopback RESP call: a
//! Postgres *backend* connects to the bgworker's own TCP port and speaks the
//! same protocol any redis client would. This crate is that client.
//!
//! **Why it is a separate, PG-free crate.** Two reasons, both about where the
//! bugs live:
//!
//! 1. Every hard part here is I/O edge cases — half-arrived replies, a peer
//!    that vanished between statements, a read that times out mid-reply. Those
//!    are miserable to iterate on through `cargo pgrx test` (minutes per
//!    cycle, a whole Postgres in the loop) and trivial to iterate on against a
//!    `TcpListener` in a unit test (seconds, no Postgres at all). All of the
//!    tests in this crate run in the fast loop.
//! 2. It keeps `resp-proto` a pure parser/serializer with no socket code, so
//!    its fuzz targets stay honestly "fuzzable in isolation" (bible §6).
//!
//! **Thread safety / FFI.** Nothing here touches pgrx or PG FFI, so it is
//! callable from any thread — which matters twice over: the SQL functions call
//! it from a backend's main thread, and the S6 watchdog calls [`Client::probe`]
//! from the bgworker's main thread. Iron rule 1 is satisfied by construction
//! rather than by discipline.

use resp_proto::{encode_command_into, parse_reply, Reply, ReplyOutcome};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Everything that can go wrong on a loopback call.
///
/// Deliberately *not* collapsed into a single string: the caller's correct
/// response differs per variant. In particular the post-commit applier
/// (bible §3.3) must distinguish "the cache rejected this command" (a bug in
/// our own SQL layer, worth a loud WARNING) from "the server is not there"
/// (an operational condition that costs an invalidation and is counted, not
/// shouted about per-row).
#[derive(Debug)]
pub enum ClientError {
    /// Could not establish a connection at all.
    Connect(std::io::Error),
    /// Socket error during an established conversation.
    Io(std::io::Error),
    /// The read timeout elapsed before a complete reply arrived.
    Timeout,
    /// Peer closed the connection before a complete reply arrived.
    Closed,
    /// Bytes on the wire were not valid RESP2, or arrived when none were
    /// expected. Unrecoverable for this connection: a desynced parser cannot
    /// tell framing from payload any more.
    Protocol(String),
    /// The server answered, and its answer was a RESP error reply. This is a
    /// *successful* round trip at the transport level — the connection stays
    /// usable — so it is reported separately from the failures above.
    Server(String),
    /// `AUTH` was required and rejected. The password GUC and the server
    /// disagree; retrying will not help.
    Auth(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Connect(e) => write!(f, "could not connect: {e}"),
            ClientError::Io(e) => write!(f, "socket error: {e}"),
            ClientError::Timeout => write!(f, "timed out waiting for a reply"),
            ClientError::Closed => write!(f, "server closed the connection"),
            ClientError::Protocol(m) => write!(f, "protocol error: {m}"),
            ClientError::Server(m) => write!(f, "{m}"),
            ClientError::Auth(m) => write!(f, "authentication failed: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ClientError {
    /// Whether this error means the connection is no longer usable and must be
    /// discarded. [`ClientError::Server`] is the notable exception: the server
    /// said "no" in well-formed RESP, which leaves the stream perfectly
    /// synchronized.
    pub fn is_fatal_to_connection(&self) -> bool {
        !matches!(self, ClientError::Server(_))
    }
}

/// Default read/write timeout. Bible §3.3 budgets ~50-150µs for a loopback
/// call, so a whole second is four orders of magnitude of headroom — it is a
/// deadlock backstop, not a latency target. It must never be `None`: an
/// untimed read here would hang a *Postgres backend* indefinitely, which is
/// how a cache turns into an outage.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

const READ_CHUNK: usize = 16 * 1024;

/// A lazily-connected, reconnecting RESP2 client for one peer.
///
/// Holds at most one socket. Connection is deferred until the first command,
/// so constructing one is free and cannot fail — important because the
/// natural place to keep it is a process-local static in a Postgres backend,
/// initialized long before anyone knows whether the cache is reachable.
pub struct Client {
    addr: String,
    password: Option<Vec<u8>>,
    timeout: Duration,
    conn: Option<Conn>,
}

struct Conn {
    stream: TcpStream,
    /// Bytes read from the socket but not yet consumed by the reply parser.
    /// Normally drained to empty after every reply; anything left over at the
    /// start of the next command means the stream desynchronized.
    read_buf: Vec<u8>,
}

impl Client {
    /// `addr` is anything `to_socket_addrs` accepts, e.g. `"127.0.0.1:6379"`.
    /// `password` empty/`None` means no `AUTH` is attempted.
    pub fn new(addr: impl Into<String>, password: Option<Vec<u8>>) -> Client {
        Client {
            addr: addr.into(),
            password: password.filter(|p| !p.is_empty()),
            timeout: DEFAULT_TIMEOUT,
            conn: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Client {
        self.timeout = timeout;
        self
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Whether a socket is currently held. Does not probe it — a connection
    /// can be held and dead at the same time, which is exactly the case
    /// [`Client::command`]'s retry exists for.
    pub fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    /// Drop any held connection. The next command reconnects.
    pub fn disconnect(&mut self) {
        self.conn = None;
    }

    /// Send a command and read its reply, reconnecting once if a *reused*
    /// connection turns out to be dead.
    ///
    /// **The retry rule, and why it is drawn where it is.** The bgworker can
    /// restart underneath us (`bgw_restart_time`), which leaves a backend
    /// holding a file descriptor to nothing. Worse, that discovery usually
    /// happens on the *read*, not the write: the write lands in a socket
    /// buffer and succeeds, and only the subsequent read reports ECONNRESET.
    /// So "only retry if the write failed" would miss the very case this
    /// exists for.
    ///
    /// Instead: retry once, on a fresh connection, if and only if
    ///   (a) the connection was one we had already established before this
    ///       call — a brand-new connection that dies immediately is a real
    ///       failure, not a stale handle, and retrying it just doubles the
    ///       latency before reporting the same error; and
    ///   (b) not one byte of a reply had arrived. Once the server has started
    ///       answering, it has certainly executed the command.
    ///
    /// This means a command *can* be delivered twice in one narrow case: the
    /// server accepted and executed it, then died before writing any reply. So
    /// this method is safe only for idempotent commands. Every command the SQL
    /// surface issues (`GET`, `SET`, `DEL`, `INFO`, `SCAN`, `EXISTS`, `TTL`)
    /// is idempotent, so this is the right default here — but use
    /// [`Client::command_no_retry`] for anything where a double-apply would be
    /// visible (`INCR` being the obvious one).
    pub fn command(&mut self, args: &[&[u8]]) -> Result<Reply, ClientError> {
        let was_reused = self.conn.is_some();
        match self.attempt(args) {
            Ok(reply) => Ok(reply),
            Err(err) => {
                if err.is_fatal_to_connection() {
                    self.conn = None;
                }
                let retryable = was_reused
                    && matches!(
                        err,
                        ClientError::Io(_) | ClientError::Closed | ClientError::Connect(_)
                    );
                if !retryable {
                    return Err(err);
                }
                self.attempt(args)
            }
        }
    }

    /// Like [`Client::command`] but never retries. Use for commands whose
    /// double-application would be observable.
    pub fn command_no_retry(&mut self, args: &[&[u8]]) -> Result<Reply, ClientError> {
        match self.attempt(args) {
            Ok(reply) => Ok(reply),
            Err(err) => {
                if err.is_fatal_to_connection() {
                    self.conn = None;
                }
                Err(err)
            }
        }
    }

    /// Liveness probe for the S6 watchdog.
    ///
    /// Sends `PING` and treats **any well-formed RESP reply** as proof of life,
    /// including an error reply. That is deliberate: with `pg_resp.password`
    /// set, an unauthenticated `PING` is answered
    /// `-NOAUTH Authentication required.`, and a watchdog that read that as
    /// "dead" would kill and restart a perfectly healthy worker in a loop —
    /// turning a security setting into a crash loop. What the probe actually
    /// asserts is "something on that port parsed my command and framed a
    /// reply", which is precisely the property whose loss S6 is watching for.
    pub fn probe(&mut self) -> Result<(), ClientError> {
        match self.command(&[b"PING"]) {
            Ok(_) => Ok(()),
            Err(ClientError::Server(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// One attempt: ensure a connection (authenticating if needed), write the
    /// request, read exactly one reply.
    fn attempt(&mut self, args: &[&[u8]]) -> Result<Reply, ClientError> {
        self.ensure_connected()?;
        self.round_trip(args)
    }

    fn ensure_connected(&mut self) -> Result<(), ClientError> {
        if self.conn.is_some() {
            return Ok(());
        }
        let stream = self.dial()?;
        self.conn = Some(Conn {
            stream,
            read_buf: Vec::new(),
        });
        if let Some(password) = self.password.clone() {
            match self.round_trip(&[b"AUTH", &password]) {
                Ok(Reply::Simple(_)) => {}
                Ok(other) => {
                    self.conn = None;
                    return Err(ClientError::Auth(format!(
                        "unexpected AUTH reply: {other:?}"
                    )));
                }
                Err(ClientError::Server(msg)) => {
                    self.conn = None;
                    return Err(ClientError::Auth(msg));
                }
                Err(e) => {
                    self.conn = None;
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn dial(&self) -> Result<TcpStream, ClientError> {
        // Resolve explicitly so a bad address is a clear Connect error rather
        // than a confusing io::Error from deep inside connect().
        let mut last_err: Option<std::io::Error> = None;
        let addrs = self
            .addr
            .to_socket_addrs()
            .map_err(ClientError::Connect)?
            .collect::<Vec<_>>();
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, self.timeout) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(self.timeout))
                        .map_err(ClientError::Connect)?;
                    stream
                        .set_write_timeout(Some(self.timeout))
                        .map_err(ClientError::Connect)?;
                    // Loopback request/response with tiny payloads is exactly
                    // the shape Nagle's algorithm punishes: it would hold our
                    // request waiting for more data that is never coming.
                    stream.set_nodelay(true).map_err(ClientError::Connect)?;
                    return Ok(stream);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(ClientError::Connect(last_err.unwrap_or_else(|| {
            std::io::Error::new(ErrorKind::AddrNotAvailable, "no addresses resolved")
        })))
    }

    fn round_trip(&mut self, args: &[&[u8]]) -> Result<Reply, ClientError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| ClientError::Protocol("not connected".to_string()))?;

        // Leftover bytes from a previous reply mean we lost track of the
        // stream's framing. Reconnecting is the only safe move; guessing where
        // the next reply starts is how a client returns one key's value for
        // another key's GET.
        if !conn.read_buf.is_empty() {
            return Err(ClientError::Protocol(
                "unexpected leftover bytes from a previous reply (stream desynchronized)"
                    .to_string(),
            ));
        }

        let mut request = Vec::new();
        encode_command_into(args, &mut request);
        conn.stream.write_all(&request).map_err(map_io(false))?;
        conn.stream.flush().map_err(map_io(false))?;

        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match parse_reply(&conn.read_buf) {
                ReplyOutcome::Complete { reply, consumed } => {
                    conn.read_buf.drain(..consumed);
                    return match reply {
                        Reply::Error(msg) => Err(ClientError::Server(
                            String::from_utf8_lossy(&msg).into_owned(),
                        )),
                        other => Ok(other),
                    };
                }
                ReplyOutcome::Invalid(msg) => return Err(ClientError::Protocol(msg)),
                ReplyOutcome::Incomplete => {}
            }
            match conn.stream.read(&mut chunk) {
                Ok(0) => {
                    // EOF. `Closed` is defined to mean "closed with not one
                    // byte of a reply seen" — the retry-safe condition (b) in
                    // Client::command — so the distinction has to be made
                    // *here*, where it is still knowable, rather than
                    // reconstructed by the caller.
                    return Err(if conn.read_buf.is_empty() {
                        ClientError::Closed
                    } else {
                        ClientError::Protocol("server closed the connection mid-reply".to_string())
                    });
                }
                Ok(n) => conn.read_buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(map_io(!conn.read_buf.is_empty())(e)),
            }
        }
    }
}

/// Classify a socket error. `partial_reply_seen` is threaded through so that a
/// timeout or reset *after* the server started answering is never mistaken for
/// a never-delivered command.
fn map_io(partial_reply_seen: bool) -> impl Fn(std::io::Error) -> ClientError {
    move |e: std::io::Error| match e.kind() {
        // A read timeout on a socket with a timeout set surfaces as WouldBlock
        // on some platforms and TimedOut on others; both mean the same thing.
        ErrorKind::WouldBlock | ErrorKind::TimedOut => ClientError::Timeout,
        _ if partial_reply_seen => ClientError::Protocol(format!(
            "connection failed after a partial reply had arrived: {e}"
        )),
        _ => ClientError::Io(e),
    }
}

#[cfg(test)]
mod tests;
