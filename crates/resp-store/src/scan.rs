//! D10 (bible §12): SCAN cursor state lives in a bounded, server-side
//! registry keyed by an opaque cursor id — NOT per-connection state. A
//! per-connection snapshot was the first design, rejected before being
//! built: pooled clients (e.g. redis-py's `scan_iter` over a connection
//! pool) send successive `SCAN` calls on different physical connections,
//! so connection-scoped cursor state would break them intermittently. On a
//! registry miss or capacity overflow, the scan silently restarts from a
//! fresh snapshot — the documented guarantee ("no misses of stable keys,
//! dups possible") explicitly allows dups, never misses.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_LIVE_CURSORS: usize = 64;
pub const CURSOR_IDLE_EXPIRY: Duration = Duration::from_secs(60);

struct ScanState {
    snapshot: Vec<Box<[u8]>>,
    position: usize,
    last_used: Instant,
}

#[derive(Default)]
pub struct ScanRegistry {
    cursors: HashMap<u64, ScanState>,
    next_id: u64,
}

impl ScanRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn live_cursor_count(&self) -> usize {
        self.cursors.len()
    }

    /// `fresh_keys` is called lazily — only when actually starting a new
    /// scan (cursor 0, or a miss/overflow restart) — never on a cache hit.
    pub fn scan(
        &mut self,
        now: Instant,
        cursor: u64,
        count: usize,
        fresh_keys: impl FnOnce() -> Vec<Box<[u8]>>,
    ) -> (u64, Vec<Vec<u8>>) {
        if cursor != 0 {
            if let Some(state) = self.cursors.get_mut(&cursor) {
                if now.duration_since(state.last_used) < CURSOR_IDLE_EXPIRY {
                    state.last_used = now;
                    let page = Self::take_page(state, count);
                    return if state.position >= state.snapshot.len() {
                        self.cursors.remove(&cursor);
                        (0, page)
                    } else {
                        (cursor, page)
                    };
                }
            }
            // Miss (unknown id) or expired: restart per D10.
        }
        self.start(now, fresh_keys(), count)
    }

    fn start(&mut self, now: Instant, keys: Vec<Box<[u8]>>, count: usize) -> (u64, Vec<Vec<u8>>) {
        self.evict_stale_and_over_capacity(now);
        self.next_id = self.next_id.wrapping_add(1).max(1); // never hand out cursor 0
        let id = self.next_id;
        let mut state = ScanState {
            snapshot: keys,
            position: 0,
            last_used: now,
        };
        let page = Self::take_page(&mut state, count);
        if state.position < state.snapshot.len() {
            self.cursors.insert(id, state);
            (id, page)
        } else {
            (0, page) // exhausted in a single page
        }
    }

    fn take_page(state: &mut ScanState, count: usize) -> Vec<Vec<u8>> {
        let end = (state.position + count.max(1)).min(state.snapshot.len());
        let page = state.snapshot[state.position..end]
            .iter()
            .map(|k| k.to_vec())
            .collect();
        state.position = end;
        page
    }

    fn evict_stale_and_over_capacity(&mut self, now: Instant) {
        self.cursors
            .retain(|_, s| now.duration_since(s.last_used) < CURSOR_IDLE_EXPIRY);
        while self.cursors.len() >= MAX_LIVE_CURSORS {
            let oldest = self
                .cursors
                .iter()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(&id, _)| id);
            match oldest {
                Some(id) => {
                    self.cursors.remove(&id);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;
