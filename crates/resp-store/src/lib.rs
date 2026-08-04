//! In-memory T0-scoped store: plain HashMap + lazy TTL expiry. No PG deps,
//! no RESP-protocol deps — pure Rust, deterministic (caller supplies `now`).
//! Phase 2 scope (not here): active expiry sweep, CLOCK-LRU eviction under a
//! byte budget — the `Entry` shape below already reserves what that needs.

use std::collections::HashMap;
use std::time::Instant;

/// One stored value. `clock_bit` is unused in Phase 1 — reserved so Phase 2's
/// CLOCK-LRU sweep can add itself without changing this shape (bible §3.5).
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

#[derive(Default)]
pub struct Store {
    map: HashMap<Box<[u8]>, Entry>,
}

impl Store {
    pub fn new() -> Self {
        Store {
            map: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn is_live(entry: &Entry, now: Instant) -> bool {
        match entry.expires_at {
            Some(deadline) => now < deadline,
            None => true,
        }
    }

    /// Lazy expiry on access: if `key` is present but expired as of `now`,
    /// remove it. Bible §3.5: "lazy expiry on read."
    fn expire_if_needed(&mut self, key: &[u8], now: Instant) {
        let expired = self.map.get(key).is_some_and(|e| !Self::is_live(e, now));
        if expired {
            self.map.remove(key);
        }
    }

    pub fn get(&mut self, now: Instant, key: &[u8]) -> Option<&[u8]> {
        self.expire_if_needed(key, now);
        self.map.get(key).map(|e| e.value.as_slice())
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

        self.map.insert(
            key.to_vec().into_boxed_slice(),
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

    /// Count of keys actually removed (expired keys don't count — they're
    /// already logically gone).
    pub fn del(&mut self, now: Instant, keys: &[&[u8]]) -> u64 {
        let mut count = 0;
        for key in keys {
            self.expire_if_needed(key, now);
            if self.map.remove(*key).is_some() {
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
        self.map.insert(
            key.to_vec().into_boxed_slice(),
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
                self.map.get(*key).map(|e| e.value.clone())
            })
            .collect()
    }

    /// Always succeeds; each pair behaves like a bare SET (clears TTL).
    pub fn mset(&mut self, pairs: &[(&[u8], Vec<u8>)]) {
        for (key, value) in pairs {
            self.map.insert(
                key.to_vec().into_boxed_slice(),
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
mod tests;
