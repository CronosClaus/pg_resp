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
    assert!(matches!(
        parse_command(b"*-5\r\n"),
        ParseOutcome::Invalid(_)
    ));
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

// --- Client side (Phase 3): encode_command + parse_reply ---
//
// The loopback-RESP SQL surface (bible D3) makes this crate parse bytes from
// BOTH ends of the socket. The vectors below are the response halves of the
// resp-protocol skill §7 table — the same bytes the server-side tests above
// assert we *emit*, now asserted as bytes we correctly *read back*.

fn complete(reply: Reply, consumed: usize) -> ReplyOutcome {
    ReplyOutcome::Complete { reply, consumed }
}

#[test]
fn encode_command_shape() {
    // resp-protocol skill §7 vector 2: the wire form of PING.
    assert_eq!(encode_command(&[b"PING"]), b"*1\r\n$4\r\nPING\r\n");
    assert_eq!(
        encode_command(&[b"GET", b"k"]),
        b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n"
    );
}

#[test]
fn encode_command_is_binary_safe() {
    // Phase 2's adversarial deck proved the server handles embedded CRLF and
    // NUL in keys/values; the length prefix is what makes that safe, so the
    // encoder must never rely on the payload being text.
    let value: &[u8] = b"a\r\nb\0c";
    let encoded = encode_command(&[b"SET", b"k", value]);
    assert_eq!(
        encoded,
        b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$6\r\na\r\nb\0c\r\n"
    );
    // And it must survive a round trip through the server-side parser.
    match parse_command(&encoded) {
        ParseOutcome::Complete { args, consumed } => {
            assert_eq!(consumed, encoded.len());
            assert_eq!(args, vec![b"SET".to_vec(), b"k".to_vec(), value.to_vec()]);
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn encode_command_empty_arg() {
    assert_eq!(
        encode_command(&[b"SET", b"k", b""]),
        b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$0\r\n\r\n"
    );
}

#[test]
fn parse_reply_simple_and_error() {
    assert_eq!(
        parse_reply(b"+OK\r\n"),
        complete(Reply::Simple(b"OK".to_vec()), 5)
    );
    assert_eq!(
        parse_reply(b"+PONG\r\n"),
        complete(Reply::Simple(b"PONG".to_vec()), 7)
    );
    // Skill §7 vector 5 / §3: unknown command's exact error text.
    assert_eq!(
        parse_reply(b"-ERR unknown command 'FOOX'\r\n"),
        complete(Reply::Error(b"ERR unknown command 'FOOX'".to_vec()), 29)
    );
    // NOAUTH matters to the client specifically: it proves the server is
    // alive and speaking RESP even though it refused the command, which is
    // what the S6 watchdog keys off.
    assert_eq!(
        parse_reply(b"-NOAUTH Authentication required.\r\n"),
        complete(
            Reply::Error(b"NOAUTH Authentication required.".to_vec()),
            34
        )
    );
}

#[test]
fn parse_reply_integers() {
    assert_eq!(parse_reply(b":0\r\n"), complete(Reply::Integer(0), 4));
    assert_eq!(parse_reply(b":11\r\n"), complete(Reply::Integer(11), 5));
    // Skill §5: missing key is -2, key-with-no-expiry is -1. The pair whose
    // ordering is easy to transpose from memory, so both directions are here.
    assert_eq!(parse_reply(b":-1\r\n"), complete(Reply::Integer(-1), 5));
    assert_eq!(parse_reply(b":-2\r\n"), complete(Reply::Integer(-2), 5));
    // Spec permits an explicit leading '+' on integers.
    assert_eq!(parse_reply(b":+5\r\n"), complete(Reply::Integer(5), 5));
    assert_eq!(
        parse_reply(b":9223372036854775807\r\n"),
        complete(Reply::Integer(i64::MAX), 22)
    );
    assert_eq!(
        parse_reply(b":-9223372036854775808\r\n"),
        complete(Reply::Integer(i64::MIN), 23)
    );
}

#[test]
fn parse_reply_integer_overflow_is_invalid() {
    // One past i64::MAX: must be rejected, not silently wrapped.
    assert!(matches!(
        parse_reply(b":9223372036854775808\r\n"),
        ReplyOutcome::Invalid(_)
    ));
    assert!(matches!(parse_reply(b":\r\n"), ReplyOutcome::Invalid(_)));
    assert!(matches!(parse_reply(b":abc\r\n"), ReplyOutcome::Invalid(_)));
}

#[test]
fn parse_reply_null_vs_empty_bulk() {
    // Skill §1/§8's #1 correctness trap, from the reading side this time:
    // $-1 (missing) and $0 (present but empty) must not collapse together.
    assert_eq!(parse_reply(b"$-1\r\n"), complete(Reply::Bulk(None), 5));
    assert_eq!(
        parse_reply(b"$0\r\n\r\n"),
        complete(Reply::Bulk(Some(vec![])), 6)
    );
    assert_ne!(parse_reply(b"$-1\r\n"), parse_reply(b"$0\r\n\r\n"));
}

#[test]
fn parse_reply_bulk_with_embedded_crlf() {
    // The length prefix, not a delimiter scan, is what makes bulk data
    // binary-safe — a payload containing CRLF must not terminate early.
    assert_eq!(
        parse_reply(b"$6\r\na\r\nb\0c\r\n"),
        complete(Reply::Bulk(Some(b"a\r\nb\0c".to_vec())), 12)
    );
}

#[test]
fn parse_reply_bad_bulk_length() {
    // $-2 is malformed, not "another kind of null".
    assert!(matches!(parse_reply(b"$-2\r\n"), ReplyOutcome::Invalid(_)));
    assert!(matches!(
        parse_reply(b"$abc\r\nxx\r\n"),
        ReplyOutcome::Invalid(_)
    ));
    // Length prefix beyond the 512MB cap is rejected at parse time, before
    // anything is allocated.
    assert!(matches!(
        parse_reply(b"$536870913\r\n",),
        ReplyOutcome::Invalid(_)
    ));
    // Correct length but the trailing CRLF is missing.
    assert!(matches!(
        parse_reply(b"$1\r\nvXX"),
        ReplyOutcome::Invalid(_)
    ));
}

#[test]
fn parse_reply_arrays() {
    // Skill §7 vector 36: MGET's shape, with a nil hole in the middle that
    // must stay in place rather than being compacted out.
    assert_eq!(
        parse_reply(b"*3\r\n$2\r\nv1\r\n$-1\r\n$2\r\nv2\r\n"),
        complete(
            Reply::Array(Some(vec![
                Reply::Bulk(Some(b"v1".to_vec())),
                Reply::Bulk(None),
                Reply::Bulk(Some(b"v2".to_vec())),
            ])),
            25
        )
    );
    assert_eq!(
        parse_reply(b"*0\r\n"),
        complete(Reply::Array(Some(vec![])), 4)
    );
    assert_eq!(parse_reply(b"*-1\r\n"), complete(Reply::Array(None), 5));
    assert_ne!(parse_reply(b"*-1\r\n"), parse_reply(b"*0\r\n"));
}

#[test]
fn parse_reply_scan_shape() {
    // SCAN is pg_resp's only genuinely nested reply: [cursor, [keys...]].
    // This is the shape resp.keys()/resp.stats() will read back.
    assert_eq!(
        parse_reply(b"*2\r\n$1\r\n0\r\n*2\r\n$2\r\nk1\r\n$2\r\nk2\r\n"),
        complete(
            Reply::Array(Some(vec![
                Reply::Bulk(Some(b"0".to_vec())),
                Reply::Array(Some(vec![
                    Reply::Bulk(Some(b"k1".to_vec())),
                    Reply::Bulk(Some(b"k2".to_vec())),
                ])),
            ])),
            31
        )
    );
}

#[test]
fn parse_reply_rejects_resp3_sigils() {
    // pg_resp never negotiates RESP3 (skill §6/§8). A RESP3 sigil on this
    // socket means we are not talking to what we think we are — fail loudly
    // rather than resynchronize.
    for sigil in *b"_#,(!=%|~>" {
        let buf = [sigil, b'x', b'\r', b'\n'];
        assert!(
            matches!(parse_reply(&buf), ReplyOutcome::Invalid(_)),
            "RESP3 sigil {:?} must be rejected",
            sigil as char
        );
    }
}

#[test]
fn parse_reply_bounds_nesting_depth() {
    // An array that opens MAX_REPLY_DEPTH+2 levels deep must be rejected
    // rather than recursed into.
    let mut buf = Vec::new();
    for _ in 0..(MAX_REPLY_DEPTH + 2) {
        buf.extend_from_slice(b"*1\r\n");
    }
    buf.extend_from_slice(b"$1\r\nx\r\n");
    assert!(matches!(parse_reply(&buf), ReplyOutcome::Invalid(_)));

    // Just inside the limit still parses, so the bound isn't off by enough to
    // reject legitimate replies.
    let mut ok = Vec::new();
    for _ in 0..MAX_REPLY_DEPTH {
        ok.extend_from_slice(b"*1\r\n");
    }
    ok.extend_from_slice(b"$1\r\nx\r\n");
    assert!(matches!(parse_reply(&ok), ReplyOutcome::Complete { .. }));
}

#[test]
fn parse_reply_incomplete_at_every_prefix() {
    // The client's read loop depends on this: every strict prefix of a valid
    // reply must report Incomplete, never Invalid and never a bogus Complete.
    // A parser that returns Invalid on a half-arrived reply turns a slow
    // socket into a dropped connection.
    let vectors: &[&[u8]] = &[
        b"+OK\r\n",
        b"-ERR nope\r\n",
        b":12345\r\n",
        b"$-1\r\n",
        b"$0\r\n\r\n",
        b"$5\r\nhello\r\n",
        b"*3\r\n$2\r\nv1\r\n$-1\r\n$2\r\nv2\r\n",
        b"*2\r\n$1\r\n0\r\n*1\r\n$2\r\nk1\r\n",
    ];
    for full in vectors {
        for cut in 1..full.len() {
            assert_eq!(
                parse_reply(&full[..cut]),
                ReplyOutcome::Incomplete,
                "prefix of length {cut} of {:?} should be Incomplete",
                String::from_utf8_lossy(full)
            );
        }
        assert!(matches!(parse_reply(full), ReplyOutcome::Complete { .. }));
    }
}

#[test]
fn parse_reply_leaves_trailing_bytes_alone() {
    // Pipelined replies: parse one, report exactly what it consumed, leave
    // the rest for the next call.
    let buf = b"+OK\r\n:42\r\n$1\r\nv\r\n";
    let ReplyOutcome::Complete { reply, consumed } = parse_reply(buf) else {
        panic!("expected Complete");
    };
    assert_eq!(reply, Reply::Simple(b"OK".to_vec()));
    assert_eq!(consumed, 5);
    let ReplyOutcome::Complete { reply, consumed } = parse_reply(&buf[consumed..]) else {
        panic!("expected Complete");
    };
    assert_eq!(reply, Reply::Integer(42));
    assert_eq!(consumed, 5);
}

#[test]
fn parse_reply_empty_buffer_is_incomplete() {
    assert_eq!(parse_reply(b""), ReplyOutcome::Incomplete);
}

#[test]
fn reply_round_trips_through_its_own_parser() {
    // The invariant the whole loopback client rests on: anything the server
    // can serialize, the client can read back identically. Covers every
    // Reply variant including both null forms and a nested array.
    let cases = vec![
        Reply::ok(),
        Reply::Simple(b"PONG".to_vec()),
        Reply::error(&b"ERR wrong number of arguments for 'get' command"[..]),
        Reply::Integer(0),
        Reply::Integer(-2),
        Reply::Integer(i64::MIN),
        Reply::Integer(i64::MAX),
        Reply::Bulk(None),
        Reply::Bulk(Some(vec![])),
        Reply::Bulk(Some(b"hello".to_vec())),
        Reply::Bulk(Some(b"a\r\nb\0c".to_vec())),
        Reply::Bulk(Some((0u8..=255).collect())),
        Reply::Array(None),
        Reply::Array(Some(vec![])),
        Reply::Array(Some(vec![
            Reply::Bulk(Some(b"v1".to_vec())),
            Reply::Bulk(None),
            Reply::Integer(7),
            Reply::Array(Some(vec![Reply::Simple(b"nested".to_vec())])),
        ])),
    ];
    for case in cases {
        let bytes = case.to_bytes();
        assert_eq!(
            parse_reply(&bytes),
            complete(case.clone(), bytes.len()),
            "round trip failed for {case:?}"
        );
    }
}
