use super::*;

fn args(v: &[&str]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_bytes().to_vec()).collect()
}

// --- Reply serialization: byte-exact vs resp-protocol skill §1/§7 ---

#[test]
fn simple_string_ok() {
    assert_eq!(Reply::ok().to_bytes(), b"+OK\r\n");
}

#[test]
fn error_reply() {
    assert_eq!(
        Reply::error(&b"ERR unknown command 'FOOX'"[..]).to_bytes(),
        b"-ERR unknown command 'FOOX'\r\n"
    );
}

#[test]
fn integer_reply() {
    assert_eq!(Reply::Integer(0).to_bytes(), b":0\r\n");
    assert_eq!(Reply::Integer(1000).to_bytes(), b":1000\r\n");
    assert_eq!(Reply::Integer(-2).to_bytes(), b":-2\r\n");
}

#[test]
fn bulk_string_reply() {
    assert_eq!(Reply::bulk(&b"hello"[..]).to_bytes(), b"$5\r\nhello\r\n");
}

#[test]
fn null_bulk_vs_empty_bulk() {
    // The #1 trap per the skill: null ($-1) vs empty ($0) are distinct.
    assert_eq!(Reply::nil().to_bytes(), b"$-1\r\n");
    assert_eq!(Reply::bulk(&b""[..]).to_bytes(), b"$0\r\n\r\n");
    assert_ne!(Reply::nil().to_bytes(), Reply::bulk(&b""[..]).to_bytes());
}

#[test]
fn array_reply_mget_shape() {
    // vector 36: MGET k1 missing k2 -> [v1, nil, v2]
    let reply = Reply::Array(Some(vec![
        Reply::bulk(&b"v1"[..]),
        Reply::nil(),
        Reply::bulk(&b"v2"[..]),
    ]));
    assert_eq!(reply.to_bytes(), b"*3\r\n$2\r\nv1\r\n$-1\r\n$2\r\nv2\r\n");
}

#[test]
fn empty_array_reply() {
    assert_eq!(Reply::Array(Some(vec![])).to_bytes(), b"*0\r\n");
}

#[test]
fn null_array_reply() {
    assert_eq!(Reply::Array(None).to_bytes(), b"*-1\r\n");
}

#[test]
fn nested_array_reply() {
    // spec example: [[1,2,3],["Hello", Error("World")]]
    let reply = Reply::Array(Some(vec![
        Reply::Array(Some(vec![
            Reply::Integer(1),
            Reply::Integer(2),
            Reply::Integer(3),
        ])),
        Reply::Array(Some(vec![
            Reply::simple(&b"Hello"[..]),
            Reply::error(&b"World"[..]),
        ])),
    ]));
    assert_eq!(
        reply.to_bytes(),
        b"*2\r\n*3\r\n:1\r\n:2\r\n:3\r\n*2\r\n+Hello\r\n-World\r\n"
    );
}

// --- parse_command: RESP array requests, byte-exact vs skill §7 vectors ---

#[test]
fn parse_ping_array() {
    let buf = b"*1\r\n$4\r\nPING\r\n";
    assert_eq!(
        parse_command(buf),
        ParseOutcome::Complete {
            args: args(&["PING"]),
            consumed: buf.len(),
        }
    );
}

#[test]
fn parse_ping_with_message_array() {
    let buf = b"*2\r\n$4\r\nPING\r\n$5\r\nhello\r\n";
    assert_eq!(
        parse_command(buf),
        ParseOutcome::Complete {
            args: args(&["PING", "hello"]),
            consumed: buf.len(),
        }
    );
}

#[test]
fn parse_set_with_options() {
    let buf = b"*5\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\nv\r\n$2\r\nEX\r\n$2\r\n10\r\n";
    assert_eq!(
        parse_command(buf),
        ParseOutcome::Complete {
            args: args(&["SET", "k", "v", "EX", "10"]),
            consumed: buf.len(),
        }
    );
}

#[test]
fn parse_empty_value_bulk_string() {
    // SET k "" -- an empty bulk string argument must round-trip as an
    // empty (not missing) element.
    let buf = b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$0\r\n\r\n";
    assert_eq!(
        parse_command(buf),
        ParseOutcome::Complete {
            args: args(&["SET", "k", ""]),
            consumed: buf.len(),
        }
    );
}

#[test]
fn parse_incomplete_count_line() {
    assert_eq!(parse_command(b"*2\r\n$4\r\nPING"), ParseOutcome::Incomplete);
    assert_eq!(parse_command(b"*2\r"), ParseOutcome::Incomplete);
    assert_eq!(parse_command(b"*"), ParseOutcome::Incomplete);
    assert_eq!(parse_command(b""), ParseOutcome::Incomplete);
}

#[test]
fn parse_incomplete_mid_bulk_data() {
    // declared length 5 but only 3 bytes of data present so far
    assert_eq!(parse_command(b"*1\r\n$5\r\nhel"), ParseOutcome::Incomplete);
}

#[test]
fn parse_pipelined_commands_only_consumes_one() {
    let buf = b"*1\r\n$4\r\nPING\r\n*1\r\n$4\r\nPING\r\n";
    match parse_command(buf) {
        ParseOutcome::Complete { args: a, consumed } => {
            assert_eq!(a, args(&["PING"]));
            assert_eq!(consumed, 14); // exactly the first command's bytes
            assert_eq!(&buf[consumed..], b"*1\r\n$4\r\nPING\r\n");
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn parse_rejects_negative_multibulk_length() {
    assert!(matches!(parse_command(b"*-5\r\n"), ParseOutcome::Invalid(_)));
}

#[test]
fn parse_rejects_negative_bulk_length() {
    assert!(matches!(
        parse_command(b"*1\r\n$-5\r\n"),
        ParseOutcome::Invalid(_)
    ));
}

#[test]
fn parse_rejects_absurd_multibulk_length_without_allocating() {
    // Must reject based on the declared count alone, never try to
    // pre-allocate/read that many elements. This is the textbook
    // fuzz-crash / DoS surface the skill's traps section calls out.
    assert!(matches!(
        parse_command(b"*99999999999999\r\n"),
        ParseOutcome::Invalid(_)
    ));
}

#[test]
fn parse_rejects_absurd_bulk_length_without_allocating() {
    assert!(matches!(
        parse_command(b"*1\r\n$99999999999999\r\n"),
        ParseOutcome::Invalid(_)
    ));
}

#[test]
fn parse_rejects_missing_dollar_sigil() {
    assert!(matches!(
        parse_command(b"*1\r\nPING\r\n"),
        ParseOutcome::Invalid(_)
    ));
}

#[test]
fn parse_rejects_missing_trailing_crlf_after_bulk_data() {
    assert!(matches!(
        parse_command(b"*1\r\n$4\r\nPINGXX"),
        ParseOutcome::Invalid(_)
    ));
}

// --- inline commands (skill §2) ---

#[test]
fn parse_inline_ping() {
    assert_eq!(
        parse_command(b"PING\r\n"),
        ParseOutcome::Complete {
            args: args(&["PING"]),
            consumed: 6,
        }
    );
}

#[test]
fn parse_inline_exists_spec_example() {
    // the RESP2 spec's own inline-command example
    assert_eq!(
        parse_command(b"EXISTS somekey\r\n"),
        ParseOutcome::Complete {
            args: args(&["EXISTS", "somekey"]),
            consumed: 16,
        }
    );
}

#[test]
fn parse_inline_collapses_repeated_spaces() {
    assert_eq!(
        parse_command(b"GET   k\r\n"),
        ParseOutcome::Complete {
            args: args(&["GET", "k"]),
            consumed: 9,
        }
    );
}

#[test]
fn parse_inline_blank_line_is_empty_args() {
    // Redis treats an empty inline command as a no-op, not an error.
    assert_eq!(
        parse_command(b"\r\n"),
        ParseOutcome::Complete {
            args: vec![],
            consumed: 2,
        }
    );
}

#[test]
fn parse_inline_incomplete_no_crlf_yet() {
    assert_eq!(parse_command(b"PING"), ParseOutcome::Incomplete);
}

#[test]
fn empty_array_command_is_empty_args() {
    assert_eq!(
        parse_command(b"*0\r\n"),
        ParseOutcome::Complete {
            args: vec![],
            consumed: 4,
        }
    );
}
