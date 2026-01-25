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
            let inner = self.inner.read().unwrap();
            match inner.entries.get(key) {
                Some(ref entry) => {
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
        result
    }

    /// Remove an entry from the cache
    pub fn remove(&self, key: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.entries.remove(key);
    }

    pub fn insert(&self, key: String, data: Vec<u8>, ttl: u64) {
        let old_head;
        {
            let mut inner = self.inner.write().unwrap();
            let entry = Entry {
                data,
                expires_at: Instant::now() + Duration::from_secs(ttl),
                prev: None,
                next: None,
            };
            inner.entries.insert(key.clone(), entry);
            if inner.entries.len() < 1 {
                inner.tail = Some(key);
            }

            match inner.head {
                Some(ref head) => {
                    old_head = head;
                }
                None => {}
            }
        }
        /// Continue to implement the insert method
        if !old_head.is_empty() {
            inner.head = Some(key.clone());
        }
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
    prev: Option<String>,
    next: Option<String>,
}

fn test() {}
