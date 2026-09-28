//! Shared application state.

use std::sync::Arc;

use recall_core::SharedStore;

use crate::metrics::Metrics;
use crate::ratelimit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub store: SharedStore,
    pub embedder: Arc<dyn recall_core::Embedder>,
    pub api_key: Option<String>,
    /// Per-key token-bucket limiter; `None` disables limiting entirely.
    pub rate_limiter: Option<Arc<RateLimiter>>,
    /// Registry rendered at `/metrics`.
    pub metrics: Arc<Metrics>,
    /// Explicitly allowed cross-origin browser origins (AR-001 opt-in,
    /// `--allow-origin`). Empty = same-origin only.
    pub allowed_origins: Vec<String>,
    /// Extra Host header values the guard accepts besides loopback hosts
    /// (AR-001 opt-in, `--allow-host`).
    pub allowed_hosts: Vec<String>,
}

impl AppState {
    pub fn new(
        store: SharedStore,
        embedder: Arc<dyn recall_core::Embedder>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            store,
            embedder,
            api_key,
            rate_limiter: None,
            metrics: Arc::new(Metrics::new()),
            allowed_origins: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }

    /// Run `f` with exclusive access to the store, recovering from a poisoned
    /// mutex (a panicked handler must not permanently brick the server).
    pub fn with_store<R>(&self, f: impl FnOnce(&dyn recall_core::Store) -> R) -> R {
        let mut guard = self
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut *guard)
    }
}
