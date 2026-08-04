#![no_main]

use libfuzzer_sys::fuzz_target;
use resp_proto::{parse_command, ParseOutcome};

// Drives the parser exactly like the real event loop's read buffer will:
// keep parsing complete commands off the front until the parser reports
// Incomplete (needs more bytes than this fixed fuzz input has) or Invalid
// (parser correctly rejects, caller would close the connection). Bible §5
// Phase 1 gate: >=1 CPU-hour, zero crashes/UB.
fuzz_target!(|data: &[u8]| {
    let mut buf = data;
    loop {
        match parse_command(buf) {
            ParseOutcome::Complete { consumed, .. } => {
                assert!(
                    consumed > 0 && consumed <= buf.len(),
                    "parser must always make forward progress on Complete"
                );
                buf = &buf[consumed..];
                if buf.is_empty() {
                    break;
                }
            }
            ParseOutcome::Incomplete | ParseOutcome::Invalid(_) => break,
        }
    }
});
