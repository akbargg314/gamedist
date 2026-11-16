//! Shared application state.

use crate::{auth::Limiter, config::Config, metrics::Metrics, tracker::Tracker};
use gamedist_core::{manifest::ReleaseManifest, store::ContentStore};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct AppState {
    pub cfg: Config,
    pub db: SqlitePool,
    pub chunks: ContentStore,
    pub metrics: Metrics,
    pub auth_limiter: Limiter,
    pub upload_limiter: Limiter,
    pub tracker: Tracker,
    /// Parsed release manifests for the web-seed file layout, keyed by
    /// `(slug, version)`.
    manifests: Mutex<HashMap<(String, String), Arc<ReleaseManifest>>>,
}

pub type St = Arc<AppState>;

impl AppState {
    pub fn new(cfg: Config, db: SqlitePool, chunks: ContentStore) -> Self {
        AppState {
            auth_limiter: Limiter::new(cfg.auth_rate_per_minute),
            upload_limiter: Limiter::new(cfg.upload_rate_per_minute),
            tracker: Tracker::new(cfg.tracker_interval_secs),
            cfg,
            db,
            chunks,
            metrics: Metrics::default(),
            manifests: Mutex::new(HashMap::new()),
        }
    }

    pub fn cached_manifest(&self, slug: &str, version: &str) -> Option<Arc<ReleaseManifest>> {
        self.manifests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(slug.to_string(), version.to_string()))
            .cloned()
    }

    pub fn cache_manifest(&self, slug: &str, version: &str, manifest: Arc<ReleaseManifest>) {
        let mut cache = self.manifests.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert((slug.to_string(), version.to_string()), manifest);
    }

    /// Drop cached manifests of a game (unpublish, takedown).
    pub fn evict_manifests(&self, slug: &str) {
        self.manifests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(s, _), _| s != slug);
    }

    pub fn game_url(&self, slug: &str) -> String {
        format!("{}/games/{slug}", self.cfg.public_url)
    }
}
