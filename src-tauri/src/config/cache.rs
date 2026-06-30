use crate::config::schema::AppConfig;
use crate::error::AppError;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// In-memory config cache using wait-free atomic pointer swaps.
/// Optimized for "very frequent reads, very rare writes" pattern.
#[derive(Clone)]
pub struct ConfigCache {
    snapshot: Arc<ArcSwap<AppConfig>>,
}

impl ConfigCache {
    /// Create a new cache with the given initial config.
    pub fn new(initial: AppConfig) -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::new(Arc::new(initial))),
        }
    }

    /// Read from the in-memory cache.
    /// Returns an `Arc<AppConfig>` — callers can clone the Arc (atomic refcount bump)
    /// or deref to `&AppConfig` for read-only access.
    pub fn read_cached(&self) -> Arc<AppConfig> {
        self.snapshot.load_full()
    }

    /// Save to disk AND atomically update the in-memory cache.
    pub fn save_cached(&self, config: &AppConfig) -> Result<(), AppError> {
        config.save()?;
        self.snapshot.store(Arc::new(config.clone()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_reads_initial_config() {
        let config = AppConfig {
            hotkey: "F9".to_string(),
            ..Default::default()
        };
        let cache = ConfigCache::new(config.clone());
        let cached = cache.read_cached();
        assert_eq!(cached.hotkey, "F9");
    }
}
