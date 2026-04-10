use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Persistent TPN cache that survives restarts via disk serialization.
/// Replaces the mentie-based .tpn_cache.json persistence.
pub struct TpnCache {
    data: RwLock<HashMap<String, Value>>,
    file_path: String,
}

impl TpnCache {
    pub fn new(file_path: &str) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            file_path: file_path.to_string(),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Value> {
        let data = self.data.read().await;
        data.get(key).cloned()
    }

    pub async fn get_or(&self, key: &str, default: Value) -> Value {
        self.get(key).await.unwrap_or(default)
    }

    pub async fn set(&self, key: &str, value: Value) {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), value);
    }

    pub async fn remove(&self, key: &str) {
        let mut data = self.data.write().await;
        data.remove(key);
    }

    /// Restore cache from disk.
    pub async fn restore_from_disk(&self) -> Result<()> {
        let path = Path::new(&self.file_path);
        if !path.exists() {
            info!("No cache file found at {}, starting fresh", self.file_path);
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path).await?;
        let parsed: HashMap<String, Value> = serde_json::from_str(&content)?;
        let count = parsed.len();
        let mut data = self.data.write().await;
        *data = parsed;
        info!("Restored {} keys from cache file {}", count, self.file_path);
        Ok(())
    }

    /// Save cache to disk.
    pub async fn save_to_disk(&self) -> Result<()> {
        let data = self.data.read().await;
        let content = serde_json::to_string_pretty(&*data)?;

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&self.file_path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&self.file_path, content).await?;
        info!("Saved {} keys to cache file {}", data.len(), self.file_path);
        Ok(())
    }

    /// Spawn periodic save task.
    pub fn spawn_save_task(self: &Arc<Self>) {
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                if let Err(e) = cache.save_to_disk().await {
                    warn!("Failed to save TPN cache to disk: {}", e);
                }
            }
        });
    }
}
