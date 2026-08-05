use super::*;

fn t0() -> Instant {
    Instant::now()
}

fn keys(n: usize) -> Vec<Box<[u8]>> {
    (0..n)
        .map(|i| format!("k{i}").into_bytes().into_boxed_slice())
        .collect()
}

#[test]
fn full_scan_collects_every_key_no_misses() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    let all = keys(25);
    let all_for_closure = all.clone();

    let mut collected = Vec::new();
    let mut cursor = 0u64;
    loop {
        let (next, page) = reg.scan(now, cursor, 7, || all_for_closure.clone());
        collected.extend(page);
        if next == 0 {
            break;
        }
        cursor = next;
    }

    let mut expected: Vec<Vec<u8>> = all.iter().map(|k| k.to_vec()).collect();
    collected.sort();
    expected.sort();
    assert_eq!(collected, expected, "a full scan must find every key present for its whole duration");
}

#[test]
fn cursor_zero_always_starts_fresh() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    let (c1, page1) = reg.scan(now, 0, 3, || keys(10));
    assert!(c1 != 0);
    assert_eq!(page1.len(), 3);
}

#[test]
fn exhausted_scan_returns_cursor_zero_and_frees_registry_slot() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    let (cursor, page) = reg.scan(now, 0, 100, || keys(5)); // count > total: exhausts in one page
    assert_eq!(cursor, 0);
    assert_eq!(page.len(), 5);
    assert_eq!(reg.live_cursor_count(), 0);
}

#[test]
fn unknown_cursor_restarts_instead_of_erroring() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    // A cursor id that was never issued — simulates a miss (e.g. after a
    // restart, or a client holding a stale id from a different registry).
    let (next, page) = reg.scan(now, 999_999, 3, || keys(10));
    assert!(next != 0 || page.len() == 10, "a miss must restart the scan, not error");
}

#[test]
fn idle_cursor_expires_after_60s_and_restarts() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    let (c1, _) = reg.scan(now, 0, 3, || keys(10));

    let later = now + Duration::from_secs(61);
    let (c2, page) = reg.scan(later, c1, 3, || keys(10));
    // Restarted: page comes from a fresh snapshot at position 0 again.
    assert_eq!(page.len(), 3);
    assert!(c2 != c1 || c2 == 0, "an idle-expired cursor must not silently resume mid-scan");
}

#[test]
fn registry_bounded_to_max_live_cursors() {
    let mut reg = ScanRegistry::new();
    let now = t0();
    // Start more concurrent scans than the cap, never finishing any of them.
    for _ in 0..(MAX_LIVE_CURSORS + 20) {
        let (cursor, _) = reg.scan(now, 0, 1, || keys(50));
        assert!(cursor != 0, "each scan should have more pages left, so a live cursor is registered");
    }
    assert!(
        reg.live_cursor_count() <= MAX_LIVE_CURSORS,
        "registry must stay bounded even under scan overflow: got {}",
        reg.live_cursor_count()
    );
}

#[test]
fn continuation_works_across_simulated_different_connections() {
    // The whole point of D10: cursor state lives in the Store/registry, not
    // in any per-connection object. Simulate "connection A" issuing the
    // first SCAN and "connection B" (a completely separate call site, no
    // shared state passed between them except the cursor id itself, exactly
    // as it would arrive over the wire from a different socket) continuing
    // it.
    let mut reg = ScanRegistry::new();
    let now = t0();
    let all = keys(12);

    // "Connection A": first SCAN call.
    let a_keys = all.clone();
    let (cursor_after_a, page_a) = reg.scan(now, 0, 5, || a_keys);
    assert_eq!(page_a.len(), 5);
    assert!(cursor_after_a != 0);

    // "Connection B": a distinct call, later, with no access to anything
    // connection A held — only the cursor id (as it would arrive in a real
    // SCAN command's argument over a brand new socket).
    let b_keys = all.clone();
    let (cursor_after_b, page_b) = reg.scan(now, cursor_after_a, 5, || b_keys);
    assert_eq!(page_b.len(), 5);

    // "Connection A" again, finishing the scan.
    let c_keys = all.clone();
    let (final_cursor, page_c) = reg.scan(now, cursor_after_b, 5, || c_keys);
    assert_eq!(final_cursor, 0);
    assert_eq!(page_c.len(), 2);

    let mut collected: Vec<Vec<u8>> = [page_a, page_b, page_c].concat();
    let mut expected: Vec<Vec<u8>> = all.iter().map(|k| k.to_vec()).collect();
    collected.sort();
    expected.sort();
    assert_eq!(collected, expected);
}
