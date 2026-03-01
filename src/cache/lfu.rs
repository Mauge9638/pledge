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

pub(super) struct CacheInner {
    pub(super) entries: HashMap<String, Entry>,
    pub(super) current_size_bytes: u64,
    pub(super) max_size_bytes: u64,
}

impl CacheInner {
    pub(super) fn new(max_size_bytes: u64) -> Self {
        CacheInner {
            entries: HashMap::new(),
            current_size_bytes: 0,
            max_size_bytes,
        }
    }

    /// Returns 2 values in a tuple
    ///
    /// The first value is the data, the second value is a boolean indicating if the entry should be removed
    pub(super) fn get(&self, key: &str) -> (Option<Vec<u8>>, bool) {
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

    pub(super) fn remove(&mut self, key: &str) {
        let entry_option = self.entries.get(key);
        if let Some(entry) = entry_option {
            self.current_size_bytes -= entry.get_size(key);
            self.entries.remove(key);
        }
    }

    pub(super) fn insert(&mut self, key: String, data: Vec<u8>, ttl: Instant) {
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

    pub(super) fn evict_lfu(&mut self) {
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

pub(super) struct Entry {
    pub(super) data: Vec<u8>,
    pub(super) expires_at: Instant,
    pub(super) counter: AtomicU64,
}

impl Entry {
    pub(super) fn get_size(&self, key: &str) -> u64 {
        self.data.len() as u64
            + key.len() as u64
            + std::mem::size_of_val(&self.counter) as u64
            + std::mem::size_of_val(&self.expires_at) as u64
    }
}
