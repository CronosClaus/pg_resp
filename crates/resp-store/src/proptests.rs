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
