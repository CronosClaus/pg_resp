//! RESP2 wire protocol: parser (client requests) and serializer (server
//! replies). No PG dependency — pure Rust, fuzzable standalone. Ground truth:
//! `.claude/skills/resp-protocol/SKILL.md` (framing, error taxonomy, vectors).

/// Defensive parser limits (not part of the RESP2 spec itself, but required
/// parser hygiene — see resp-protocol skill's traps section: unbounded array
/// nesting / bulk length parsing is the textbook fuzz-crash surface).
pub const MAX_BULK_LEN: i64 = 512 * 1024 * 1024; // matches Redis's proto-max-bulk-len default
pub const MAX_ARRAY_LEN: i64 = 1024 * 1024; // defensive cap, not a protocol limit
pub const MAX_INLINE_LEN: usize = 64 * 1024; // defensive cap on inline command line length

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

    ParseOutcome::Complete { args, consumed: pos }
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
