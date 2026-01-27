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
        result
    }

    /// Remove an entry from the cache
    pub fn remove(&self, key: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.entries.remove(key);
    }

    pub fn insert(&self, key: String, data: Vec<u8>, ttl: u64) {
        let mut inner = self.inner.write().unwrap();
        let old_head_key = inner.head.clone();

        // if inner.entries.is_empty() {
        //     inner.tail = Some(key.clone());
        // }

        // if let Some(ref old_key) = old_head_key {
        //     if let Some(old_entry) = inner.entries.get_mut(old_key) {
        //         old_entry.prev = Some(key.clone());
        //     }
        // }

        inner.head = Some(key.clone());

        if inner.entries.contains_key(&key) {
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.data = data;
                entry.expires_at = Instant::now() + Duration::from_secs(ttl);
            }
            self.move_to_head_inner(&mut inner, &key)
        } else {
            let entry = Entry {
                data: data.clone(),
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
    fn move_to_head_inner(&self, inner: &mut CacheInner, key: &str) {
        // TODO: Implement case where the key is the tail
        match &inner.head {
            Some(inner_head) => {
                if inner_head != key {
                    let old_head_key = inner_head.clone();
                    inner.head = Some(key.to_string());

                    if let Some(old_head) = inner.entries.get_mut(&old_head_key) {
                        old_head.prev = Some(key.to_string());
                        if old_head.next.is_none() {
                            inner.tail = Some(old_head_key.clone())
                        }
                    }
                    if let Some(new_head) = inner.entries.get_mut(key) {
                        new_head.next = Some(old_head_key);
                    }
                }
            }
            // If the head is None, set the head to the key and set the tail to the key
            None => {
                inner.head = Some(key.to_string());
                inner.tail = Some(key.to_string());
            }
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
    /// Towards the head of the list
    prev: Option<String>,
    /// Towards the tail of the list
    next: Option<String>,
}

fn test() {}
