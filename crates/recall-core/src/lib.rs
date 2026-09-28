//! recall-core — pure domain layer for Recall-MCP.
//!
//! No HTTP, no MCP, no async runtime: models, the [`store::Store`] trait with a
//! SQLite implementation (FTS5 keyword index), embedders, and the explainable
//! hybrid scorer. Everything else in the workspace is a shell around this crate.

// Style guide (docs/style-guides/rust-tooling-conventions.md): curated clippy
// opt-ins with justified `#[allow]`s, never blanket pedantic noise.
#![warn(clippy::dbg_macro, clippy::todo, clippy::unimplemented)]
// No `unwrap`/`expect` outside tests (rust-error-handling.md item 5); each
// non-test use carries a local `#[allow]` naming the invariant that makes it
// provably infallible.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod capture;
pub mod crypto;
pub mod embed;
pub mod error;
pub mod export;
pub mod models;
#[cfg(feature = "onnx")]
pub mod onnx_embed;
#[cfg(feature = "openai")]
pub mod openai_embed;
pub mod scorer;
pub mod sqlite;
pub mod store;
pub mod util;

pub use embed::{Embedder, EmbedderKind, HashEmbedder, parse_embedder_name, select_embedder};
pub use error::{RecallError, Result};
pub use models::{
    Memory, MemoryUpdate, Namespace, NewMemory, RecallHit, RecallParams, ScoreBreakdown, Stats,
    Weights,
};
#[cfg(feature = "onnx")]
pub use onnx_embed::{OnnxEmbedder, OnnxModel};
pub use scorer::HybridScorer;
pub use sqlite::SqliteStore;
pub use store::Store;

/// Shared, thread-safe handle to a store, used by the server and CLI shells.
pub type SharedStore = std::sync::Arc<std::sync::Mutex<dyn Store>>;

/// Wrap a concrete store in the shared handle.
pub fn shared(store: SqliteStore) -> SharedStore {
    std::sync::Arc::new(std::sync::Mutex::new(store))
}

/// Current unix time in seconds.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_now_is_plausible() {
        let now = unix_now();
        // 2026-09-06 is ~1.786e9; guard against clock disasters both ways.
        assert!((1_700_000_000..2_500_000_000).contains(&now));
    }
}
