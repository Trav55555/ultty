//! Bounded cache of uploaded terminal-image textures.
//!
//! Every terminal numbers its images from 0, so an image is identified by
//! the terminal it belongs to (`Terminal::uid`) together with its
//! [`ImageId`]. The cache holds at most [`MAX_IMAGE_CACHE_BYTES`] of pixel
//! data and evicts the least recently used entry first.

use std::collections::{HashMap, HashSet};

use crate::grid::ImageId;

/// Pixel bytes the cache may hold before it evicts. Cached textures are
/// RGBA8, so this is also their GPU footprint.
pub(super) const MAX_IMAGE_CACHE_BYTES: usize = 256 * 1024 * 1024;

/// Which image a cached texture shows: the owning terminal and its image id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ImageKey {
    pub(super) owner: u64,
    pub(super) image: ImageId,
}

impl ImageKey {
    pub(super) fn new(owner: u64, image: ImageId) -> Self {
        Self { owner, image }
    }
}

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

/// An LRU cache with a byte budget.
pub(super) struct ImageCache<V> {
    entries: HashMap<ImageKey, Entry<V>>,
    total_bytes: usize,
    max_bytes: usize,
    clock: u64,
}

impl<V> ImageCache<V> {
    pub(super) fn new() -> Self {
        Self::with_budget(MAX_IMAGE_CACHE_BYTES)
    }

    pub(super) fn with_budget(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            max_bytes,
            clock: 0,
        }
    }

    pub(super) fn contains(&self, key: &ImageKey) -> bool {
        self.entries.contains_key(key)
    }

    /// The cached value, marked as most recently used.
    pub(super) fn get(&mut self, key: &ImageKey) -> Option<&V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(&entry.value)
    }

    /// Cache `value`, which weighs `bytes`, then evict least recently used
    /// entries until the budget holds. The new entry is never evicted, so a
    /// single image larger than the budget still draws.
    pub(super) fn insert(&mut self, key: ImageKey, value: V, bytes: usize) {
        self.clock += 1;
        let entry = Entry {
            value,
            bytes,
            last_used: self.clock,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.total_bytes -= old.bytes;
        }
        self.total_bytes += bytes;
        while self.total_bytes > self.max_bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.total_bytes -= evicted.bytes;
            }
        }
    }

    /// Keep only the entries of terminals in `live`.
    pub(super) fn retain_owners(&mut self, live: &HashSet<u64>) {
        let mut freed = 0;
        self.entries.retain(|key, entry| {
            let keep = live.contains(&key.owner);
            if !keep {
                freed += entry.bytes;
            }
            keep
        });
        self.total_bytes -= freed;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(owner: u64, image: u32) -> ImageKey {
        ImageKey::new(owner, ImageId(image))
    }

    #[test]
    fn same_image_id_of_two_terminals_are_two_entries() {
        let mut cache = ImageCache::with_budget(1000);
        cache.insert(key(1, 0), "tab one", 10);
        cache.insert(key(2, 0), "tab two", 10);
        assert_eq!(cache.get(&key(1, 0)), Some(&"tab one"));
        assert_eq!(cache.get(&key(2, 0)), Some(&"tab two"));
    }

    #[test]
    fn evicts_least_recently_used_past_the_byte_budget() {
        let mut cache = ImageCache::with_budget(100);
        cache.insert(key(1, 0), 'a', 40);
        cache.insert(key(1, 1), 'b', 40);
        // Touch `a`, so `b` is now the oldest.
        cache.get(&key(1, 0));
        cache.insert(key(1, 2), 'c', 40);
        assert!(cache.contains(&key(1, 0)));
        assert!(!cache.contains(&key(1, 1)));
        assert!(cache.contains(&key(1, 2)));
    }

    #[test]
    fn an_image_over_the_budget_is_still_kept_alone() {
        let mut cache = ImageCache::with_budget(10);
        cache.insert(key(1, 0), 'a', 5);
        cache.insert(key(1, 1), 'b', 50);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&key(1, 1)));
    }

    #[test]
    fn replacing_an_entry_does_not_double_count_its_bytes() {
        let mut cache = ImageCache::with_budget(100);
        cache.insert(key(1, 0), 'a', 60);
        cache.insert(key(1, 0), 'b', 60);
        cache.insert(key(1, 1), 'c', 30);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn closed_terminals_lose_their_entries_and_free_their_bytes() {
        let mut cache = ImageCache::with_budget(100);
        cache.insert(key(1, 0), 'a', 60);
        cache.insert(key(2, 0), 'b', 30);
        cache.retain_owners(&HashSet::from([2]));
        assert!(!cache.contains(&key(1, 0)));
        assert!(cache.contains(&key(2, 0)));
        // The freed 60 bytes are available again.
        cache.insert(key(2, 1), 'c', 60);
        assert_eq!(cache.len(), 2);
    }
}
