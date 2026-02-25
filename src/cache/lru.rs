use std::{
    array,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, RwLock},
    time::Instant,
};

pub struct Cache<const N: usize> {
    inner: Arc<[RwLock<CacheInner>; N]>,
}

impl<const N: usize> Cache<N> {
    pub fn new(max_cache_size_mib: u64) -> Self {
        let max_shard_size = (max_cache_size_mib * 1024 * 1024) / (N as u64);
        let inner = Arc::new(array::from_fn::<_, N, _>(|_| {
            RwLock::new(CacheInner::new(max_shard_size))
        }));

        Cache { inner }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let shard = self.get_cache_shard(key);
        let mut inner = shard.write().unwrap();
        inner.get(key)
    }

    pub fn remove(&self, key: &str) {
        let shard = self.get_cache_shard(key);
        let mut inner = shard.write().unwrap();
        inner.remove(key);
    }

    pub fn insert(&self, key: String, data: Vec<u8>, ttl: Instant) {
        let shard = self.get_cache_shard(&key);
        let mut inner = shard.write().unwrap();
        inner.insert(key, data, ttl);
    }

    pub fn entry_count(&self) -> u64 {
        let mut total_entries: u64 = 0;
        self.inner
            .iter()
            .for_each(|inner| total_entries += inner.read().unwrap().entries.len() as u64);
        total_entries
    }
    pub fn cache_size_bytes(&self) -> u64 {
        let mut total_bytes: u64 = 0;
        self.inner
            .iter()
            .for_each(|inner| total_bytes += inner.read().unwrap().current_size_bytes);
        total_bytes
    }

    fn get_cache_shard(&self, key: &str) -> &RwLock<CacheInner> {
        let cache_shard_key = self.get_cache_shard_key(&key);
        &self.inner[cache_shard_key]
    }

    fn get_cache_shard_key<T: Hash>(&self, key: &T) -> usize {
        let mut s = DefaultHasher::new();
        key.hash(&mut s);
        s.finish() as usize % N
    }
}

struct CacheInner {
    entries: HashMap<String, Entry>,
    head: Option<String>,
    tail: Option<String>,
    current_size_bytes: u64,
    max_size_bytes: u64,
}

impl CacheInner {
    fn new(max_size_bytes: u64) -> Self {
        CacheInner {
            entries: HashMap::new(),
            head: None,
            tail: None,
            current_size_bytes: 0,
            max_size_bytes,
        }
    }

    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        let entry = self.entries.get(key)?;

        if entry.expires_at < Instant::now() {
            self.remove(key);
            return None;
        }

        let data = entry.data.clone();
        self.move_to_head(key);
        Some(data)
    }

    fn remove(&mut self, key: &str) {
        let entry_option = self.entries.get(key);
        if let Some(entry) = entry_option {
            self.current_size_bytes -= entry.get_size(key);
            self.unlink(key);
            self.entries.remove(key);
        }
    }

    fn insert(&mut self, key: String, data: Vec<u8>, ttl: Instant) {
        if self.entries.contains_key(&key) {
            if let Some(entry) = self.entries.get_mut(&key) {
                let old_entry_size = entry.get_size(&key);
                entry.data = data;
                entry.expires_at = ttl;
                let new_entry_size = entry.get_size(&key);
                while self.current_size_bytes + new_entry_size
                    > self.max_size_bytes + old_entry_size
                {
                    self.evict_tail();
                }
                self.current_size_bytes += new_entry_size;
                self.current_size_bytes -= old_entry_size;
                self.move_to_head(&key);
            }
        } else {
            let entry = Entry {
                data,
                expires_at: ttl,
                prev: None,
                next: None,
            };
            let entry_size = entry.get_size(&key);
            while self.current_size_bytes + entry_size > self.max_size_bytes {
                self.evict_tail();
            }
            self.entries.insert(key.clone(), entry);
            self.move_to_head(&key);
            self.current_size_bytes += entry_size;
        }
    }

    /// Move an entry to the head of the LRU cache.
    /// It will update the head pointer (and possibly the tail pointer) and adjust the prev and next pointers of the entry.
    ///
    /// For params it need the mutable reference to the CacheInner and the key of the entry to move.
    ///
    /// The key must already be present in the cache.
    fn move_to_head(&mut self, key: &str) {
        if !self.entries.contains_key(key) || &self.head == &Some(key.to_string()) {
            return;
        }

        let old_head_key_option = self.head.clone();

        self.unlink(key);

        if old_head_key_option.is_some() {
            if let Some(new_head) = self.entries.get_mut(key) {
                new_head.next = self.head.clone();
            }
            self.head = Some(key.to_string());
        } else {
            self.head = Some(key.to_string());
            self.tail = Some(key.to_string());
        }
        if let Some(old_head_key) = old_head_key_option {
            if let Some(old_head) = self.entries.get_mut(&old_head_key) {
                old_head.prev = self.head.clone();
            }
        }
    }

    fn unlink_from_neighbors(&mut self, key: &str) {
        let (old_prev, old_next) = {
            if let Some(entry) = self.entries.get_mut(key) {
                (entry.prev.take(), entry.next.take())
            } else {
                return;
            }
        };

        if let Some(old_prev_key) = &old_prev {
            if let Some(old_prev_entry) = self.entries.get_mut(old_prev_key) {
                old_prev_entry.next = old_next.clone(); // clone here
            }
        }
        if let Some(old_next_key) = &old_next {
            if let Some(old_next_entry) = self.entries.get_mut(old_next_key) {
                old_next_entry.prev = old_prev; // move here (last use)
            }
        }
    }

    fn unlink(&mut self, key: &str) {
        let entry_to_unlink = self.entries.get(key);
        if let Some(entry) = &entry_to_unlink {
            if let Some(head) = &self.head {
                if head.eq(key) {
                    self.head = entry.next.clone();
                }
            }
            if let Some(tail) = &self.tail {
                if tail.eq(key) {
                    self.tail = entry.prev.clone();
                }
            }
        }
        self.unlink_from_neighbors(key);
    }

    fn evict_tail(&mut self) {
        let old_tail_key: String;
        match &self.tail {
            Some(key) => old_tail_key = key.clone(),
            None => return,
        }
        self.remove(&old_tail_key);
    }
}

struct Entry {
    data: Vec<u8>,
    expires_at: Instant,
    /// Towards the head of the list
    prev: Option<String>,
    /// Towards the tail of the list
    next: Option<String>,
}

impl Entry {
    fn get_size(&self, key: &str) -> u64 {
        let data_size = self.data.len() as u64;

        data_size + (key.len() * 3) as u64
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread::sleep, time::Duration};

    // === Basic Operations ===

    #[test]
    fn test_new_cache_is_empty() {
        let inner = CacheInner::new(10 * 1024 * 1024);
        assert_eq!(inner.entries.len(), 0);
        assert_eq!(inner.head, None);
        assert_eq!(inner.tail, None);
    }

    #[test]
    fn test_insert_and_get() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        let result = inner.get("key1");
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_get_nonexistent_key() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        assert_eq!(inner.get("missing"), None);
    }

    #[test]
    fn test_remove() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.remove("key1");

        assert_eq!(inner.get("key1"), None);
        assert_eq!(inner.entries.len(), 0);
    }

    #[test]
    fn test_insert_duplicate_key_updates_value() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key1".to_string(),
            b"updated".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        assert_eq!(inner.get("key1"), Some(b"updated".to_vec()));
        assert_eq!(inner.entries.len(), 1);
    }

    // === TTL Expiration ===

    #[test]
    fn test_ttl_expiration() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(1),
        ); // 1 second TTL

        // Should exist immediately
        assert_eq!(inner.get("key1"), Some(b"value1".to_vec()));

        // Wait for expiration
        sleep(Duration::from_secs(2));

        // Should be gone
        assert_eq!(inner.get("key1"), None);
    }

    // === Single Entry Edge Cases ===

    #[test]
    fn test_single_entry_is_head_and_tail() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        assert_eq!(inner.head, Some("key1".to_string()));
        assert_eq!(inner.tail, Some("key1".to_string()));
    }

    #[test]
    fn test_single_entry_has_no_neighbors() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"value1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        assert_eq!(inner.entries.get("key1").unwrap().prev, None);
        assert_eq!(inner.entries.get("key1").unwrap().next, None);
    }

    // === LRU Order on Insert ===

    #[test]
    fn test_insert_order_newest_is_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        assert_eq!(inner.head, Some("key3".to_string()));
        assert_eq!(inner.tail, Some("key1".to_string()));
    }

    #[test]
    fn test_insert_order_linked_list_integrity() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // Order should be: key3 (head) <-> key2 <-> key1 (tail)

        // key3 is head, no prev, next is key2
        assert_eq!(inner.entries.get("key3").unwrap().prev, None);
        assert_eq!(
            inner.entries.get("key3").unwrap().next,
            Some("key2".to_string())
        );

        // key2 in middle
        assert_eq!(
            inner.entries.get("key2").unwrap().prev,
            Some("key3".to_string())
        );
        assert_eq!(
            inner.entries.get("key2").unwrap().next,
            Some("key1".to_string())
        );

        // key1 is tail, prev is key2, no next
        assert_eq!(
            inner.entries.get("key1").unwrap().prev,
            Some("key2".to_string())
        );
        assert_eq!(inner.entries.get("key1").unwrap().next, None);
    }

    // === LRU Order on Get (Move to Head) ===

    #[test]
    fn test_get_moves_to_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Access key1 (tail)
        inner.get("key1");

        // Now key1 should be head
        assert_eq!(inner.head, Some("key1".to_string()));
        assert_eq!(inner.tail, Some("key2".to_string())); // key2 is now tail
    }

    #[test]
    fn test_get_middle_element_moves_to_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Access key2 (middle)
        inner.get("key2");

        // Now: key2 (head) <-> key3 <-> key1 (tail)
        assert_eq!(inner.head, Some("key2".to_string()));
        assert_eq!(inner.tail, Some("key1".to_string()));

        // Verify links
        assert_eq!(
            inner.entries.get("key3").unwrap().next,
            Some("key1".to_string())
        );
        assert_eq!(
            inner.entries.get("key2").unwrap().next,
            Some("key3".to_string())
        );
    }

    #[test]
    fn test_get_head_does_not_change_order() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // key2 is head
        inner.get("key2");

        // Should still be head
        assert_eq!(inner.head, Some("key2".to_string()));
        assert_eq!(inner.tail, Some("key1".to_string()));
    }

    // === Duplicate Key Moves to Head ===

    #[test]
    fn test_insert_duplicate_moves_to_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // Order: key3 (head) <-> key2 <-> key1 (tail)

        // Re-insert key1
        inner.insert(
            "key1".to_string(),
            b"v1_updated".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // key1 should now be head
        assert_eq!(inner.head, Some("key1".to_string()));
        assert_eq!(inner.get("key1"), Some(b"v1_updated".to_vec()));
    }

    // === Remove Updates Linked List ===

    #[test]
    fn test_remove_head_updates_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // key2 is head
        inner.remove("key2");

        assert_eq!(inner.head, Some("key1".to_string()));
        assert_eq!(inner.tail, Some("key1".to_string()));
    }

    #[test]
    fn test_remove_tail_updates_tail() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // key1 is tail
        inner.remove("key1");

        assert_eq!(inner.head, Some("key2".to_string()));
        assert_eq!(inner.tail, Some("key2".to_string()));
    }

    #[test]
    fn test_remove_middle_links_neighbors() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        // Order: key3 <-> key2 <-> key1
        inner.remove("key2");

        // key3 and key1 should be linked
        assert_eq!(
            inner.entries.get("key3").unwrap().next,
            Some("key1".to_string())
        );
        assert_eq!(
            inner.entries.get("key1").unwrap().prev,
            Some("key3".to_string())
        );
    }

    #[test]
    fn test_remove_last_entry() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.remove("key1");

        assert_eq!(inner.entries.len(), 0);
        assert_eq!(inner.head, None);
        assert_eq!(inner.tail, None);
    }

    #[test]
    fn test_evict_tail_removes_least_recent() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key2".to_string(),
            b"v2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            b"v3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // Order: key3 (head) <-> key2 <-> key1 (tail)

        inner.evict_tail();

        assert_eq!(inner.get("key1"), None); // evicted
        assert_eq!(inner.tail, Some("key2".to_string()));
        assert_eq!(inner.entries.get("key2").unwrap().next, None); // new tail has no next
        assert_eq!(inner.entries.len(), 2);
    }

    #[test]
    fn test_evict_tail_single_entry() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            b"v1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.evict_tail();

        assert_eq!(inner.entries.len(), 0);
        assert_eq!(inner.head, None);
        assert_eq!(inner.tail, None);
    }

    #[test]
    fn test_evict_tail_empty_cache() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.evict_tail(); // should not panic
        assert_eq!(inner.entries.len(), 0);
    }

    // === Full integrity after complex operations ===

    #[test]
    fn test_insert_get_remove_sequence() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // c (head) <-> b <-> a (tail)

        inner.get("a"); // move a to head
        // a (head) <-> c <-> b (tail)

        inner.remove("c"); // remove middle
        // a (head) <-> b (tail)

        assert_eq!(inner.head, Some("a".to_string()));
        assert_eq!(inner.tail, Some("b".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().prev, Some("a".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().prev, None);
        assert_eq!(inner.entries.get("b").unwrap().next, None);
        assert_eq!(inner.entries.len(), 2);
    }

    #[test]
    fn test_multiple_gets_maintain_order() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // c (head) <-> b <-> a (tail)

        inner.get("a");
        inner.get("b");
        // b (head) <-> a <-> c (tail)

        assert_eq!(inner.head, Some("b".to_string()));
        assert_eq!(inner.tail, Some("c".to_string()));
    }

    #[test]
    fn test_five_entries_insert_order() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e (head) <-> d <-> c <-> b <-> a (tail)

        assert_eq!(inner.head, Some("e".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));
        assert_eq!(inner.entries.len(), 5);

        // Full chain forward (head to tail via next)
        assert_eq!(inner.entries.get("e").unwrap().prev, None);
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().prev, Some("e".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("c".to_string()));
        assert_eq!(inner.entries.get("c").unwrap().prev, Some("d".to_string()));
        assert_eq!(inner.entries.get("c").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().prev, Some("c".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, Some("a".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().prev, Some("b".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().next, None);
    }

    #[test]
    fn test_five_entries_get_tail_moves_to_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.get("a");
        // a (head) <-> e <-> d <-> c <-> b (tail)

        assert_eq!(inner.head, Some("a".to_string()));
        assert_eq!(inner.tail, Some("b".to_string()));

        assert_eq!(inner.entries.get("a").unwrap().prev, None);
        assert_eq!(inner.entries.get("a").unwrap().next, Some("e".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().prev, Some("a".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("c".to_string()));
        assert_eq!(inner.entries.get("c").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, None);
    }

    #[test]
    fn test_five_entries_get_second_from_tail() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.get("b");
        // b (head) <-> e <-> d <-> c <-> a (tail)

        assert_eq!(inner.head, Some("b".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));

        assert_eq!(inner.entries.get("b").unwrap().prev, None);
        assert_eq!(inner.entries.get("b").unwrap().next, Some("e".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("c".to_string()));
        assert_eq!(inner.entries.get("c").unwrap().next, Some("a".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().prev, Some("c".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().next, None);
    }

    #[test]
    fn test_five_entries_get_middle() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.get("c");
        // c (head) <-> e <-> d <-> b <-> a (tail)

        assert_eq!(inner.head, Some("c".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));

        assert_eq!(inner.entries.get("c").unwrap().prev, None);
        assert_eq!(inner.entries.get("c").unwrap().next, Some("e".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().prev, Some("c".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().prev, Some("d".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, Some("a".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().next, None);
    }

    #[test]
    fn test_five_entries_get_second_from_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.get("d");
        // d (head) <-> e <-> c <-> b <-> a (tail)

        assert_eq!(inner.head, Some("d".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));

        assert_eq!(inner.entries.get("d").unwrap().prev, None);
        assert_eq!(inner.entries.get("d").unwrap().next, Some("e".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().prev, Some("d".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().next, Some("c".to_string()));
        assert_eq!(inner.entries.get("c").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, Some("a".to_string()));
        assert_eq!(inner.entries.get("a").unwrap().next, None);
    }

    #[test]
    fn test_five_entries_remove_middle() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.remove("c");
        // e <-> d <-> b <-> a

        assert_eq!(inner.head, Some("e".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));
        assert_eq!(inner.entries.len(), 4);

        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().prev, Some("d".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, Some("a".to_string()));
        assert_eq!(inner.get("c"), None);
    }

    #[test]
    fn test_five_entries_remove_head() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.remove("e");
        // d <-> c <-> b <-> a

        assert_eq!(inner.head, Some("d".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));
        assert_eq!(inner.entries.len(), 4);
        assert_eq!(inner.entries.get("d").unwrap().prev, None);
        assert_eq!(inner.entries.get("d").unwrap().next, Some("c".to_string()));
    }

    #[test]
    fn test_five_entries_remove_tail() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.remove("a");
        // e <-> d <-> c <-> b

        assert_eq!(inner.head, Some("e".to_string()));
        assert_eq!(inner.tail, Some("b".to_string()));
        assert_eq!(inner.entries.len(), 4);
        assert_eq!(inner.entries.get("b").unwrap().next, None);
        assert_eq!(inner.entries.get("b").unwrap().prev, Some("c".to_string()));
    }

    #[test]
    fn test_five_entries_multiple_gets_then_evict() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.get("a"); // a to head
        inner.get("c"); // c to head
        // c <-> a <-> e <-> d <-> b (tail)

        inner.evict_tail();
        // c <-> a <-> e <-> d

        assert_eq!(inner.get("b"), None);
        assert_eq!(inner.entries.len(), 4);
        assert_eq!(inner.tail, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, None);
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
    }

    #[test]
    fn test_five_entries_insert_duplicate_from_middle() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // e <-> d <-> c <-> b <-> a

        inner.insert(
            "c".to_string(),
            b"updated".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        // c (head) <-> e <-> d <-> b <-> a (tail)

        assert_eq!(inner.head, Some("c".to_string()));
        assert_eq!(inner.tail, Some("a".to_string()));
        assert_eq!(inner.get("c"), Some(b"updated".to_vec()));
        assert_eq!(inner.entries.len(), 5);

        assert_eq!(inner.entries.get("c").unwrap().next, Some("e".to_string()));
        assert_eq!(inner.entries.get("e").unwrap().next, Some("d".to_string()));
        assert_eq!(inner.entries.get("d").unwrap().next, Some("b".to_string()));
        assert_eq!(inner.entries.get("b").unwrap().next, Some("a".to_string()));
    }

    #[test]
    fn test_five_entries_evict_all() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.insert(
            "a".to_string(),
            b"1".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "b".to_string(),
            b"2".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "c".to_string(),
            b"3".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "d".to_string(),
            b"4".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "e".to_string(),
            b"5".to_vec(),
            Instant::now() + Duration::from_secs(60),
        );

        inner.evict_tail(); // removes a
        inner.evict_tail(); // removes b
        inner.evict_tail(); // removes c
        inner.evict_tail(); // removes d
        inner.evict_tail(); // removes e

        assert_eq!(inner.entries.len(), 0);
        assert_eq!(inner.head, None);
        assert_eq!(inner.tail, None);
    }

    #[test]
    fn test_remove_nonexistent_key() {
        let mut cache = CacheInner::new(10 * 1024 * 1024);
        cache.remove("ghost"); // should not panic
    }

    #[test]
    fn test_concurrent_inserts_and_gets() {
        use std::thread;

        let cache = Arc::new(Cache::<6>::new(10));
        let mut handles = vec![];

        // Spawn writers
        for i in 0..100 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                cache.insert(
                    format!("key{}", i),
                    vec![i as u8],
                    Instant::now() + Duration::from_secs(60),
                );
            }));
        }

        // Spawn readers
        for i in 0..100 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                cache.get(&format!("key{}", i));
            }));
        }

        for h in handles {
            h.join().unwrap(); // no panics = no poisoned locks
        }
    }
}
