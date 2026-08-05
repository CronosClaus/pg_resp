use super::*;

fn av(strs: &[&str]) -> Vec<Vec<u8>> {
    strs.iter().map(|s| s.as_bytes().to_vec()).collect()
}

fn now() -> (SystemTime, Instant) {
    (SystemTime::now(), Instant::now())
}

fn run(store: &mut Store, args: &[&str]) -> Reply {
    let (sys_now, mono_now) = now();
    let mut conn = ConnState::default();
    dispatch(store, sys_now, mono_now, &av(args), &mut conn, None, 0)
}

fn run_with_auth(
    store: &mut Store,
    conn: &mut ConnState,
    password: Option<&[u8]>,
    args: &[&str],
) -> Reply {
    let (sys_now, mono_now) = now();
    dispatch(store, sys_now, mono_now, &av(args), conn, password, 0)
}

#[test]
fn ping_bare() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["PING"]), Reply::simple("PONG"));
}

#[test]
fn hello_bare_returns_resp2_array_not_error() {
    let mut s = Store::new();
    assert!(matches!(run(&mut s, &["HELLO"]), Reply::Array(Some(_))));
}

#[test]
fn hello_2_returns_resp2_array_not_error() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["HELLO", "2"]),
        Reply::Array(Some(_))
    ));
}

#[test]
fn hello_3_returns_unknown_command_not_noproto() {
    // Empirically required (see dispatch.rs's HELLO comment): redis-py
    // defaults to requesting HELLO 3 and tolerates "unknown command" as a
    // signal to fall back to RESP2, but treats a spec-correct NOPROTO reply
    // as fatal. Matching real client behavior over spec purism here.
    let mut s = Store::new();
    match run(&mut s, &["HELLO", "3"]) {
        Reply::Error(msg) => {
            assert!(String::from_utf8_lossy(&msg).contains("unknown command"));
            assert!(!String::from_utf8_lossy(&msg).contains("NOPROTO"));
        }
        other => panic!("expected unknown-command-style error, got {other:?}"),
    }
}

#[test]
fn ping_with_message_echoes_as_bulk() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["PING", "hello"]), Reply::bulk(&b"hello"[..]));
}

#[test]
fn ping_too_many_args_errors() {
    let mut s = Store::new();
    assert!(matches!(run(&mut s, &["PING", "a", "b"]), Reply::Error(_)));
}

#[test]
fn echo() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["ECHO", "hi"]), Reply::bulk(&b"hi"[..]));
}

#[test]
fn get_missing_is_nil() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::nil());
}

#[test]
fn set_then_get() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["SET", "k", "v"]), Reply::ok());
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b"v"[..]));
}

#[test]
fn set_empty_value_is_empty_not_nil() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", ""]);
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b""[..]));
}

#[test]
fn set_nx_on_existing_returns_nil_not_error() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v1"]);
    assert_eq!(run(&mut s, &["SET", "k", "v2", "NX"]), Reply::nil());
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b"v1"[..]));
}

#[test]
fn set_xx_on_missing_returns_nil() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["SET", "k", "v", "XX"]), Reply::nil());
}

#[test]
fn set_nx_and_xx_together_is_syntax_error() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["SET", "k", "v", "NX", "XX"]),
        Reply::Error(_)
    ));
}

#[test]
fn set_ex_and_px_together_is_syntax_error() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["SET", "k", "v", "EX", "10", "PX", "5000"]),
        Reply::Error(_)
    ));
}

#[test]
fn set_get_option_returns_old_value_and_still_sets() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "old"]);
    assert_eq!(
        run(&mut s, &["SET", "k", "new", "GET"]),
        Reply::bulk(&b"old"[..])
    );
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b"new"[..]));
}

#[test]
fn set_ex_then_ttl() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v", "EX", "10"]);
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(10));
}

#[test]
fn set_keepttl_preserves_ttl() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v1", "EX", "10"]);
    run(&mut s, &["SET", "k", "v2", "KEEPTTL"]);
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(10));
}

#[test]
fn set_zero_expire_is_invalid_expire_error() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["SET", "k", "v", "EX", "0"]),
        Reply::Error(_)
    ));
}

#[test]
fn del_counts_existing_only() {
    let mut s = Store::new();
    run(&mut s, &["SET", "a", "1"]);
    assert_eq!(run(&mut s, &["DEL", "a", "missing"]), Reply::Integer(1));
}

#[test]
fn exists_counts_repeats() {
    let mut s = Store::new();
    run(&mut s, &["SET", "x", "1"]);
    assert_eq!(run(&mut s, &["EXISTS", "x", "x"]), Reply::Integer(2));
}

#[test]
fn ttl_missing_is_neg2_no_expiry_is_neg1() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["TTL", "missing"]), Reply::Integer(-2));
    run(&mut s, &["SET", "k", "v"]);
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(-1));
}

#[test]
fn expire_missing_key_returns_zero() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["EXPIRE", "missing", "10"]), Reply::Integer(0));
}

#[test]
fn expire_existing_key_returns_one() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v"]);
    assert_eq!(run(&mut s, &["EXPIRE", "k", "10"]), Reply::Integer(1));
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(10));
}

#[test]
fn incr_on_missing_key_starts_at_one() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["INCR", "n"]), Reply::Integer(1));
    assert_eq!(run(&mut s, &["INCR", "n"]), Reply::Integer(2));
}

#[test]
fn incr_on_non_integer_errors() {
    let mut s = Store::new();
    run(&mut s, &["SET", "n", "notanumber"]);
    assert!(matches!(run(&mut s, &["INCR", "n"]), Reply::Error(_)));
}

#[test]
fn decr_and_incrby_decrby() {
    let mut s = Store::new();
    run(&mut s, &["SET", "n", "10"]);
    assert_eq!(run(&mut s, &["DECR", "n"]), Reply::Integer(9));
    assert_eq!(run(&mut s, &["INCRBY", "n", "5"]), Reply::Integer(14));
    assert_eq!(run(&mut s, &["DECRBY", "n", "4"]), Reply::Integer(10));
}

#[test]
fn mget_mixed_missing() {
    let mut s = Store::new();
    run(&mut s, &["MSET", "k1", "v1", "k2", "v2"]);
    assert_eq!(
        run(&mut s, &["MGET", "k1", "missing", "k2"]),
        Reply::Array(Some(vec![
            Reply::bulk(&b"v1"[..]),
            Reply::nil(),
            Reply::bulk(&b"v2"[..]),
        ]))
    );
}

#[test]
fn mset_odd_args_errors() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["MSET", "k1", "v1", "k2"]),
        Reply::Error(_)
    ));
}

#[test]
fn unknown_command_is_well_formed_error_never_panics() {
    let mut s = Store::new();
    assert!(matches!(run(&mut s, &["FOOX"]), Reply::Error(_)));
}

#[test]
fn wrong_arity_errors_do_not_panic_on_missing_args() {
    let mut s = Store::new();
    assert!(matches!(run(&mut s, &["SET", "k"]), Reply::Error(_)));
    assert!(matches!(run(&mut s, &["GET"]), Reply::Error(_)));
    assert!(matches!(run(&mut s, &["EXPIRE", "k"]), Reply::Error(_)));
}

// --- T1 ---

#[test]
fn select_zero_ok_other_db_errors() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["SELECT", "0"]), Reply::ok());
    assert!(matches!(run(&mut s, &["SELECT", "1"]), Reply::Error(_)));
}

#[test]
fn dbsize_reflects_key_count() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["DBSIZE"]), Reply::Integer(0));
    run(&mut s, &["SET", "a", "1"]);
    run(&mut s, &["SET", "b", "2"]);
    assert_eq!(run(&mut s, &["DBSIZE"]), Reply::Integer(2));
}

#[test]
fn flushdb_clears_everything() {
    let mut s = Store::new();
    run(&mut s, &["SET", "a", "1"]);
    assert_eq!(run(&mut s, &["FLUSHDB"]), Reply::ok());
    assert_eq!(run(&mut s, &["DBSIZE"]), Reply::Integer(0));
}

#[test]
fn info_returns_bulk_string_with_expected_fields() {
    let mut s = Store::new();
    run(&mut s, &["SET", "a", "1"]);
    run(&mut s, &["GET", "a"]);
    run(&mut s, &["GET", "missing"]);
    match run(&mut s, &["INFO"]) {
        Reply::Bulk(Some(body)) => {
            let text = String::from_utf8_lossy(&body);
            assert!(text.contains("keyspace_hits:1"));
            assert!(text.contains("keyspace_misses:1"));
            assert!(text.contains("db0:keys=1"));
        }
        other => panic!("expected bulk string, got {other:?}"),
    }
}

#[test]
fn scan_full_iteration_finds_every_key() {
    let mut s = Store::new();
    for i in 0..25 {
        run(&mut s, &["SET", &format!("k{i}"), "v"]);
    }
    let mut found = Vec::new();
    let mut cursor = "0".to_string();
    loop {
        match run(&mut s, &["SCAN", &cursor, "COUNT", "7"]) {
            Reply::Array(Some(items)) => {
                let next = match &items[0] {
                    Reply::Bulk(Some(b)) => String::from_utf8_lossy(b).to_string(),
                    other => panic!("expected bulk cursor, got {other:?}"),
                };
                if let Reply::Array(Some(keys)) = &items[1] {
                    for k in keys {
                        if let Reply::Bulk(Some(b)) = k {
                            found.push(String::from_utf8_lossy(b).to_string());
                        }
                    }
                }
                cursor = next;
            }
            other => panic!("expected array reply, got {other:?}"),
        }
        if cursor == "0" {
            break;
        }
    }
    found.sort();
    let mut expected: Vec<String> = (0..25).map(|i| format!("k{i}")).collect();
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn scan_match_filters_by_glob_pattern() {
    let mut s = Store::new();
    run(&mut s, &["SET", "user:1", "a"]);
    run(&mut s, &["SET", "user:2", "b"]);
    run(&mut s, &["SET", "other", "c"]);
    match run(&mut s, &["SCAN", "0", "MATCH", "user:*", "COUNT", "100"]) {
        Reply::Array(Some(items)) => {
            if let Reply::Array(Some(keys)) = &items[1] {
                assert_eq!(keys.len(), 2);
            } else {
                panic!("expected keys array");
            }
        }
        other => panic!("expected array reply, got {other:?}"),
    }
}

#[test]
fn client_setinfo_and_setname_are_ok_getname_is_empty_bulk() {
    let mut s = Store::new();
    assert_eq!(
        run(&mut s, &["CLIENT", "SETINFO", "lib-name", "x"]),
        Reply::ok()
    );
    assert_eq!(run(&mut s, &["CLIENT", "SETNAME", "conn1"]), Reply::ok());
    assert_eq!(run(&mut s, &["CLIENT", "GETNAME"]), Reply::bulk(""));
}

#[test]
fn command_stub_never_errors() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["COMMAND"]), Reply::Array(Some(vec![])));
    assert_eq!(run(&mut s, &["COMMAND", "COUNT"]), Reply::Integer(0));
    assert_eq!(
        run(&mut s, &["COMMAND", "DOCS"]),
        Reply::Array(Some(vec![]))
    );
}

#[test]
fn quit_returns_ok() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["QUIT"]), Reply::ok());
}

#[test]
fn auth_with_no_password_configured_errors() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    assert!(matches!(
        run_with_auth(&mut s, &mut conn, None, &["AUTH", "anything"]),
        Reply::Error(_)
    ));
}

#[test]
fn auth_wrong_password_errors_and_does_not_authenticate() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    let reply = run_with_auth(&mut s, &mut conn, Some(b"secret"), &["AUTH", "wrong"]);
    assert!(matches!(reply, Reply::Error(_)));
    assert!(!conn.authenticated);
}

#[test]
fn auth_correct_password_succeeds_and_authenticates() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    let reply = run_with_auth(&mut s, &mut conn, Some(b"secret"), &["AUTH", "secret"]);
    assert_eq!(reply, Reply::ok());
    assert!(conn.authenticated);
}

#[test]
fn noauth_gate_blocks_commands_until_authenticated() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    let reply = run_with_auth(&mut s, &mut conn, Some(b"secret"), &["GET", "k"]);
    match reply {
        Reply::Error(msg) => assert!(String::from_utf8_lossy(&msg).starts_with("NOAUTH")),
        other => panic!("expected NOAUTH error, got {other:?}"),
    }
}

#[test]
fn noauth_gate_allows_auth_hello_quit_through() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    assert!(matches!(
        run_with_auth(&mut s, &mut conn, Some(b"secret"), &["HELLO"]),
        Reply::Array(Some(_))
    ));
    assert!(matches!(
        run_with_auth(&mut s, &mut conn, Some(b"secret"), &["QUIT"]),
        Reply::Simple(_)
    ));
}

#[test]
fn after_authenticating_commands_proceed_normally() {
    let mut s = Store::new();
    let mut conn = ConnState::default();
    run_with_auth(&mut s, &mut conn, Some(b"secret"), &["AUTH", "secret"]);
    assert_eq!(
        run_with_auth(&mut s, &mut conn, Some(b"secret"), &["SET", "k", "v"]),
        Reply::ok()
    );
}

// --- T2 ---

#[test]
fn setex_sets_value_and_ttl() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["SETEX", "k", "10", "v"]), Reply::ok());
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b"v"[..]));
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(10));
}

#[test]
fn setex_zero_seconds_errors() {
    let mut s = Store::new();
    assert!(matches!(
        run(&mut s, &["SETEX", "k", "0", "v"]),
        Reply::Error(_)
    ));
}

#[test]
fn setnx_returns_integer_not_ok_or_nil() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["SETNX", "k", "v"]), Reply::Integer(1));
    assert_eq!(run(&mut s, &["SETNX", "k", "v2"]), Reply::Integer(0));
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::bulk(&b"v"[..]));
}

#[test]
fn getdel_returns_value_and_removes_key() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v"]);
    assert_eq!(run(&mut s, &["GETDEL", "k"]), Reply::bulk(&b"v"[..]));
    assert_eq!(run(&mut s, &["GET", "k"]), Reply::nil());
}

#[test]
fn getex_with_no_options_leaves_ttl_untouched() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v", "EX", "10"]);
    assert_eq!(run(&mut s, &["GETEX", "k"]), Reply::bulk(&b"v"[..]));
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(10));
}

#[test]
fn getex_persist_clears_ttl() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v", "EX", "10"]);
    assert_eq!(
        run(&mut s, &["GETEX", "k", "PERSIST"]),
        Reply::bulk(&b"v"[..])
    );
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(-1));
}

#[test]
fn getex_ex_sets_new_ttl() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v"]);
    assert_eq!(
        run(&mut s, &["GETEX", "k", "EX", "30"]),
        Reply::bulk(&b"v"[..])
    );
    assert_eq!(run(&mut s, &["TTL", "k"]), Reply::Integer(30));
}

#[test]
fn persist_removes_ttl_returns_integer() {
    let mut s = Store::new();
    run(&mut s, &["SET", "k", "v", "EX", "10"]);
    assert_eq!(run(&mut s, &["PERSIST", "k"]), Reply::Integer(1));
    assert_eq!(run(&mut s, &["PERSIST", "k"]), Reply::Integer(0));
}

#[test]
fn type_returns_string_or_none() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["TYPE", "missing"]), Reply::simple("none"));
    run(&mut s, &["SET", "k", "v"]);
    assert_eq!(run(&mut s, &["TYPE", "k"]), Reply::simple("string"));
}

#[test]
fn randomkey_nil_on_empty_some_key_otherwise() {
    let mut s = Store::new();
    assert_eq!(run(&mut s, &["RANDOMKEY"]), Reply::nil());
    run(&mut s, &["SET", "only", "v"]);
    assert_eq!(run(&mut s, &["RANDOMKEY"]), Reply::bulk(&b"only"[..]));
}

#[test]
fn keys_matches_glob_pattern() {
    let mut s = Store::new();
    run(&mut s, &["SET", "user:1", "a"]);
    run(&mut s, &["SET", "user:2", "b"]);
    run(&mut s, &["SET", "other", "c"]);
    match run(&mut s, &["KEYS", "user:*"]) {
        Reply::Array(Some(items)) => assert_eq!(items.len(), 2),
        other => panic!("expected array reply, got {other:?}"),
    }
}
