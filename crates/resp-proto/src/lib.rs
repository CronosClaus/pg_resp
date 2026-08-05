//! RESP2 wire protocol, both directions.
//!
//! Server side (Phase 1): [`parse_command`] reads client requests, [`Reply`]
//! serializes server responses.
//!
//! Client side (Phase 3): [`encode_command`] writes a request, [`parse_reply`]
//! reads a server response. These exist because bible D3 implements the SQL
//! surface as a *loopback RESP client* living inside a Postgres backend — so
//! this crate is now parsed-against from both ends of the same socket.
//!
//! No PG dependency — pure Rust, fuzzable standalone. Ground truth:
//! `.claude/skills/resp-protocol/SKILL.md` (framing, error taxonomy, vectors).

/// Defensive parser limits (not part of the RESP2 spec itself, but required
/// parser hygiene — see resp-protocol skill's traps section: unbounded array
/// nesting / bulk length parsing is the textbook fuzz-crash surface).
pub const MAX_BULK_LEN: i64 = 512 * 1024 * 1024; // matches Redis's proto-max-bulk-len default
pub const MAX_ARRAY_LEN: i64 = 1024 * 1024; // defensive cap, not a protocol limit
pub const MAX_INLINE_LEN: usize = 64 * 1024; // defensive cap on inline command line length

/// Maximum array nesting depth [`parse_reply`] will descend. RESP2 itself
/// imposes no limit, which is exactly why a recursive-descent reply parser
/// needs one (resp-protocol skill §8: "unbounded array-nesting is the
/// textbook fuzz crash"). pg_resp's own deepest reply is depth 2 (`SCAN`'s
/// `[cursor, [keys...]]`), so 32 is generous by an order of magnitude while
/// still bounding stack use to something trivial.
pub const MAX_REPLY_DEPTH: usize = 32;

/// A server reply value. Covers exactly the 5 RESP2 types (skill §1) —
/// deliberately no RESP3 types (bible §3.4/D9: T0-T2 scope only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `+<text>\r\n`. `text` must not contain CR or LF (unenforced here —
    /// callers only ever construct these from static strings, never
    /// wire-derived data).
    Simple(Vec<u8>),
    /// `-<text>\r\n`.
    Error(Vec<u8>),
    /// `:<n>\r\n`.
    Integer(i64),
    /// `$<len>\r\n<data>\r\n`, or `$-1\r\n` for `None` (null bulk string —
    /// distinct from `Some(vec![])`, the empty string).
    Bulk(Option<Vec<u8>>),
    /// `*<len>\r\n<elem>...`, or `*-1\r\n` for `None` (null array).
    Array(Option<Vec<Reply>>),
}

impl Reply {
    pub fn ok() -> Reply {
        Reply::Simple(b"OK".to_vec())
    }

    pub fn simple(s: impl Into<Vec<u8>>) -> Reply {
        Reply::Simple(s.into())
    }

    pub fn error(s: impl Into<Vec<u8>>) -> Reply {
        Reply::Error(s.into())
    }

    pub fn bulk(s: impl Into<Vec<u8>>) -> Reply {
        Reply::Bulk(Some(s.into()))
    }

    pub fn nil() -> Reply {
        Reply::Bulk(None)
    }

    /// Serialize into `out`, appending (never truncating existing content) —
    /// callers reuse one output buffer across many replies on the hot path.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Reply::Simple(s) => {
                out.push(b'+');
                out.extend_from_slice(s);
                out.extend_from_slice(b"\r\n");
            }
            Reply::Error(s) => {
                out.push(b'-');
                out.extend_from_slice(s);
                out.extend_from_slice(b"\r\n");
            }
            Reply::Integer(n) => {
                out.push(b':');
                out.extend_from_slice(n.to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Reply::Bulk(None) => out.extend_from_slice(b"$-1\r\n"),
            Reply::Bulk(Some(data)) => {
                out.push(b'$');
                out.extend_from_slice(data.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(data);
                out.extend_from_slice(b"\r\n");
            }
            Reply::Array(None) => out.extend_from_slice(b"*-1\r\n"),
            Reply::Array(Some(items)) => {
                out.push(b'*');
                out.extend_from_slice(items.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                for item in items {
                    item.write_to(out);
                }
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_to(&mut out);
        out
    }
}

/// Result of attempting to parse one command from a byte buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    /// A full command was parsed. `args` is the command name + arguments as
    /// raw bytes (never interpreted here — command dispatch is resp-store/
    /// pg_resp's job); `consumed` is how many bytes of `buf` to drop.
    /// Empty `args` (a blank inline line, or `*0\r\n`) means: no-op, consume
    /// and continue — matches Redis's handling of empty commands.
    Complete { args: Vec<Vec<u8>>, consumed: usize },
    /// Not enough bytes yet; caller should read more and retry with a larger
    /// buffer.
    Incomplete,
    /// Malformed input that can never become valid by reading more bytes.
    /// Caller should reply with a protocol error and, per Redis convention,
    /// close the connection (a desynced parser cannot safely continue).
    Invalid(String),
}

/// Parse exactly one command (RESP array-of-bulk-strings, or an inline
/// command) from the front of `buf`. See resp-protocol skill §1/§2 for the
/// framing rules this implements.
pub fn parse_command(buf: &[u8]) -> ParseOutcome {
    if buf.is_empty() {
        return ParseOutcome::Incomplete;
    }
    if buf[0] == b'*' {
        parse_array_command(buf)
    } else {
        parse_inline_command(buf)
    }
}

fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_i64_strict(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse::<i64>().ok()
}

fn parse_array_command(buf: &[u8]) -> ParseOutcome {
    let Some(crlf) = find_crlf(buf, 1) else {
        if buf.len() > MAX_INLINE_LEN {
            return ParseOutcome::Invalid("count line too long".to_string());
        }
        return ParseOutcome::Incomplete;
    };
    let Some(count) = parse_i64_strict(&buf[1..crlf]) else {
        return ParseOutcome::Invalid("invalid multibulk length".to_string());
    };
    if count < 0 {
        return ParseOutcome::Invalid("invalid multibulk length".to_string());
    }
    if count > MAX_ARRAY_LEN {
        return ParseOutcome::Invalid("multibulk length too large".to_string());
    }
    let mut pos = crlf + 2;
    let count = count as usize;
    let mut args = Vec::with_capacity(count.min(1024));

    for _ in 0..count {
        if pos >= buf.len() {
            return ParseOutcome::Incomplete;
        }
        if buf[pos] != b'$' {
            return ParseOutcome::Invalid(format!("expected '$', got '{}'", buf[pos] as char));
        }
        let Some(len_crlf) = find_crlf(buf, pos + 1) else {
            if buf.len() - pos > MAX_INLINE_LEN {
                return ParseOutcome::Invalid("bulk length line too long".to_string());
            }
            return ParseOutcome::Incomplete;
        };
        let Some(len) = parse_i64_strict(&buf[pos + 1..len_crlf]) else {
            return ParseOutcome::Invalid("invalid bulk length".to_string());
        };
        if len < 0 {
            return ParseOutcome::Invalid("invalid bulk length".to_string());
        }
        if len > MAX_BULK_LEN {
            return ParseOutcome::Invalid("bulk length too large".to_string());
        }
        let len = len as usize;
        let data_start = len_crlf + 2;
        let data_end = data_start + len;
        if data_end + 2 > buf.len() {
            return ParseOutcome::Incomplete;
        }
        if buf[data_end] != b'\r' || buf[data_end + 1] != b'\n' {
            return ParseOutcome::Invalid("expected CRLF after bulk data".to_string());
        }
        args.push(buf[data_start..data_end].to_vec());
        pos = data_end + 2;
    }

    ParseOutcome::Complete {
        args,
        consumed: pos,
    }
}

/// Encode a command as a RESP2 array of bulk strings, appending to `out`.
///
/// This is the only request shape a client may send (resp-protocol skill §1:
/// "Clients send commands as a RESP **Array of Bulk Strings only**"). Inline
/// commands are a server-side courtesy for humans with telnet, never
/// something pg_resp's own client emits.
///
/// Binary-safe: `args` are written with an explicit length prefix, so keys
/// and values containing CR, LF, or NUL round-trip intact. Phase 2's
/// adversarial deck proved the server side handles those; the SQL surface
/// can pass them straight through from `bytea`/`text` without escaping.
pub fn encode_command_into(args: &[&[u8]], out: &mut Vec<u8>) {
    out.push(b'*');
    out.extend_from_slice(args.len().to_string().as_bytes());
    out.extend_from_slice(b"\r\n");
    for arg in args {
        out.push(b'$');
        out.extend_from_slice(arg.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
}

/// [`encode_command_into`] into a fresh buffer.
pub fn encode_command(args: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_command_into(args, &mut out);
    out
}

/// Result of attempting to parse one server reply from a byte buffer.
///
/// Deliberately mirrors [`ParseOutcome`]'s three-way shape rather than using
/// `Option`/`Result`: a client's read loop needs the same
/// need-more-bytes-vs-permanently-broken distinction the server's does, and
/// conflating them is how a client either spins on a truncated reply or
/// closes a connection that was merely slow.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplyOutcome {
    /// A full reply was parsed; `consumed` is how many bytes of `buf` to drop.
    Complete { reply: Reply, consumed: usize },
    /// Not enough bytes yet; read more and retry with a larger buffer.
    Incomplete,
    /// Malformed input that can never become valid by reading more bytes.
    /// The caller must drop the connection — a desynced reply parser cannot
    /// safely resynchronize, since it can no longer tell payload from framing.
    Invalid(String),
}

/// Parse exactly one reply from the front of `buf`.
///
/// Accepts all five RESP2 types and both null forms (`$-1\r\n`, `*-1\r\n`).
/// Rejects RESP3 sigils as `Invalid` — pg_resp never negotiates RESP3
/// (resp-protocol skill §6/§8), so seeing one means the socket is not talking
/// to what we think it is, which is a fail-loudly situation rather than
/// something to skip over.
pub fn parse_reply(buf: &[u8]) -> ReplyOutcome {
    match parse_reply_at(buf, 0, 0) {
        Step::Done(reply, pos) => ReplyOutcome::Complete {
            reply,
            consumed: pos,
        },
        Step::Incomplete => ReplyOutcome::Incomplete,
        Step::Invalid(msg) => ReplyOutcome::Invalid(msg),
    }
}

/// Internal recursion result: like [`ReplyOutcome`] but carrying an absolute
/// buffer position instead of a consumed-length, so nested elements can be
/// parsed without re-slicing (and without the quadratic cost that implies on
/// a large array).
enum Step {
    Done(Reply, usize),
    Incomplete,
    Invalid(String),
}

/// Read one CRLF-terminated header line starting at `pos` (the byte after the
/// sigil). Returns the payload bytes and the position just past the CRLF.
fn read_line(buf: &[u8], pos: usize) -> Result<Option<(&[u8], usize)>, String> {
    match find_crlf(buf, pos) {
        Some(crlf) => Ok(Some((&buf[pos..crlf], crlf + 2))),
        None => {
            // No terminator yet. Only "incomplete" while the unterminated
            // line is still plausibly short — otherwise a peer that never
            // sends CRLF would make us buffer without bound.
            if buf.len().saturating_sub(pos) > MAX_INLINE_LEN {
                Err("reply line too long".to_string())
            } else {
                Ok(None)
            }
        }
    }
}

fn parse_reply_at(buf: &[u8], pos: usize, depth: usize) -> Step {
    if depth > MAX_REPLY_DEPTH {
        return Step::Invalid(format!("reply nested deeper than {MAX_REPLY_DEPTH}"));
    }
    if pos >= buf.len() {
        return Step::Incomplete;
    }
    let sigil = buf[pos];
    let body = pos + 1;
    match sigil {
        b'+' | b'-' => match read_line(buf, body) {
            Err(msg) => Step::Invalid(msg),
            Ok(None) => Step::Incomplete,
            Ok(Some((line, next))) => {
                let payload = line.to_vec();
                let reply = if sigil == b'+' {
                    Reply::Simple(payload)
                } else {
                    Reply::Error(payload)
                };
                Step::Done(reply, next)
            }
        },
        b':' => match read_line(buf, body) {
            Err(msg) => Step::Invalid(msg),
            Ok(None) => Step::Incomplete,
            Ok(Some((line, next))) => match parse_i64_strict(line) {
                Some(n) => Step::Done(Reply::Integer(n), next),
                None => Step::Invalid("invalid integer reply".to_string()),
            },
        },
        b'$' => match read_line(buf, body) {
            Err(msg) => Step::Invalid(msg),
            Ok(None) => Step::Incomplete,
            Ok(Some((line, data_start))) => {
                let Some(len) = parse_i64_strict(line) else {
                    return Step::Invalid("invalid bulk length".to_string());
                };
                // -1 is the null bulk string, the ONLY legal negative length.
                // $-2 and friends are malformed, not "some other kind of
                // null" — resp-protocol skill §1.
                if len == -1 {
                    return Step::Done(Reply::Bulk(None), data_start);
                }
                if len < 0 {
                    return Step::Invalid("invalid bulk length".to_string());
                }
                if len > MAX_BULK_LEN {
                    return Step::Invalid("bulk length too large".to_string());
                }
                let len = len as usize;
                let data_end = data_start + len;
                if data_end + 2 > buf.len() {
                    return Step::Incomplete;
                }
                if buf[data_end] != b'\r' || buf[data_end + 1] != b'\n' {
                    return Step::Invalid("expected CRLF after bulk data".to_string());
                }
                Step::Done(
                    Reply::Bulk(Some(buf[data_start..data_end].to_vec())),
                    data_end + 2,
                )
            }
        },
        b'*' => match read_line(buf, body) {
            Err(msg) => Step::Invalid(msg),
            Ok(None) => Step::Incomplete,
            Ok(Some((line, mut next))) => {
                let Some(count) = parse_i64_strict(line) else {
                    return Step::Invalid("invalid multibulk length".to_string());
                };
                if count == -1 {
                    return Step::Done(Reply::Array(None), next);
                }
                if count < 0 {
                    return Step::Invalid("invalid multibulk length".to_string());
                }
                if count > MAX_ARRAY_LEN {
                    return Step::Invalid("multibulk length too large".to_string());
                }
                let count = count as usize;
                // Don't pre-allocate `count` slots: the count is peer-supplied
                // and this parser runs inside a Postgres backend, so a bogus
                // `*1000000` must not become a megabyte of allocation before
                // the first element is even validated.
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    match parse_reply_at(buf, next, depth + 1) {
                        Step::Done(item, after) => {
                            items.push(item);
                            next = after;
                        }
                        other => return other,
                    }
                }
                Step::Done(Reply::Array(Some(items)), next)
            }
        },
        other => Step::Invalid(format!(
            "unexpected reply type byte '{}' (0x{:02x})",
            other as char, other
        )),
    }
}

fn parse_inline_command(buf: &[u8]) -> ParseOutcome {
    let Some(crlf) = find_crlf(buf, 0) else {
        if buf.len() > MAX_INLINE_LEN {
            return ParseOutcome::Invalid("inline command too long".to_string());
        }
        return ParseOutcome::Incomplete;
    };
    let line = &buf[..crlf];
    let args: Vec<Vec<u8>> = line
        .split(|&b| b == b' ')
        .filter(|tok| !tok.is_empty())
        .map(|tok| tok.to_vec())
        .collect();
    ParseOutcome::Complete {
        args,
        consumed: crlf + 2,
    }
}

#[cfg(test)]
mod tests;
