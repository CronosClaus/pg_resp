//! In-memory T0/T1-scoped store: HashMap + lazy/active TTL expiry +
//! approximate CLOCK-LRU eviction under a byte budget. No PG deps, no
//! RESP-protocol deps — pure Rust, deterministic (caller supplies `now`).
//!
//! Phase 2 pre-work scope (bible §5 Phase 2, done here as fast-loop-only
//! pre-work per this run's Stage C): active expiry sweep, CLOCK-LRU
//! eviction, and the three named property tests. NOT here (real Phase 2,
//! needs a live server + memtier): the soak test, GUC wiring
//! (`pg_resp.max_memory`/`pg_resp.eviction`), tuning the sample size against
//! real workloads.

use std::collections::HashMap;
use std::time::Instant;

mod scan;
use scan::ScanRegistry;

/// Fixed per-entry overhead (HashMap bucket + `Entry` struct fields +
/// allocator bookkeeping for two heap allocations — key and value).
///
/// **Measured** (bible §3.5), not theoretical: live `pg_resp` bgworker,
/// `/proc/<pid>/status`'s `VmRSS` before/after loading 100,000 1-byte-value
/// entries, twice in a row (200,000 total). First 100k: RSS delta 8,600 KB
/// → ~88.1 bytes/entry including key+value payload (avg ~15.9-byte keys) →
/// ~71 bytes overhead alone. Second 100k (same process, keys now 100k-200k):
/// RSS delta 14,568 KB → ~149.2 bytes/entry (avg ~13.9-byte keys, shorter)
/// → ~134 bytes overhead alone — nearly double the first measurement.
///
/// The two don't match because `std::collections::HashMap` grows its
/// backing table by doubling, so crossing a capacity threshold mid-load (as
/// the second batch did, ~100k→200k) allocates roughly double the bucket
/// array for a load that only grew linearly — real memory-per-entry is
/// inherently non-linear near a resize point, not a flaw in the
/// measurement. `96` is a documented, deliberately conservative constant
/// between the two observations (closer to the resize-affected reading) —
/// erring toward *overestimating* overhead is the safe direction for a
/// byte-budget accounting constant: `resp.stats()`/`INFO`'s `used_bytes`
/// under-promising available capacity is far better than a store that
/// silently exceeds its configured `pg_resp.max_memory` in practice.
pub const PER_ENTRY_OVERHEAD_BYTES: usize = 96;

/// How many keys a single eviction/expiry sweep samples per call. Redis's
/// own default active-expire-cycle sample size is in this range; bible §3.5
/// calls this "N keys per tick, Redis-style" and defers real tuning to
/// Phase 2 proper (needs a soak test to tune against, not pre-work).
pub const DEFAULT_SAMPLE_SIZE: usize = 20;

fn entry_bytes(key: &[u8], value_len: usize) -> usize {
    key.len() + value_len + PER_ENTRY_OVERHEAD_BYTES
}

/// One stored value. `clock_bit` is set on every access (get or write) and
/// cleared by the CLOCK-LRU sweep as it gives an entry a second chance —
/// bible §3.5: "approximate LRU via CLOCK on a memory budget... same
/// philosophy as Redis's sampled LRU, simpler implementation."
#[derive(Debug, Clone)]
pub struct Entry {
    pub value: Vec<u8>,
    pub expires_at: Option<Instant>,
    pub clock_bit: bool,
}

/// A resolved expiry decision for SET. Translating RESP's EX/PX/EXAT/PXAT
/// (relative seconds/ms, or absolute unix timestamps) into an `Instant`
/// deadline is the caller's job (naturally done once per command, where the
/// wall-clock/monotonic-clock correlation belongs) — keeps this crate
/// protocol-agnostic and deterministic for testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// Clear any existing TTL (bare `SET`, or an explicit request to persist).
    None,
    /// Preserve whatever TTL (or lack of one) the key already has.
    KeepTtl,
    /// Set an absolute expiry deadline.
    At(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Condition {
    #[default]
    None,
    IfExists,
    IfNotExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOutcome {
    /// Whether the write actually happened (false if NX/XX condition failed).
    pub applied: bool,
    /// The value that was in the key before this call, if any (for SET...GET).
    pub old_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncrError {
    NotAnInteger,
    Overflow,
}

/// Snapshot of the counters `INFO` reports (bible §5 Phase 2 "memory
/// honesty" gate, satisfied via `INFO` rather than the Phase-3 `resp.stats()`
/// SQL function — see project-bible.md's Phase 3 gate table for why the two
/// must stay consistent once both exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub keys: usize,
    pub used_bytes: usize,
    pub max_memory_bytes: Option<usize>,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

pub struct Store {
    map: HashMap<Box<[u8]>, Entry>,
    used_bytes: usize,
    max_memory_bytes: Option<usize>,
    scan_registry: ScanRegistry,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        Store {
            map: HashMap::new(),
            used_bytes: 0,
            max_memory_bytes: None,
            scan_registry: ScanRegistry::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// A store that evicts (CLOCK-LRU) to stay within `max_bytes` — accounted
    /// as key bytes + value bytes + `PER_ENTRY_OVERHEAD_BYTES` per entry.
    pub fn with_max_memory(max_bytes: usize) -> Self {
        Store {
            map: HashMap::new(),
            used_bytes: 0,
            max_memory_bytes: Some(max_bytes),
            scan_registry: ScanRegistry::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn max_memory_bytes(&self) -> Option<usize> {
        self.max_memory_bytes
    }

    fn is_live(entry: &Entry, now: Instant) -> bool {
        match entry.expires_at {
            Some(deadline) => now < deadline,
            None => true,
        }
    }

    fn remove_accounted(&mut self, key: &[u8]) -> Option<Entry> {
        let removed = self.map.remove(key);
        if let Some(ref e) = removed {
            self.used_bytes -= entry_bytes(key, e.value.len());
        }
        removed
    }

    fn insert_accounted(&mut self, key: &[u8], mut entry: Entry) {
        entry.clock_bit = true; // a fresh write counts as an access
        let new_size = entry_bytes(key, entry.value.len());
        if let Some(old) = self.map.insert(key.to_vec().into_boxed_slice(), entry) {
            self.used_bytes -= entry_bytes(key, old.value.len());
        }
        self.used_bytes += new_size;
        self.evict_to_budget(key);
    }

    /// Lazy expiry on access: if `key` is present but expired as of `now`,
    /// remove it. Bible §3.5: "lazy expiry on read."
    fn expire_if_needed(&mut self, key: &[u8], now: Instant) {
        let expired = self.map.get(key).is_some_and(|e| !Self::is_live(e, now));
        if expired {
            self.remove_accounted(key);
        }
    }

    /// CLOCK-LRU: while over budget, sample keys (skipping `protect`, the
    /// key just written — never evict what you just inserted in the same
    /// call) and evict the first one found with a clear clock bit, clearing
    /// bits along the way (the "second chance"). Simplification vs. a
    /// production CLOCK sweep (documented, not hidden): samples via
    /// HashMap's own iteration order rather than a true random sample or a
    /// stable ring/cursor — correct for the "never exceeds budget" property,
    /// but sampling-quality tuning is real Phase 2 work (needs a soak test
    /// to tune against, not pre-work).
    fn evict_to_budget(&mut self, protect: &[u8]) {
        let Some(budget) = self.max_memory_bytes else {
            return;
        };
        while self.used_bytes > budget && !self.map.is_empty() {
            let mut fallback: Option<Box<[u8]>> = None;
            let mut evict: Option<Box<[u8]>> = None;
            for (k, e) in self.map.iter_mut().take(DEFAULT_SAMPLE_SIZE) {
                if k.as_ref() == protect {
                    continue;
                }
                if e.clock_bit {
                    e.clock_bit = false; // second chance
                    fallback.get_or_insert_with(|| k.clone());
                } else {
                    evict = Some(k.clone());
                    break;
                }
            }
            let victim = evict.or(fallback);
            match victim {
                Some(k) => {
                    self.remove_accounted(&k);
                    self.evictions += 1;
                }
                None => break, // only `protect` left (or map only has protect) — stop
            }
        }
    }

    /// Active expiry sweep: samples up to `sample_size` keys and removes any
    /// expired as of `now`. Bible §3.5: "active sweep from the event loop
    /// timer (N keys per tick, Redis-style), so memory is actually reclaimed
    /// without a read." The caller (pg_resp's event loop, real Phase 2 work)
    /// is responsible for invoking this periodically; this crate just
    /// provides the mechanism.
    pub fn active_expire_sweep(&mut self, now: Instant, sample_size: usize) -> usize {
        let expired: Vec<Box<[u8]>> = self
            .map
            .iter()
            .take(sample_size)
            .filter(|(_, e)| !Self::is_live(e, now))
            .map(|(k, _)| k.clone())
            .collect();
        let count = expired.len();
        for k in expired {
            self.remove_accounted(&k);
        }
        count
    }

    pub fn get(&mut self, now: Instant, key: &[u8]) -> Option<&[u8]> {
        self.expire_if_needed(key, now);
        match self.map.get_mut(key) {
            Some(e) => {
                e.clock_bit = true;
                self.hits += 1;
                Some(e.value.as_slice())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            keys: self.map.len(),
            used_bytes: self.used_bytes,
            max_memory_bytes: self.max_memory_bytes,
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
        }
    }

    /// SCAN (bible §3.4 T1 tier). `cursor` 0 starts a new scan; any other
    /// value continues a prior one. Returns `(next_cursor, keys_in_this_page)`
    /// — `next_cursor == 0` means the scan is complete. See D10 (bible §12)
    /// and `scan.rs` for the cursor-registry design (store-level, bounded,
    /// idle-expiring, restart-on-miss).
    pub fn scan(&mut self, now: Instant, cursor: u64, count: usize) -> (u64, Vec<Vec<u8>>) {
        let map = &self.map;
        self.scan_registry.scan(now, cursor, count, || {
            map.iter()
                .filter(|(_, e)| Self::is_live(e, now))
                .map(|(k, _)| k.clone())
                .collect()
        })
    }

    /// KEYS (bible §3.4 T2 — "with docs warning": O(n), unlike SCAN).
    /// Pattern filtering (glob match) is the caller's job (pg_resp's
    /// dispatch layer), keeping this crate protocol/pattern-syntax-agnostic.
    pub fn all_keys(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.map
            .iter()
            .filter(|(_, e)| Self::is_live(e, now))
            .map(|(k, _)| k.to_vec())
            .collect()
    }

    pub fn set(
        &mut self,
        now: Instant,
        key: &[u8],
        value: Vec<u8>,
        expiry: Expiry,
        condition: Condition,
        want_old: bool,
    ) -> SetOutcome {
        self.expire_if_needed(key, now);
        let exists = self.map.contains_key(key);

        let condition_ok = match condition {
            Condition::None => true,
            Condition::IfExists => exists,
            Condition::IfNotExists => !exists,
        };

        let old_value = if want_old {
            self.map.get(key).map(|e| e.value.clone())
        } else {
            None
        };

        if !condition_ok {
            return SetOutcome {
                applied: false,
                old_value,
            };
        }

        let expires_at = match expiry {
            Expiry::None => None,
            Expiry::KeepTtl => self.map.get(key).and_then(|e| e.expires_at),
            Expiry::At(deadline) => Some(deadline),
        };

        self.insert_accounted(
            key,
            Entry {
                value,
                expires_at,
                clock_bit: false,
            },
        );

        SetOutcome {
            applied: true,
            old_value,
        }
    }

    /// FLUSHDB/FLUSHALL (bible §3.4 T1 — single-db model, so both are
    /// equivalent per bible §3.6 "SELECT accept db 0 only"). Does not touch
    /// stats counters (hits/misses/evictions are lifetime counters, matching
    /// `INFO`'s own semantics of not resetting on a data flush).
    pub fn clear(&mut self) {
        self.map.clear();
        self.used_bytes = 0;
    }

    /// PERSIST (bible §3.4 T2). Valkey semantics (docs/refs/valkey-notes.md):
    /// integer 1 if a TTL was actually removed, 0 if the key is missing or
    /// already had no TTL — these last two cases are indistinguishable by
    /// design, matching Valkey.
    pub fn persist(&mut self, now: Instant, key: &[u8]) -> bool {
        self.expire_if_needed(key, now);
        match self.map.get_mut(key) {
            Some(entry) if entry.expires_at.is_some() => {
                entry.expires_at = None;
                true
            }
            _ => false,
        }
    }

    /// RANDOMKEY (bible §3.4 T2). `None` on an empty store. Not a true
    /// uniform random pick (iterates HashMap's own arbitrary-but-not
    /// necessarily-uniform order and takes the first live entry) — matches
    /// this crate's existing "correctness first" sampling philosophy
    /// (bible §3.5's CLOCK-LRU sampling has the same documented simplicity).
    pub fn random_key(&mut self, now: Instant) -> Option<Vec<u8>> {
        let expired: Vec<Box<[u8]>> = self
            .map
            .iter()
            .take(1)
            .filter(|(_, e)| !Self::is_live(e, now))
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.remove_accounted(&k);
        }
        self.map.keys().next().map(|k| k.to_vec())
    }

    /// GETDEL (bible §3.4 T2): atomic get-then-delete. `None` if missing.
    pub fn get_del(&mut self, now: Instant, key: &[u8]) -> Option<Vec<u8>> {
        self.expire_if_needed(key, now);
        match self.map.get(key) {
            Some(e) => {
                let value = e.value.clone();
                self.hits += 1;
                self.remove_accounted(key);
                Some(value)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// GETEX (bible §3.4 T2): get, optionally updating/clearing TTL in the
    /// same call. `expiry: None` means "leave the TTL exactly as-is" (plain
    /// GETEX with no option) — distinct from `Expiry::None` on `set`, which
    /// means "clear it"; GETEX's own PERSIST option maps to
    /// `Some(Expiry::None)` here to make that distinction explicit at the
    /// call site.
    pub fn get_ex(&mut self, now: Instant, key: &[u8], expiry: Option<Expiry>) -> Option<Vec<u8>> {
        self.expire_if_needed(key, now);
        let value = match self.map.get(key) {
            Some(e) => {
                self.hits += 1;
                e.value.clone()
            }
            None => {
                self.misses += 1;
                return None;
            }
        };
        if let Some(expiry) = expiry {
            let new_expires_at = match expiry {
                Expiry::None => None,
                Expiry::KeepTtl => self.map.get(key).and_then(|e| e.expires_at),
                Expiry::At(deadline) => Some(deadline),
            };
            if let Some(entry) = self.map.get_mut(key) {
                entry.expires_at = new_expires_at;
            }
        }
        Some(value)
    }

    /// Count of keys actually removed (expired keys don't count — they're
    /// already logically gone).
    pub fn del(&mut self, now: Instant, keys: &[&[u8]]) -> u64 {
        let mut count = 0;
        for key in keys {
            self.expire_if_needed(key, now);
            if self.remove_accounted(key).is_some() {
                count += 1;
            }
        }
        count
    }

    /// Counts repeats, matching Valkey (`EXISTS k k` on one key returns 2).
    pub fn exists(&mut self, now: Instant, keys: &[&[u8]]) -> u64 {
        let mut count = 0;
        for key in keys {
            self.expire_if_needed(key, now);
            if self.map.contains_key(*key) {
                count += 1;
            }
        }
        count
    }

    /// -2 missing, -1 exists with no TTL, else whole seconds remaining
    /// (rounded up, matching Valkey's `(ms + 500) / 1000` — resp-protocol
    /// skill §5, source-confirmed against docs/refs/valkey-notes.md).
    pub fn ttl_seconds(&mut self, now: Instant, key: &[u8]) -> i64 {
        self.expire_if_needed(key, now);
        match self.map.get(key) {
            None => -2,
            Some(Entry {
                expires_at: None, ..
            }) => -1,
            Some(Entry {
                expires_at: Some(deadline),
                ..
            }) => {
                let remaining_ms = deadline.saturating_duration_since(now).as_millis() as i64;
                (remaining_ms + 500) / 1000
            }
        }
    }

    /// -2 missing, -1 exists with no TTL, else milliseconds remaining (unrounded).
    pub fn pttl_millis(&mut self, now: Instant, key: &[u8]) -> i64 {
        self.expire_if_needed(key, now);
        match self.map.get(key) {
            None => -2,
            Some(Entry {
                expires_at: None, ..
            }) => -1,
            Some(Entry {
                expires_at: Some(deadline),
                ..
            }) => deadline.saturating_duration_since(now).as_millis() as i64,
        }
    }

    /// Bible T0 scope: bare EXPIRE only (no NX/XX/GT/LT — a Valkey extension
    /// noted out-of-scope in the resp-protocol skill). Returns false (no-op)
    /// if the key doesn't exist, true if the TTL was set.
    pub fn expire(&mut self, now: Instant, key: &[u8], deadline: Instant) -> bool {
        self.expire_if_needed(key, now);
        match self.map.get_mut(key) {
            None => false,
            Some(entry) => {
                entry.expires_at = Some(deadline);
                true
            }
        }
    }

    /// Shared by INCR (+1), DECR (-1), INCRBY (delta), DECRBY (-delta).
    /// Missing key is treated as 0. TTL (if any) is preserved — this is a
    /// value mutation, not a fresh SET.
    pub fn incr_by(&mut self, now: Instant, key: &[u8], delta: i64) -> Result<i64, IncrError> {
        self.expire_if_needed(key, now);
        let current: i64 = match self.map.get(key) {
            None => 0,
            Some(entry) => std::str::from_utf8(&entry.value)
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or(IncrError::NotAnInteger)?,
        };
        let new_value = current.checked_add(delta).ok_or(IncrError::Overflow)?;
        let expires_at = self.map.get(key).and_then(|e| e.expires_at);
        self.insert_accounted(
            key,
            Entry {
                value: new_value.to_string().into_bytes(),
                expires_at,
                clock_bit: false,
            },
        );
        Ok(new_value)
    }

    pub fn mget(&mut self, now: Instant, keys: &[&[u8]]) -> Vec<Option<Vec<u8>>> {
        keys.iter()
            .map(|key| {
                self.expire_if_needed(key, now);
                match self.map.get_mut(*key) {
                    Some(e) => {
                        e.clock_bit = true;
                        self.hits += 1;
                        Some(e.value.clone())
                    }
                    None => {
                        self.misses += 1;
                        None
                    }
                }
            })
            .collect()
    }

    /// Always succeeds; each pair behaves like a bare SET (clears TTL).
    pub fn mset(&mut self, pairs: &[(&[u8], Vec<u8>)]) {
        for (key, value) in pairs {
            self.insert_accounted(
                key,
                Entry {
                    value: value.clone(),
                    expires_at: None,
                    clock_bit: false,
                },
            );
        }
    }
}

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;
