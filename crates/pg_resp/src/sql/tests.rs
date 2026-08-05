//! Unit tests for the pure, PG-free parts of the SQL surface.
//!
//! Everything else in this module is only meaningful against a live instance
//! with a running bgworker, and is tested by `tests/sql_surface/gates.py` —
//! including both bible §5 Phase 3 gates. See that script's header for why a
//! `#[pg_test]` cannot cover a feature whose whole substance is crossing the
//! backend/bgworker process boundary.

use super::parse_info_field;

/// A realistic INFO payload, byte-for-byte the shape `dispatch.rs` emits.
const INFO: &str = "# Server\r\n\
                    redis_version:7.0.0\r\n\
                    pg_resp_version:0.0.0\r\n\
                    # Memory\r\n\
                    used_memory:12345\r\n\
                    maxmemory:268435456\r\n\
                    # Stats\r\n\
                    keyspace_hits:42\r\n\
                    keyspace_misses:7\r\n\
                    evicted_keys:0\r\n\
                    invalidations_lost:3\r\n\
                    # Keyspace\r\n\
                    db0:keys=9\r\n";

#[test]
fn parses_plain_colon_fields() {
    assert_eq!(parse_info_field(INFO, "used_memory"), Some(12345));
    assert_eq!(parse_info_field(INFO, "maxmemory"), Some(268435456));
    assert_eq!(parse_info_field(INFO, "keyspace_hits"), Some(42));
    assert_eq!(parse_info_field(INFO, "keyspace_misses"), Some(7));
    assert_eq!(parse_info_field(INFO, "evicted_keys"), Some(0));
    assert_eq!(parse_info_field(INFO, "invalidations_lost"), Some(3));
}

#[test]
fn parses_the_keyspace_sections_equals_shape() {
    // `db0:keys=9` is the one field that is not `name:value` — the reason this
    // parser is hand-rolled instead of a split on ':'.
    assert_eq!(parse_info_field(INFO, "db0:keys"), Some(9));
}

#[test]
fn missing_field_is_none_not_zero() {
    // The distinction matters: resp.stats() maps None to 0 for display, but a
    // parser that cannot tell "absent" from "zero" would silently report a
    // healthy 0 for a field that INFO stopped emitting — exactly the kind of
    // drift the G4 gate exists to catch.
    assert_eq!(parse_info_field(INFO, "no_such_field"), None);
}

#[test]
fn section_headers_are_not_fields() {
    assert_eq!(parse_info_field(INFO, "# Memory"), None);
}

#[test]
fn non_numeric_value_is_none() {
    // redis_version is a real field with a non-integer value; asking for it as
    // an integer must fail rather than parse a prefix.
    assert_eq!(parse_info_field(INFO, "redis_version"), None);
}

#[test]
fn tolerates_lf_only_line_endings() {
    // INFO is CRLF-framed on the wire, but being strict about it here would
    // make the parser fragile for no benefit.
    let lf = "used_memory:99\nkeyspace_hits:1\n";
    assert_eq!(parse_info_field(lf, "used_memory"), Some(99));
    assert_eq!(parse_info_field(lf, "keyspace_hits"), Some(1));
}

#[test]
fn prefix_collision_does_not_win() {
    // `keyspace_hits` must not be answered by the `keyspace_hits_ratio` line
    // that happens to start with the same text. The scan strips only ':'/'='
    // after the name, so a longer name leaves a non-numeric remainder and is
    // correctly skipped.
    let info = "keyspace_hits_ratio:0.86\r\nkeyspace_hits:5\r\n";
    assert_eq!(parse_info_field(info, "keyspace_hits"), Some(5));
}
