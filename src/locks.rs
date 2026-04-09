use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Named mutex registry for application-level locking.
/// Replaces PostgreSQL advisory locks and the async-mutex npm package.
#[derive(Debug, Default)]
pub struct LockRegistry {
    locks: DashMap<String, Arc<Mutex<()>>>,
}

impl LockRegistry {
    pub fn new() -> Self {
        Self {
            locks: DashMap::new(),
        }
    }

    /// Try to acquire a named lock without blocking.
    /// Returns the guard if successful, None if the lock is already held.
    pub fn try_acquire(&self, name: &str) -> Option<OwnedMutexGuard<()>> {
        let mutex = self
            .locks
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        mutex.try_lock_owned().ok()
    }

    /// Acquire a named lock, blocking until available, then run the async closure.
    pub async fn with_lock<F, Fut, T>(&self, name: &str, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let mutex = self
            .locks
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = mutex.lock().await;
        f().await
    }
}
