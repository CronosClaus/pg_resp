//! Bible §5 Phase 2 property tests, done as fast-loop-only Stage C pre-work:
//! never exceeds max_memory + one entry; TTL'd keys never returned after
//! expiry; INCR stays monotonic under churn. No soak, no memtier, no pgrx —
//! pure Rust, per this run's Stage C scope.

use super::*;
use proptest::prelude::*;
use std::time::Duration;

fn t0() -> Instant {
    Instant::now()
}

proptest! {
    #[test]
    fn never_exceeds_budget_by_more_than_one_entry(
        ops in prop::collection::vec((0u32..50, 0usize..200), 1..300)
    ) {
        let budget: usize = 4096;
        let mut s = Store::with_max_memory(budget);
        let now = t0();
        let mut max_single_entry: usize = 0;

        for (key_id, value_len) in ops {
            let key = format!("k{key_id}").into_bytes();
            let value = vec![b'x'; value_len];
            max_single_entry = max_single_entry.max(entry_bytes(&key, value.len()));
            s.set(now, &key, value, Expiry::None, Condition::None, false);
            prop_assert!(
                s.used_bytes() <= budget + max_single_entry,
                "used_bytes={} exceeds budget({}) + one entry({})",
                s.used_bytes(),
                budget,
                max_single_entry
            );
        }
    }

    #[test]
    fn expired_keys_never_returned(
        ttls_ms in prop::collection::vec(1i64..1000, 1..50)
    ) {
        let mut s = Store::new();
        let now = t0();
        let keys: Vec<Vec<u8>> = (0..ttls_ms.len())
            .map(|i| format!("k{i}").into_bytes())
            .collect();

        for (key, ttl_ms) in keys.iter().zip(ttls_ms.iter()) {
            s.set(
                now,
                key,
                b"v".to_vec(),
                Expiry::At(now + Duration::from_millis(*ttl_ms as u64)),
                Condition::None,
                false,
            );
        }

        // `now` here is guaranteed past every generated TTL (all < 1000ms).
        let later = now + Duration::from_millis(1001);
        for key in &keys {
            prop_assert_eq!(s.get(later, key), None);
        }
    }

    #[test]
    fn incr_monotonic_under_churn(
        deltas in prop::collection::vec(-100i64..100, 1..200),
        churn in prop::collection::vec((0u32..20, 0usize..50), 1..200)
    ) {
        let mut s = Store::new();
        let now = t0();
        let target = b"target";
        let mut expected: i64 = 0;
        let mut churn_iter = churn.into_iter().cycle();

        for delta in deltas {
            // Interleave unrelated churn on other keys — inserts/removes
            // that reshape the underlying HashMap — between every INCR on
            // the target key. `target` is never touched by churn, so it's
            // never itself evicted or overwritten by unrelated activity.
            let (ck, cv) = churn_iter.next().unwrap();
            let churn_key = format!("churn{ck}").into_bytes();
            s.set(now, &churn_key, vec![b'y'; cv], Expiry::None, Condition::None, false);

            expected += delta; // bounded small (|deltas| <= 200*100), never overflows i64
            let result = s.incr_by(now, target, delta);
            prop_assert_eq!(result, Ok(expected));
        }
    }
}

// W2 (Phase 4 filler): randomized SCAN-interleaving properties.
//
// The existing scan tests (`scan/tests.rs`) are deterministic unit tests of the
// registry's mechanics — pagination, expiry, capacity, continuation across
// connections. What they do not cover is the guarantee D10 actually makes to a
// client, under mutation, with arbitrary interleaving:
//
//     "no misses of stable keys, dups possible"
//
// A key is STABLE across a scan if it was present when the scan started and was
// never removed before the scan finished. Those keys must all come back. Keys
// created mid-scan may or may not appear, keys deleted mid-scan may still appear
// (the snapshot already holds them), and any key may appear more than once — all
// three are explicitly permitted, so the assertions below must not accidentally
// forbid them. Getting that boundary wrong in the test is how a correct
// implementation gets "fixed" into a wrong one.

/// Mutations interleaved between SCAN pages.
#[derive(Debug, Clone)]
enum Churn {
    /// Overwrite a key that the scan started with (stays stable — a value
    /// change is not a removal).
    Touch(usize),
    /// Delete one of the initial keys (removes it from the stable set).
    Delete(usize),
    /// Insert a key that did not exist when the scan began.
    Insert(usize),
    /// Nothing — keeps "no churn on this page" in the generated space.
    Idle,
}

fn churn_strategy() -> impl Strategy<Value = Churn> {
    prop_oneof![
        3 => (0usize..40).prop_map(Churn::Touch),
        3 => (0usize..40).prop_map(Churn::Delete),
        3 => (0usize..40).prop_map(Churn::Insert),
        1 => Just(Churn::Idle),
    ]
}

fn init_key(i: usize) -> Vec<u8> {
    format!("init:{i:04}").into_bytes()
}

fn new_key(i: usize) -> Vec<u8> {
    format!("new:{i:04}").into_bytes()
}

proptest! {
    /// The core D10 guarantee: mutate freely between pages, and every key that
    /// was present at the start and never deleted still comes back.
    #[test]
    fn scan_never_misses_stable_keys_under_churn(
        n_initial in 1usize..40,
        count in 1usize..8,
        churn in prop::collection::vec(churn_strategy(), 0..60),
    ) {
        let mut s = Store::new();
        let now = t0();

        for i in 0..n_initial {
            s.set(now, &init_key(i), b"v0".to_vec(), Expiry::None, Condition::None, false);
        }

        // Keys deleted at any point during the scan lose stability. Tracked as
        // the scan runs rather than derived afterwards, because a key deleted
        // and then re-inserted was still absent for a while and is therefore
        // not stable.
        let mut deleted_during_scan: std::collections::HashSet<Vec<u8>> =
            std::collections::HashSet::new();
        let mut seen: Vec<Vec<u8>> = Vec::new();

        let mut cursor = 0u64;
        let mut churn = churn.into_iter();
        // Termination bound: each page consumes at least one snapshot slot, and
        // a restart re-snapshots at most the live keyspace. A scan that needs
        // more pages than this is not converging, which is itself the bug.
        let max_pages = 8 * (n_initial + 40) + 64;
        let mut pages = 0usize;

        loop {
            let (next, page) = s.scan(now, cursor, count);
            seen.extend(page);
            pages += 1;
            prop_assert!(pages <= max_pages, "scan did not terminate in {max_pages} pages");
            cursor = next;
            if cursor == 0 {
                break;
            }

            match churn.next() {
                Some(Churn::Touch(i)) => {
                    let k = init_key(i % n_initial);
                    s.set(now, &k, b"v1".to_vec(), Expiry::None, Condition::None, false);
                }
                Some(Churn::Delete(i)) => {
                    let k = init_key(i % n_initial);
                    s.del(now, &[&k]);
                    deleted_during_scan.insert(k);
                }
                Some(Churn::Insert(i)) => {
                    s.set(now, &new_key(i), b"v".to_vec(), Expiry::None, Condition::None, false);
                }
                Some(Churn::Idle) | None => {}
            }
        }

        let seen_set: std::collections::HashSet<&Vec<u8>> = seen.iter().collect();
        for i in 0..n_initial {
            let k = init_key(i);
            if deleted_during_scan.contains(&k) {
                continue; // not stable — a miss is allowed
            }
            prop_assert!(
                seen_set.contains(&k),
                "stable key {:?} was MISSED (n_initial={}, count={}, pages={}, deleted={})",
                String::from_utf8_lossy(&k), n_initial, count, pages, deleted_during_scan.len()
            );
        }
    }

    /// Same guarantee, but with the cursor deliberately invalidated mid-scan so
    /// the registry-miss restart path (D10) is the one under test. Dups are
    /// certain here; misses still are not allowed.
    #[test]
    fn scan_never_misses_stable_keys_across_forced_restarts(
        n_initial in 1usize..30,
        count in 1usize..6,
        restart_after in prop::collection::vec(0usize..4, 0..20),
    ) {
        let mut s = Store::new();
        let now = t0();
        for i in 0..n_initial {
            s.set(now, &init_key(i), b"v0".to_vec(), Expiry::None, Condition::None, false);
        }

        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut cursor = 0u64;
        let mut restarts = restart_after.into_iter();
        let mut pages_since_restart = 0usize;
        let mut budget = 8 * n_initial + 256;

        loop {
            let (next, page) = s.scan(now, cursor, count);
            seen.extend(page);
            cursor = next;
            pages_since_restart += 1;
            budget -= 1;
            prop_assert!(budget > 0, "forced-restart scan did not converge");
            if cursor == 0 {
                break;
            }
            // Throw the cursor away and hand back an id the registry has never
            // issued. D10 says this restarts from a fresh snapshot rather than
            // erroring, so the scan must still complete and still cover
            // everything stable.
            if let Some(after) = restarts.next() {
                if pages_since_restart > after {
                    cursor = u64::MAX - (budget as u64);
                    pages_since_restart = 0;
                }
            }
        }

        let seen_set: std::collections::HashSet<&Vec<u8>> = seen.iter().collect();
        for i in 0..n_initial {
            let k = init_key(i);
            prop_assert!(
                seen_set.contains(&k),
                "stable key {:?} MISSED across forced restarts",
                String::from_utf8_lossy(&k)
            );
        }
    }

    /// Soundness in the other direction: SCAN must never invent a key. Every
    /// key returned has to be one that was actually written at some point.
    #[test]
    fn scan_never_returns_a_key_that_was_never_written(
        n_initial in 1usize..30,
        count in 1usize..6,
        inserts in prop::collection::vec(0usize..40, 0..30),
    ) {
        let mut s = Store::new();
        let now = t0();
        let mut ever_written: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();

        for i in 0..n_initial {
            let k = init_key(i);
            s.set(now, &k, b"v0".to_vec(), Expiry::None, Condition::None, false);
            ever_written.insert(k);
        }

        let mut cursor = 0u64;
        let mut inserts = inserts.into_iter();
        let mut budget = 8 * (n_initial + 40) + 64;

        loop {
            let (next, page) = s.scan(now, cursor, count);
            for k in page {
                prop_assert!(
                    ever_written.contains(&k),
                    "SCAN returned {:?}, which was never written",
                    String::from_utf8_lossy(&k)
                );
            }
            cursor = next;
            budget -= 1;
            prop_assert!(budget > 0, "scan did not terminate");
            if cursor == 0 {
                break;
            }
            if let Some(i) = inserts.next() {
                let k = new_key(i);
                s.set(now, &k, b"v".to_vec(), Expiry::None, Condition::None, false);
                ever_written.insert(k);
            }
        }
    }
}
