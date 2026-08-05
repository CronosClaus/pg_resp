use super::*;

fn av(strs: &[&str]) -> Vec<Vec<u8>> {
    strs.iter().map(|s| s.as_bytes().to_vec()).collect()
}

fn now() -> (SystemTime, Instant) {
    (SystemTime::now(), Instant::now())
}

fn run(store: &mut Store, args: &[&str]) -> Reply {
    let (sys_now, mono_now) = now();
    dispatch(store, sys_now, mono_now, &av(args))
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
    assert!(matches!(run(&mut s, &["HELLO", "2"]), Reply::Array(Some(_))));
}

#[test]
fn hello_3_returns_noproto_error() {
    let mut s = Store::new();
    match run(&mut s, &["HELLO", "3"]) {
        Reply::Error(msg) => assert!(String::from_utf8_lossy(&msg).starts_with("NOPROTO")),
        other => panic!("expected NOPROTO error, got {other:?}"),
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
