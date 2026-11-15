//! A bounded, least-recently-used table for the process-wide budget maps.
//!
//! Several modules keep one budget object per authority: admission policies,
//! socket semaphores, stream semaphores, redirector buckets. All of them were
//! bounded the same broken way — `retain(strong_count > 1)` once the map reached
//! its nominal cap — which frees nothing when every entry is live. That is not a
//! rare case: a wide install holds hundreds of authorities at once, so the maps
//! grew past their caps without bound.
//!
//! This table enforces the cap on *insertion* instead: it prunes anything the
//! caller can prove is droppable, then evicts the least-recently-used droppable
//! entry. An entry that must not be forgotten — one a permit still references,
//! or one carrying a ban — is never taken, and if every entry is like that the
//! new value is handed out unregistered rather than breaking the existing
//! entries' invariants.
//!
//! Handing out an unregistered value is the conservative failure: the table's
//! length stays at its cap, no live budget loses its permits, and the caller
//! still gets a working object. It costs at most one duplicate budget for a key
//! that is already live — and that can only happen when the table holds its full
//! allowance of *simultaneously active* authorities, which is already bounded by
//! the process-wide socket and stream ceilings those budgets feed into.

use std::collections::HashMap;
use std::hash::Hash;

struct Entry<V> {
    value: V,
    last_used: u64,
}

pub(crate) struct BoundedTable<K, V> {
    entries: HashMap<K, Entry<V>>,
    capacity: usize,
    clock: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> BoundedTable<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    /// Returns the entry for `key`, creating it with `make` if absent.
    ///
    /// `droppable` answers whether an existing entry may be forgotten. It is
    /// consulted only when the table is full, and an entry for which it returns
    /// `false` is kept even if that means the new key cannot be stored.
    pub(crate) fn get_or_insert_with<F, D>(
        &mut self,
        key: K,
        make: F,
        droppable: D,
    ) -> V
    where
        F: FnOnce() -> V,
        D: Fn(&V) -> bool,
    {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.tick();
            return entry.value.clone();
        }
        if self.entries.len() >= self.capacity {
            self.entries.retain(|_, entry| !droppable(&entry.value));
        }
        if self.entries.len() >= self.capacity
            && let Some(victim) = self
                .entries
                .iter()
                .filter(|(_, entry)| droppable(&entry.value))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| (*key).clone())
        {
            self.entries.remove(&victim);
        }
        if self.entries.len() >= self.capacity {
            return make();
        }
        let value = make();
        let last_used = self.tick();
        self.entries.insert(
            key,
            Entry {
                value: value.clone(),
                last_used,
            },
        );
        value
    }

    /// Every stored value, for maintenance sweeps.
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|entry| &entry.value)
    }
}
