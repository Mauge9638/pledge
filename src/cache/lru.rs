use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

pub struct Cache {
    inner: Arc<RwLock<CacheInner>>,
}

impl Cache {
    pub fn new(max_cache_size_mib: u64) -> Self {
        let inner = Arc::new(RwLock::new(CacheInner {
            entries: HashMap::new(),
            head: None,
            tail: None,
            max_cache_size_mib,
        }));
        Cache { inner }
    }

    /// Get an entry from the cache
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let should_remove;
        let result;
        {
            let inner_read = self.inner.read().unwrap();
            match inner_read.entries.get(key) {
                Some(entry) => {
                    if entry.expires_at < Instant::now() {
                        should_remove = true;
                        result = None;
                    } else {
                        should_remove = false;
                        result = Some(entry.data.clone());
                    }
                }
                None => {
                    should_remove = false;
                    result = None
                }
            }
        }
        if should_remove {
            self.remove(key);
        }
        let mut inner_write = self.inner.write().unwrap();
        self.move_to_head_inner(&mut inner_write, key);
        result
    }

    /// Remove an entry from the cache
    pub fn remove(&self, key: &str) {
        let mut inner = self.inner.write().unwrap();

        let (entry_prev, entry_next) = {
            if let Some(entry) = inner.entries.get(key) {
                (entry.prev.clone(), entry.next.clone())
            } else {
                return;
            }
        };

        if &inner.head == &Some(key.to_string()) {
            inner.head = entry_next;
        }
        if &inner.tail == &Some(key.to_string()) {
            inner.tail = entry_prev;
        }

        self.unlink_from_neighbors_inner(&mut inner, key);
        inner.entries.remove(key);
    }

    pub fn insert(&self, key: String, data: Vec<u8>, ttl: u64) {
        let mut inner = self.inner.write().unwrap();

        if inner.entries.contains_key(&key) {
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.data = data;
                entry.expires_at = Instant::now() + Duration::from_secs(ttl);
            }
            self.move_to_head_inner(&mut inner, &key)
        } else {
            let entry = Entry {
                data,
                expires_at: Instant::now() + Duration::from_secs(ttl),
                prev: None,
                next: None,
            };
            inner.entries.insert(key.clone(), entry);
            self.move_to_head_inner(&mut inner, &key)
        }
    }

    /// Move an entry to the head of the LRU cache.
    /// It will update the head pointer (and possibly the tail pointer) and adjust the prev and next pointers of the entry.
    ///
    /// For params it need the mutable reference to the CacheInner and the key of the entry to move.
    ///
    /// The key must already be present in the cache.
    fn move_to_head_inner(&self, inner: &mut CacheInner, key: &str) {
        if !inner.entries.contains_key(key) || &inner.head == &Some(key.to_string()) {
            return;
        }

        let (old_tail_key_option, old_head_key_option) = (&inner.tail, inner.head.clone());

        if let Some(old_tail_key) = old_tail_key_option
            && let Some(new_head) = inner.entries.get(key)
        {
            if old_tail_key.eq(key) {
                inner.tail = new_head.prev.clone();
            }
        }
        self.unlink_from_neighbors_inner(inner, key);

        if old_head_key_option.is_some() {
            if let Some(new_head) = inner.entries.get_mut(key) {
                new_head.next = inner.head.clone();
            }
            inner.head = Some(key.to_string());
        } else {
            inner.head = Some(key.to_string());
            inner.tail = Some(key.to_string());
        }
        if let Some(old_head_key) = old_head_key_option {
            if let Some(old_head) = inner.entries.get_mut(&old_head_key) {
                old_head.prev = inner.head.clone();
            }
        }
    }
    fn unlink_from_neighbors_inner(&self, inner: &mut CacheInner, key: &str) {
        let (old_prev, old_next) = {
            if let Some(entry) = inner.entries.get_mut(key) {
                (entry.prev.take(), entry.next.take())
            } else {
                return;
            }
        };

        if let Some(old_prev_key) = &old_prev {
            if let Some(old_prev_entry) = inner.entries.get_mut(old_prev_key) {
                old_prev_entry.next = old_next.clone(); // clone here
            }
        }
        if let Some(old_next_key) = &old_next {
            if let Some(old_next_entry) = inner.entries.get_mut(old_next_key) {
                old_next_entry.prev = old_prev; // move here (last use)
            }
        }
    }

    fn unlink_inner(&self, inner: &mut CacheInner, key: &str) {
        let entry_to_unlink = inner.entries.get(key);
        if let Some(entry) = &entry_to_unlink {
            if let Some(head) = &inner.head {
                if head == key {
                    inner.head = entry.next.clone();
                }
            }
            if let Some(tail) = &inner.tail {
                if tail == key {
                    inner.tail = entry.prev.clone();
                }
            }
        }
        self.unlink_from_neighbors_inner(inner, key);
    }
}

struct CacheInner {
    entries: HashMap<String, Entry>,
    head: Option<String>,
    tail: Option<String>,
    max_cache_size_mib: u64,
}

struct Entry {
    data: Vec<u8>,
    expires_at: Instant,
    /// Towards the head of the list
    prev: Option<String>,
    /// Towards the tail of the list
    next: Option<String>,
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    // Helper: get head key for testing
    impl Cache {
        #[cfg(test)]
        fn head(&self) -> Option<String> {
            self.inner.read().unwrap().head.clone()
        }

        #[cfg(test)]
        fn tail(&self) -> Option<String> {
            self.inner.read().unwrap().tail.clone()
        }

        #[cfg(test)]
        fn len(&self) -> usize {
            self.inner.read().unwrap().entries.len()
        }

        #[cfg(test)]
        fn entry_next(&self, key: &str) -> Option<String> {
            self.inner
                .read()
                .unwrap()
                .entries
                .get(key)
                .and_then(|e| e.next.clone())
        }

        #[cfg(test)]
        fn entry_prev(&self, key: &str) -> Option<String> {
            self.inner
                .read()
                .unwrap()
                .entries
                .get(key)
                .and_then(|e| e.prev.clone())
        }
    }

    // === Basic Operations ===

    #[test]
    fn test_new_cache_is_empty() {
        let cache = Cache::new(10);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.head(), None);
        assert_eq!(cache.tail(), None);
    }

    #[test]
    fn test_insert_and_get() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 60);

        let result = cache.get("key1");
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_get_nonexistent_key() {
        let cache = Cache::new(10);
        assert_eq!(cache.get("missing"), None);
    }

    #[test]
    fn test_remove() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 60);

        cache.remove("key1");

        assert_eq!(cache.get("key1"), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_insert_duplicate_key_updates_value() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 60);
        cache.insert("key1".to_string(), b"updated".to_vec(), 60);

        assert_eq!(cache.get("key1"), Some(b"updated".to_vec()));
        assert_eq!(cache.len(), 1);
    }

    // === TTL Expiration ===

    #[test]
    fn test_ttl_expiration() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 1); // 1 second TTL

        // Should exist immediately
        assert_eq!(cache.get("key1"), Some(b"value1".to_vec()));

        // Wait for expiration
        sleep(Duration::from_secs(2));

        // Should be gone
        assert_eq!(cache.get("key1"), None);
    }

    // === Single Entry Edge Cases ===

    #[test]
    fn test_single_entry_is_head_and_tail() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 60);

        assert_eq!(cache.head(), Some("key1".to_string()));
        assert_eq!(cache.tail(), Some("key1".to_string()));
    }

    #[test]
    fn test_single_entry_has_no_neighbors() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"value1".to_vec(), 60);

        assert_eq!(cache.entry_prev("key1"), None);
        assert_eq!(cache.entry_next("key1"), None);
    }

    // === LRU Order on Insert ===

    #[test]
    fn test_insert_order_newest_is_head() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        assert_eq!(cache.head(), Some("key3".to_string()));
        assert_eq!(cache.tail(), Some("key1".to_string()));
    }

    #[test]
    fn test_insert_order_linked_list_integrity() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        // Order should be: key3 (head) <-> key2 <-> key1 (tail)

        // key3 is head, no prev, next is key2
        assert_eq!(cache.entry_prev("key3"), None);
        assert_eq!(cache.entry_next("key3"), Some("key2".to_string()));

        // key2 in middle
        assert_eq!(cache.entry_prev("key2"), Some("key3".to_string()));
        assert_eq!(cache.entry_next("key2"), Some("key1".to_string()));

        // key1 is tail, prev is key2, no next
        assert_eq!(cache.entry_prev("key1"), Some("key2".to_string()));
        assert_eq!(cache.entry_next("key1"), None);
    }

    // === LRU Order on Get (Move to Head) ===

    #[test]
    fn test_get_moves_to_head() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Access key1 (tail)
        cache.get("key1");

        // Now key1 should be head
        assert_eq!(cache.head(), Some("key1".to_string()));
        assert_eq!(cache.tail(), Some("key2".to_string())); // key2 is now tail
    }

    #[test]
    fn test_get_middle_element_moves_to_head() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Access key2 (middle)
        cache.get("key2");

        // Now: key2 (head) <-> key3 <-> key1 (tail)
        assert_eq!(cache.head(), Some("key2".to_string()));
        assert_eq!(cache.tail(), Some("key1".to_string()));

        // Verify links
        assert_eq!(cache.entry_next("key2"), Some("key3".to_string()));
        assert_eq!(cache.entry_next("key3"), Some("key1".to_string()));
    }

    #[test]
    fn test_get_head_does_not_change_order() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);

        // key2 is head
        cache.get("key2");

        // Should still be head
        assert_eq!(cache.head(), Some("key2".to_string()));
        assert_eq!(cache.tail(), Some("key1".to_string()));
    }

    // === Duplicate Key Moves to Head ===

    #[test]
    fn test_insert_duplicate_moves_to_head() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Re-insert key1
        cache.insert("key1".to_string(), b"v1_updated".to_vec(), 60);

        // key1 should now be head
        assert_eq!(cache.head(), Some("key1".to_string()));
        assert_eq!(cache.get("key1"), Some(b"v1_updated".to_vec()));
    }

    // === Remove Updates Linked List ===

    #[test]
    fn test_remove_head_updates_head() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);

        // key2 is head
        cache.remove("key2");

        assert_eq!(cache.head(), Some("key1".to_string()));
        assert_eq!(cache.tail(), Some("key1".to_string()));
    }

    #[test]
    fn test_remove_tail_updates_tail() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);

        // key1 is tail
        cache.remove("key1");

        assert_eq!(cache.head(), Some("key2".to_string()));
        assert_eq!(cache.tail(), Some("key2".to_string()));
    }

    #[test]
    fn test_remove_middle_links_neighbors() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);
        cache.insert("key2".to_string(), b"v2".to_vec(), 60);
        cache.insert("key3".to_string(), b"v3".to_vec(), 60);

        // Order: key3 <-> key2 <-> key1
        cache.remove("key2");

        // key3 and key1 should be linked
        assert_eq!(cache.entry_next("key3"), Some("key1".to_string()));
        assert_eq!(cache.entry_prev("key1"), Some("key3".to_string()));
    }

    #[test]
    fn test_remove_last_entry() {
        let cache = Cache::new(10);
        cache.insert("key1".to_string(), b"v1".to_vec(), 60);

        cache.remove("key1");

        assert_eq!(cache.len(), 0);
        assert_eq!(cache.head(), None);
        assert_eq!(cache.tail(), None);
    }
}
