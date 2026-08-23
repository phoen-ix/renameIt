//! A cache bounded by what it costs, not by how much it holds.
//!
//! `meta::CACHE_CAPACITY` is the wrong shape here twice over. It counts
//! *entries*, and the things this holds differ in size by sixty-four times
//! between the smallest and largest thumbnail; and it clears **wholesale** when
//! full, which for textures means every tile on screen blanking at once, then
//! all of them decoding again, then blanking again. **P52** already records
//! that clear-wholesale is the weakness of those caches — this is the first
//! place where a user would watch it happen.
//!
//! Generic over its payload and its cost function, so everything below is
//! tested with `usize` payloads and no egui at all. The one thing that cannot
//! be tested here — that dropping an `egui::TextureHandle` actually returns the
//! memory — is a property of `epaint`'s `Drop`, which queues the free into the
//! next frame's texture delta.

use std::hash::Hash;

use lru::LruCache;

/// What an entry costs even when it holds nothing.
///
/// Not tidiness. A folder of ten thousand unreadable `.jpg` files produces ten
/// thousand *failure* entries whose payload is zero bytes — under a pure byte
/// budget that is unbounded growth with no symptom until the machine notices.
/// Charging every entry a floor is what makes the bound a bound.
const ENTRY_OVERHEAD: usize = 256;

/// A least-recently-used cache bounded by the total cost of what it holds.
pub struct CostCache<K: Hash + Eq, V> {
    /// `unbounded`, deliberately: `lru`'s own capacity counts entries, and the
    /// quantity being bounded here is bytes. Its list is used for recency and
    /// nothing else.
    entries: LruCache<K, V>,
    cost: fn(&V) -> usize,
    bytes: usize,
    budget: usize,
    /// Bumped by the drawing code once per frame. An entry touched in the
    /// current frame is not evictable — see [`Self::insert`].
    frame: u64,
    touched: std::collections::HashMap<K, u64>,
    evictions: u64,
}

impl<K: Hash + Eq + Clone, V> CostCache<K, V> {
    pub fn new(budget: usize, cost: fn(&V) -> usize) -> Self {
        Self {
            entries: LruCache::unbounded(),
            cost,
            bytes: 0,
            budget,
            frame: 0,
            touched: std::collections::HashMap::new(),
            evictions: 0,
        }
    }

    /// Starts a frame. Anything inserted or read after this is protected from
    /// eviction until the next one.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Raises the budget for as long as the visible set needs it.
    ///
    /// Without this, a window whose visible tiles alone exceed the budget would
    /// evict tile 1 while inserting tile N *of the same frame* — which then
    /// re-requests, decodes, and evicts tile 2. A decode loop that never
    /// settles, on nothing more exotic than a 4K display at the smallest tile
    /// size.
    pub fn reserve_for(&mut self, wanted: usize, ceiling: usize) {
        self.budget = self.budget.max(wanted.min(ceiling));
    }

    /// Looks an entry up **and marks it recently used**.
    ///
    /// `get`, never `peek`: `peek` does not touch the recency list, so the
    /// cache would evict whatever was inserted longest ago regardless of what
    /// is on screen. It is a one-word mistake with no symptom until a folder
    /// large enough to thrash.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let hit = self.entries.get(key).is_some();
        if hit {
            self.touched.insert(key.clone(), self.frame);
        }
        self.entries.peek(key).filter(|_| hit)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains(key)
    }

    /// Stores an entry, evicting the least recently used until the budget is
    /// met — one at a time, never wholesale.
    pub fn insert(&mut self, key: K, value: V) {
        let cost = (self.cost)(&value) + ENTRY_OVERHEAD;
        if let Some(old) = self.entries.put(key.clone(), value) {
            self.bytes -= (self.cost)(&old) + ENTRY_OVERHEAD;
        }
        self.bytes += cost;
        self.touched.insert(key, self.frame);
        self.evict();
    }

    fn evict(&mut self) {
        while self.bytes > self.budget {
            // Skip anything drawn this frame: evicting it would only make the
            // next frame ask for it again.
            let Some((key, _)) = self
                .entries
                .iter()
                .rev()
                .find(|(k, _)| self.touched.get(*k).copied().unwrap_or(0) < self.frame)
                .map(|(k, _)| (k.clone(), ()))
            else {
                // Everything is on screen. Going over budget for this frame is
                // the right answer; thrashing is not.
                break;
            };
            if let Some(dropped) = self.entries.pop(&key) {
                self.bytes -= (self.cost)(&dropped) + ENTRY_OVERHEAD;
                self.touched.remove(&key);
                self.evictions += 1;
            } else {
                break;
            }
        }
    }

    /// Drops everything. F9's half of D140, and the test fixtures' — two
    /// tempdirs can be handed the same path.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.touched.clear();
        self.bytes = 0;
    }

    /// Moves an entry to a new key, keeping its payload.
    ///
    /// What a finished rename does (D139): the file is the same file under a
    /// new name, so re-decoding four hundred photographs to learn that would be
    /// the app's commonest workflow made to look broken.
    pub fn rekey(&mut self, from: &K, to: K) -> bool {
        let Some(value) = self.entries.pop(from) else {
            return false;
        };
        self.touched.remove(from);
        // Cost is unchanged, so the accounting is a move rather than a sum.
        self.bytes -= (self.cost)(&value) + ENTRY_OVERHEAD;
        self.insert(to, value);
        true
    }

    /// Every key held, cloned.
    ///
    /// Cloned rather than borrowed because the one caller — the rekey a finished
    /// run performs — mutates the cache while walking them. Bounded by the byte
    /// budget, so a few thousand at worst, and only ever on a rename.
    pub fn keys(&self) -> Vec<K> {
        self.entries.iter().map(|(key, _)| key.clone()).collect()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

impl<K: Hash + Eq, V> std::fmt::Debug for CostCache<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostCache")
            .field("entries", &self.entries.len())
            .field("bytes", &self.bytes)
            .field("budget", &self.budget)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(budget: usize) -> CostCache<u32, usize> {
        CostCache::new(budget, |v| *v)
    }

    /// The `meta` mistake, made impossible to repeat: a full cache drops its
    /// oldest entry, not all of them.
    #[test]
    fn the_cache_evicts_the_oldest_entry_not_all_of_them() {
        let mut cache = cache(4 * (1000 + ENTRY_OVERHEAD));
        cache.begin_frame();
        for key in 0..4 {
            cache.insert(key, 1000);
        }
        assert_eq!(cache.len(), 4);

        cache.begin_frame();
        cache.insert(4, 1000);
        assert_eq!(cache.len(), 4, "one out, one in — not a reset");
        assert!(!cache.contains(&0), "the oldest went");
        assert!(cache.contains(&4));
    }

    /// A cache bounded by entries is either useless at the large end or
    /// unbounded at the small. The same budget must hold ten big things or a
    /// great many small ones, and never more bytes than it was given.
    #[test]
    fn the_cache_is_bounded_by_bytes_and_not_by_entries() {
        let budget = 100_000;

        let mut small = cache(budget);
        for key in 0..500 {
            small.begin_frame();
            small.insert(key, 100);
        }
        assert!(small.bytes() <= budget, "{}", small.bytes());

        let mut large = cache(budget);
        for key in 0..500 {
            large.begin_frame();
            large.insert(key, 50_000);
        }
        assert!(large.bytes() <= budget, "{}", large.bytes());

        assert!(
            small.len() > large.len() * 10,
            "small entries: {}, large: {}",
            small.len(),
            large.len()
        );
    }

    /// Ten thousand failures cost nothing in pixels and must still be bounded.
    #[test]
    fn an_entry_that_holds_nothing_still_costs_something() {
        let budget = 100 * ENTRY_OVERHEAD;
        let mut cache = cache(budget);
        for key in 0..10_000 {
            cache.begin_frame();
            cache.insert(key, 0);
        }
        assert!(cache.bytes() <= budget);
        assert!(cache.len() <= 100, "{} entries", cache.len());
    }

    /// `peek` instead of `get` is a one-word mistake with no symptom until a
    /// folder large enough to thrash.
    #[test]
    fn the_entry_nobody_looked_at_is_the_one_dropped() {
        let mut cache = cache(3 * (1000 + ENTRY_OVERHEAD));
        cache.begin_frame();
        for key in 0..3 {
            cache.insert(key, 1000);
        }

        cache.begin_frame();
        assert!(cache.get(&0).is_some(), "0 is looked at, so it is recent");
        cache.insert(3, 1000);

        assert!(cache.contains(&0), "the one that was read survives");
        assert!(!cache.contains(&1), "the one nobody read went");
    }

    /// Evicting a tile that is on screen only makes the next frame ask for it
    /// again — a decode loop on nothing more exotic than a large window.
    #[test]
    fn nothing_drawn_this_frame_is_evicted() {
        let mut cache = cache(2 * (1000 + ENTRY_OVERHEAD));
        cache.begin_frame();
        for key in 0..4 {
            cache.insert(key, 1000);
        }
        assert_eq!(cache.len(), 4, "all four are this frame's, so none went");
        assert!(cache.bytes() > cache.budget(), "over budget, deliberately");

        // Next frame, none of them is protected any more.
        cache.begin_frame();
        cache.insert(9, 1000);
        assert!(cache.bytes() <= cache.budget());
    }

    #[test]
    fn the_budget_grows_with_what_is_on_screen() {
        let mut cache = cache(1_000);
        cache.reserve_for(50_000, 100_000);
        assert_eq!(cache.budget(), 50_000);
        cache.reserve_for(500_000, 100_000);
        assert_eq!(cache.budget(), 100_000, "and stops at the ceiling");
    }

    /// A rename is the same file under a new name. Re-decoding four hundred
    /// photographs to learn that is the app's commonest workflow made to look
    /// broken.
    #[test]
    fn an_entry_can_move_to_a_new_key_without_being_rebuilt() {
        let mut cache = cache(10_000);
        cache.begin_frame();
        cache.insert(1, 500);
        let before = cache.bytes();

        assert!(cache.rekey(&1, 2));
        assert!(!cache.contains(&1));
        assert_eq!(cache.get(&2).copied(), Some(500));
        assert_eq!(cache.bytes(), before, "a move, not a copy");

        assert!(!cache.rekey(&99, 100), "nothing to move is not an error");
    }

    #[test]
    fn clearing_returns_everything() {
        let mut cache = cache(10_000);
        cache.begin_frame();
        cache.insert(1, 500);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }
}
