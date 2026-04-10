pub mod tpn_cache;

use dashmap::DashMap;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A single cache entry with optional TTL.
struct CacheEntry {
    value: Value,
    expires_at: Option<Instant>,
}

/// TTL-based in-memory cache backed by DashMap.
/// Replaces the mentie `cache()` function.
pub struct TtlCache {
    store: DashMap<String, CacheEntry>,
}

impl TtlCache {
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
        }
    }

    /// Get a cached value by key. Returns None if missing or expired.
    pub fn get(&self, key: &str) -> Option<Value> {
        let entry = self.store.get(key)?;
        if let Some(expires_at) = entry.expires_at {
            if Instant::now() > expires_at {
                drop(entry);
                self.store.remove(key);
                return None;
            }
        }
        Some(entry.value.clone())
    }

    /// Get a cached value or return the provided default.
    pub fn get_or(&self, key: &str, default: Value) -> Value {
        self.get(key).unwrap_or(default)
    }

    /// Set a cached value with optional TTL in milliseconds.
    pub fn set(&self, key: &str, value: Value, ttl_ms: Option<u64>) {
        let expires_at = ttl_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        self.store
            .insert(key.to_string(), CacheEntry { value, expires_at });
    }

    /// Set a cached value without TTL (permanent until evicted).
    pub fn set_permanent(&self, key: &str, value: Value) {
        self.set(key, value, None);
    }

    /// Merge an array value into an existing array cache entry.
    /// If the key doesn't exist or isn't an array, creates a new array.
    pub fn merge(&self, key: &str, values: Vec<Value>) {
        let mut entry = self
            .store
            .entry(key.to_string())
            .or_insert_with(|| CacheEntry {
                value: Value::Array(vec![]),
                expires_at: None,
            });
        if let Value::Array(ref mut arr) = entry.value {
            arr.extend(values);
        }
    }

    /// Remove a key from the cache.
    pub fn remove(&self, key: &str) {
        self.store.remove(key);
    }

    /// Check if a key exists and is not expired.
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Number of entries currently in the cache (including expired but not yet evicted).
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Collect non-expired entries for keys matching a prefix.
    pub fn entries_with_prefix(&self, prefix: &str) -> Vec<(String, Value)> {
        let now = Instant::now();
        self.store
            .iter()
            .filter_map(|entry| {
                if !entry.key().starts_with(prefix) {
                    return None;
                }
                if let Some(expires_at) = entry.expires_at {
                    if now > expires_at {
                        return None;
                    }
                }
                Some((entry.key().clone(), entry.value.clone()))
            })
            .collect()
    }

    /// Evict all expired entries. Called periodically by background task.
    pub fn evict_expired(&self) {
        let now = Instant::now();
        let expired_keys: Vec<String> = self
            .store
            .iter()
            .filter_map(|entry| {
                if let Some(expires_at) = entry.expires_at {
                    if now > expires_at {
                        return Some(entry.key().clone());
                    }
                }
                None
            })
            .collect();
        for key in expired_keys {
            self.store.remove(&key);
        }
    }

    /// Spawn a background task to periodically evict expired entries.
    pub fn spawn_eviction_task(self: &Arc<Self>) {
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                cache.evict_expired();
            }
        });
    }
}

impl Default for TtlCache {
    fn default() -> Self {
        Self::new()
    }
}
