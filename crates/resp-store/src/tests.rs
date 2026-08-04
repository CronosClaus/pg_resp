use super::*;
use std::time::Duration;

fn t0() -> Instant {
    Instant::now()
}

#[test]
fn eviction_kicks_in_when_over_budget() {
    // Small enough budget that a handful of entries must trigger eviction.
    let mut s = Store::with_max_memory(PER_ENTRY_OVERHEAD_BYTES * 3);
    let now = t0();
    for i in 0..10 {
        s.set(
            now,
            format!("k{i}").as_bytes(),
            b"v".to_vec(),
            Expiry::None,
            Condition::None,
            false,
        );
    }
    assert!(s.len() < 10, "eviction should have kept the store under 10 entries");
    assert!(s.used_bytes() <= PER_ENTRY_OVERHEAD_BYTES * 4, "should stay near budget + one entry");
}

#[test]
fn eviction_never_evicts_the_key_just_written() {
    let mut s = Store::with_max_memory(PER_ENTRY_OVERHEAD_BYTES); // room for ~1 entry
    let now = t0();
    for i in 0..5 {
        s.set(
            now,
            format!("k{i}").as_bytes(),
            b"v".to_vec(),
            Expiry::None,
            Condition::None,
            false,
        );
        // the key just written must always still be readable immediately after
        assert_eq!(s.get(now, format!("k{i}").as_bytes()), Some(&b"v"[..]));
    }
}

#[test]
fn active_expire_sweep_removes_expired_and_leaves_live_keys() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"expiring",
        b"v".to_vec(),
        Expiry::At(now + Duration::from_millis(10)),
        Condition::None,
        false,
    );
    s.set(now, b"forever", b"v".to_vec(), Expiry::None, Condition::None, false);

    let later = now + Duration::from_millis(50);
    let removed = s.active_expire_sweep(later, 20);
    assert_eq!(removed, 1);
    assert_eq!(s.len(), 1);
    assert_eq!(s.get(later, b"forever"), Some(&b"v"[..]));
}

#[test]
fn get_missing_returns_none() {
    let mut s = Store::new();
    assert_eq!(s.get(t0(), b"k"), None);
}

#[test]
fn set_then_get() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"v".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.get(now, b"k"), Some(&b"v"[..]));
}

#[test]
fn set_without_expiry_clears_any_existing_ttl() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k",
        b"v1".to_vec(),
        Expiry::At(now + Duration::from_secs(10)),
        Condition::None,
        false,
    );
    assert_eq!(s.ttl_seconds(now, b"k"), 10);
    // Plain SET (Expiry::None) must clear the TTL.
    s.set(now, b"k", b"v2".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.ttl_seconds(now, b"k"), -1);
}

#[test]
fn ttl_missing_key_is_neg2() {
    let mut s = Store::new();
    assert_eq!(s.ttl_seconds(t0(), b"nope"), -2);
    assert_eq!(s.pttl_millis(t0(), b"nope"), -2);
}

#[test]
fn ttl_key_with_no_expiry_is_neg1() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"v".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.ttl_seconds(now, b"k"), -1);
    assert_eq!(s.pttl_millis(now, b"k"), -1);
}

#[test]
fn ttl_and_pttl_with_expiry() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k",
        b"v".to_vec(),
        Expiry::At(now + Duration::from_millis(10_000)),
        Condition::None,
        false,
    );
    assert_eq!(s.ttl_seconds(now, b"k"), 10);
    assert_eq!(s.pttl_millis(now, b"k"), 10_000);
}

#[test]
fn ttl_rounds_to_nearest_second_like_valkey() {
    // (ms + 500) / 1000, matching Valkey's expire.c idiom (resp-protocol
    // skill §5 / docs/refs/valkey-notes.md). Note this rounds 1ms remaining
    // down to 0, not up to 1 — a key can legitimately report TTL=0 (distinct
    // from -1 "no expiry" and -2 "missing") in the last half-second before
    // it lazily expires. Picked non-boundary values to avoid ambiguity.
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"a",
        b"v".to_vec(),
        Expiry::At(now + Duration::from_millis(400)),
        Condition::None,
        false,
    );
    assert_eq!(s.ttl_seconds(now, b"a"), 0);
    s.set(
        now,
        b"b",
        b"v".to_vec(),
        Expiry::At(now + Duration::from_millis(600)),
        Condition::None,
        false,
    );
    assert_eq!(s.ttl_seconds(now, b"b"), 1);
    assert_eq!(s.pttl_millis(now, b"b"), 600);
}

#[test]
fn lazy_expiry_removes_key_after_deadline() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k",
        b"v".to_vec(),
        Expiry::At(now + Duration::from_millis(100)),
        Condition::None,
        false,
    );
    let later = now + Duration::from_millis(200);
    assert_eq!(s.get(later, b"k"), None);
    assert_eq!(s.len(), 0, "expired key must be physically removed on access");
}

#[test]
fn set_nx_on_existing_is_noop() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"v1".to_vec(), Expiry::None, Condition::None, false);
    let outcome = s.set(
        now,
        b"k",
        b"v2".to_vec(),
        Expiry::None,
        Condition::IfNotExists,
        false,
    );
    assert!(!outcome.applied);
    assert_eq!(s.get(now, b"k"), Some(&b"v1"[..]));
}

#[test]
fn set_nx_on_missing_applies() {
    let mut s = Store::new();
    let now = t0();
    let outcome = s.set(
        now,
        b"k",
        b"v".to_vec(),
        Expiry::None,
        Condition::IfNotExists,
        false,
    );
    assert!(outcome.applied);
    assert_eq!(s.get(now, b"k"), Some(&b"v"[..]));
}

#[test]
fn set_xx_on_missing_is_noop() {
    let mut s = Store::new();
    let now = t0();
    let outcome = s.set(
        now,
        b"k",
        b"v".to_vec(),
        Expiry::None,
        Condition::IfExists,
        false,
    );
    assert!(!outcome.applied);
    assert_eq!(s.get(now, b"k"), None);
}

#[test]
fn set_xx_on_existing_applies() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"v1".to_vec(), Expiry::None, Condition::None, false);
    let outcome = s.set(
        now,
        b"k",
        b"v2".to_vec(),
        Expiry::None,
        Condition::IfExists,
        false,
    );
    assert!(outcome.applied);
    assert_eq!(s.get(now, b"k"), Some(&b"v2"[..]));
}

#[test]
fn set_get_option_returns_old_value_and_still_sets() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"old".to_vec(), Expiry::None, Condition::None, false);
    let outcome = s.set(now, b"k", b"new".to_vec(), Expiry::None, Condition::None, true);
    assert!(outcome.applied);
    assert_eq!(outcome.old_value, Some(b"old".to_vec()));
    assert_eq!(s.get(now, b"k"), Some(&b"new"[..]));
}

#[test]
fn set_get_option_on_missing_key_returns_none_and_still_sets() {
    let mut s = Store::new();
    let now = t0();
    let outcome = s.set(now, b"k", b"v".to_vec(), Expiry::None, Condition::None, true);
    assert!(outcome.applied);
    assert_eq!(outcome.old_value, None);
    assert_eq!(s.get(now, b"k"), Some(&b"v"[..]));
}

#[test]
fn set_keepttl_preserves_existing_expiry() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k",
        b"v1".to_vec(),
        Expiry::At(now + Duration::from_secs(10)),
        Condition::None,
        false,
    );
    s.set(now, b"k", b"v2".to_vec(), Expiry::KeepTtl, Condition::None, false);
    assert_eq!(s.ttl_seconds(now, b"k"), 10);
    assert_eq!(s.get(now, b"k"), Some(&b"v2"[..]));
}

#[test]
fn del_counts_only_existing_keys() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"a", b"1".to_vec(), Expiry::None, Condition::None, false);
    let count = s.del(now, &[b"a", b"missing"]);
    assert_eq!(count, 1);
    assert_eq!(s.get(now, b"a"), None);
}

#[test]
fn exists_counts_repeats() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"x", b"1".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.exists(now, &[b"x", b"x", b"x"]), 3);
    assert_eq!(s.exists(now, &[b"missing"]), 0);
}

#[test]
fn expire_on_missing_key_returns_false() {
    let mut s = Store::new();
    let now = t0();
    assert!(!s.expire(now, b"missing", now + Duration::from_secs(10)));
}

#[test]
fn expire_on_existing_key_sets_ttl_and_returns_true() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k", b"v".to_vec(), Expiry::None, Condition::None, false);
    assert!(s.expire(now, b"k", now + Duration::from_secs(10)));
    assert_eq!(s.ttl_seconds(now, b"k"), 10);
}

#[test]
fn incr_missing_key_starts_at_zero() {
    let mut s = Store::new();
    let now = t0();
    assert_eq!(s.incr_by(now, b"n", 1), Ok(1));
    assert_eq!(s.incr_by(now, b"n", 1), Ok(2));
}

#[test]
fn incr_on_existing_integer() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"n", b"10".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.incr_by(now, b"n", 1), Ok(11));
    assert_eq!(s.incr_by(now, b"n", 3), Ok(14));
}

#[test]
fn decr_is_incr_by_negative_delta() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"n", b"5".to_vec(), Expiry::None, Condition::None, false);
    assert_eq!(s.incr_by(now, b"n", -1), Ok(4));
}

#[test]
fn incr_on_non_integer_errors() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"n",
        b"notanumber".to_vec(),
        Expiry::None,
        Condition::None,
        false,
    );
    assert_eq!(s.incr_by(now, b"n", 1), Err(IncrError::NotAnInteger));
}

#[test]
fn incr_overflow_errors() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"n",
        i64::MAX.to_string().into_bytes(),
        Expiry::None,
        Condition::None,
        false,
    );
    assert_eq!(s.incr_by(now, b"n", 1), Err(IncrError::Overflow));
}

#[test]
fn incr_preserves_existing_ttl() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"n",
        b"1".to_vec(),
        Expiry::At(now + Duration::from_secs(10)),
        Condition::None,
        false,
    );
    s.incr_by(now, b"n", 1).unwrap();
    assert_eq!(s.ttl_seconds(now, b"n"), 10);
}

#[test]
fn mget_returns_nil_for_missing_keys_in_place() {
    let mut s = Store::new();
    let now = t0();
    s.set(now, b"k1", b"v1".to_vec(), Expiry::None, Condition::None, false);
    s.set(now, b"k2", b"v2".to_vec(), Expiry::None, Condition::None, false);
    let result = s.mget(now, &[b"k1", b"missing", b"k2"]);
    assert_eq!(
        result,
        vec![Some(b"v1".to_vec()), None, Some(b"v2".to_vec())]
    );
}

#[test]
fn mset_sets_multiple_keys_and_clears_ttl() {
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k1",
        b"old".to_vec(),
        Expiry::At(now + Duration::from_secs(10)),
        Condition::None,
        false,
    );
    s.mset(&[(&b"k1"[..], b"new1".to_vec()), (&b"k2"[..], b"new2".to_vec())]);
    assert_eq!(s.get(now, b"k1"), Some(&b"new1"[..]));
    assert_eq!(s.get(now, b"k2"), Some(&b"new2"[..]));
    assert_eq!(s.ttl_seconds(now, b"k1"), -1, "mset clears TTL like a plain SET");
}

#[test]
fn expired_key_treated_as_not_existing_for_nx() {
    // Bible §3.5: an expired-but-not-yet-swept key must be treated as "not
    // existing" for NX/XX checks (lazy-expiry-on-access semantics).
    let mut s = Store::new();
    let now = t0();
    s.set(
        now,
        b"k",
        b"v1".to_vec(),
        Expiry::At(now + Duration::from_millis(10)),
        Condition::None,
        false,
    );
    let later = now + Duration::from_millis(50);
    let outcome = s.set(
        later,
        b"k",
        b"v2".to_vec(),
        Expiry::None,
        Condition::IfNotExists,
        false,
    );
    assert!(outcome.applied, "expired key must not block NX");
    assert_eq!(s.get(later, b"k"), Some(&b"v2"[..]));
}
