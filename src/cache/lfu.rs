use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

pub struct Cache {
    inner: Arc<Vec<RwLock<CacheInner>>>,
    shards: usize,
}

impl Cache {
    pub fn new(max_cache_size_mib: u64, cache_shards: usize) -> Self {
        let max_shard_size = (max_cache_size_mib * 1024 * 1024) / (cache_shards as u64);
        let inner = Arc::new(
            (0..cache_shards)
                .map(|_| RwLock::new(CacheInner::new(max_shard_size)))
                .collect::<Vec<_>>(),
        );

        Cache {
            inner,
            shards: cache_shards,
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let shard = self.get_cache_shard(key);
        let (data, should_remove): (Option<Vec<u8>>, bool);
        {
            let inner = shard.read().unwrap();
            (data, should_remove) = inner.get(key).clone();
        }
        if should_remove && data.is_none() {
            let mut mutable_inner = shard.write().unwrap();
            mutable_inner.remove(key);
        }
        data
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
        s.finish() as usize % self.shards
    }
}

struct CacheInner {
    entries: HashMap<String, Entry>,
    current_size_bytes: u64,
    max_size_bytes: u64,
}

impl CacheInner {
    fn new(max_size_bytes: u64) -> Self {
        CacheInner {
            entries: HashMap::new(),
            current_size_bytes: 0,
            max_size_bytes,
        }
    }

    /// Returns 2 values in a tuple
    ///
    /// The first value is the data, the second value is a boolean indicating if the entry should be removed
    fn get(&self, key: &str) -> (Option<Vec<u8>>, bool) {
        let entry = match self.entries.get(key) {
            Some(entry) => entry,
            None => return (None, false),
        };
        if entry.expires_at < Instant::now() {
            return (None, true);
        }
        entry.counter.fetch_add(1, Ordering::Relaxed);
        let data = entry.data.clone();
        (Some(data), false)
    }

    fn remove(&mut self, key: &str) {
        let entry_option = self.entries.get(key);
        if let Some(entry) = entry_option {
            self.current_size_bytes -= entry.get_size(key);
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
                    self.evict_lfu();
                }
                self.current_size_bytes += new_entry_size;
                self.current_size_bytes -= old_entry_size;
            }
        } else {
            let entry = Entry {
                data,
                expires_at: ttl,
                counter: AtomicU64::new(1),
            };
            let entry_size = entry.get_size(&key);
            while self.current_size_bytes + entry_size > self.max_size_bytes {
                self.evict_lfu();
            }
            self.entries.insert(key.clone(), entry);
            self.current_size_bytes += entry_size;
        }
    }

    fn evict_lfu(&mut self) {
        let (mut least_accessed_key, mut least_accessed_amount): (String, u64) =
            ("".to_string(), 0);
        self.entries.iter().for_each(|(key, entry)| {
            if (entry.counter.load(Ordering::Relaxed)) < (least_accessed_amount as u64)
                || least_accessed_key.is_empty()
            {
                (least_accessed_key, least_accessed_amount) =
                    (key.clone(), entry.counter.load(Ordering::Relaxed) as u64)
            }
        });
        if !least_accessed_key.is_empty() {
            self.remove(&least_accessed_key);
        }
    }
}

struct Entry {
    data: Vec<u8>,
    expires_at: Instant,
    counter: AtomicU64,
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

    fn helper_create_1mb_entry(key: &str, expires_in_seconds: u64) -> Entry {
        let one_mb_in_bytes: u32 = 1_048_576;
        let key_size = (key.len() * 3) as u32;
        let data = helper_create_1mb_vector(one_mb_in_bytes - key_size);

        Entry {
            data,
            counter: AtomicU64::new(0),
            expires_at: Instant::now() + Duration::from_secs(expires_in_seconds),
        }
    }
    fn helper_create_1mb_vector(size: u32) -> Vec<u8> {
        (0..size).map(|_| 255).collect()
    }

    #[test]
    fn test_new_cache_is_empty() {
        let inner = CacheInner::new(10 * 1024 * 1024);
        assert_eq!(inner.entries.len(), 0);
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
        assert_eq!(result, (Some(b"value1".to_vec()), false));
    }

    #[test]
    fn test_get_nonexistent_key() {
        let inner = CacheInner::new(10 * 1024 * 1024);
        assert_eq!(inner.get("missing"), (None, false));
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

        assert_eq!(inner.get("key1"), (None, false));
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

        assert_eq!(inner.get("key1"), (Some(b"updated".to_vec()), false));
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
        assert_eq!(inner.get("key1"), (Some(b"value1".to_vec()), false));

        // Wait for expiration
        sleep(Duration::from_secs(2));

        // Should be gone
        assert_eq!(inner.get("key1"), (None, true));
    }

    #[test]
    fn test_evict_lfu_empty_cache() {
        let mut inner = CacheInner::new(10 * 1024 * 1024);
        inner.evict_lfu(); // should not panic
        assert_eq!(inner.entries.len(), 0);
    }

    #[test]
    fn test_remove_nonexistent_key() {
        let mut cache = CacheInner::new(10 * 1024 * 1024);
        cache.remove("ghost"); // should not panic
    }

    #[test]
    fn test_full_cache_evicts_non_accessed() {
        let mut inner = CacheInner::new(4 * 1024 * 1024);
        inner.insert(
            "key1".to_string(),
            helper_create_1mb_entry("key1", 60).data,
            Instant::now() + Duration::from_secs(60),
        );

        inner.insert(
            "key2".to_string(),
            helper_create_1mb_entry("key2", 60).data,
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key3".to_string(),
            helper_create_1mb_entry("key3", 60).data,
            Instant::now() + Duration::from_secs(60),
        );
        inner.insert(
            "key4".to_string(),
            helper_create_1mb_entry("key4", 60).data,
            Instant::now() + Duration::from_secs(60),
        );

        inner.get("key2");
        for _ in 0..10 {
            inner.get("key3");
            inner.get("key4");
        }

        inner.insert(
            "key5".to_string(),
            helper_create_1mb_entry("key5", 60).data,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(inner.entries.len(), 4);
        assert_eq!(inner.entries.get("key1").is_none(), true);
        assert_eq!(inner.entries.get("key2").is_some(), true);
        assert_eq!(inner.entries.get("key3").is_some(), true);
        assert_eq!(inner.entries.get("key4").is_some(), true);
        assert_eq!(inner.entries.get("key5").is_some(), true);
    }

    #[test]
    fn test_concurrent_inserts_and_gets() {
        use std::thread;

        let cache = Arc::new(Cache::new(10, 6));
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
