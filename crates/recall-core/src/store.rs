//! Persistence abstraction. Sync by design: SQLite calls are sub-millisecond at
//! v1 scale, and a sync trait keeps the core testable without a runtime.
//!
//! Implementations are `Send` (rusqlite's `Connection` is not `Sync`), so
//! cross-thread sharing goes through [`crate::SharedStore`] (`Arc<Mutex<dyn Store>>`).

use crate::error::Result;
use crate::models::{Memory, MemoryUpdate, Namespace, NewMemory, RecallHit, RecallParams, Stats};

pub trait Store: Send {
    /// Create a namespace; errors with [`crate::error::RecallError::DuplicateNamespace`]
    /// when the name is taken.
    fn create_namespace(&self, name: &str) -> Result<Namespace>;
    /// Idempotent variant used by writers/importers.
    fn get_or_create_namespace(&self, name: &str) -> Result<Namespace>;
    fn list_namespaces(&self) -> Result<Vec<Namespace>>;
    fn find_namespace(&self, name: &str) -> Result<Option<Namespace>>;

    /// Insert a memory. `namespace` must exist; `text` must be non-empty after trimming.
    /// `embedding`, when supplied, must match the embedder dimensionality (checked by caller)
    /// and is stored as a BLOB.
    fn insert_memory(&self, memory: &NewMemory) -> Result<Memory>;
    fn get_memory(&self, id: &str) -> Result<Option<Memory>>;
    /// Update a memory in place (D-015): only the fields set on `update`
    /// change; `created_at` and `last_accessed_at` are preserved. Errors with
    /// [`crate::error::RecallError::MemoryNotFound`] for an unknown id and
    /// [`crate::error::RecallError::InvalidInput`] for an empty update or
    /// blank replacement text. When `text` changes without `embedding`, the
    /// stored embedding is cleared (a stale vector is worse than none).
    fn update_memory(&self, id: &str, update: &MemoryUpdate) -> Result<Memory>;
    /// `Ok(true)` when a row was deleted, `Ok(false)` when the id was unknown.
    fn delete_memory(&self, id: &str) -> Result<bool>;
    /// Newest first. `namespace` filters; `limit` must be > 0; `offset` skips
    /// that many rows for pagination (0 = from the top).
    fn list_memories(
        &self,
        namespace: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Memory>>;
    /// Total memories matching `namespace` (all stores when `None`) — the
    /// pagination denominator. Unknown namespaces error.
    fn count_memories(&self, namespace: Option<&str>) -> Result<u64>;

    /// Flush the write-ahead log into the main database file (`wal_checkpoint(TRUNCATE)`).
    /// Called on graceful shutdown and before backups.
    fn checkpoint_wal(&self) -> Result<()>;
    /// Compact the database in place, reclaiming freelist pages.
    fn vacuum(&self) -> Result<()>;
    /// Write a compacted, consistent snapshot of the whole database to
    /// `path` (`VACUUM INTO`). The target must not exist. Encrypted stores
    /// produce encrypted backups (the ciphertext is copied verbatim).
    fn vacuum_into(&self, path: &std::path::Path) -> Result<()>;

    /// Hybrid recall: FTS5 keyword candidates merged with a brute-force cosine scan,
    /// ranked by [`crate::scorer::HybridScorer`], each hit carrying its score breakdown.
    /// `query_embedding` of `None`/empty disables the vector term for this query.
    /// Recall is read-only over `last_accessed_at` (write-time only, AR-018).
    fn recall(
        &self,
        namespace: &str,
        query: &str,
        query_embedding: Option<&[f32]>,
        params: &RecallParams,
        now: i64,
    ) -> Result<Vec<RecallHit>>;

    /// Run `f` inside a single write transaction: either every change `f`
    /// makes commits, or (on error) none of it does. Multi-step writes such
    /// as import (AR-023) must use this so a failure never leaves partial
    /// state behind.
    fn write_tx(&self, f: &mut dyn FnMut(&dyn Store) -> Result<()>) -> Result<()>;

    fn stats(&self) -> Result<Stats>;
}
