#![no_main]

use libfuzzer_sys::fuzz_target;
use resp_proto::{parse_reply, ReplyOutcome};

// Drives the reply parser exactly like the loopback client's read buffer
// will: keep parsing complete replies off the front until the parser reports
// Incomplete (needs more bytes than this fixed fuzz input has) or Invalid
// (parser correctly rejects, caller would close the connection).
//
// Why this target exists at all, given the parser only ever reads bytes
// pg_resp's own server thread wrote: because it runs *inside a Postgres
// backend process*. parse_command's blast radius is one dropped client
// connection behind a panic fence; a crash or unbounded allocation here takes
// down a backend serving real SQL. That makes this the higher-consequence of
// the two parsers even though it faces the friendlier input.
fuzz_target!(|data: &[u8]| {
    let mut buf = data;
    loop {
        match parse_reply(buf) {
            ReplyOutcome::Complete { consumed, .. } => {
                assert!(
                    consumed > 0 && consumed <= buf.len(),
                    "parser must always make forward progress on Complete"
                );
                buf = &buf[consumed..];
                if buf.is_empty() {
                    break;
                }
            }
            ReplyOutcome::Incomplete | ReplyOutcome::Invalid(_) => break,
        }
    }
});
