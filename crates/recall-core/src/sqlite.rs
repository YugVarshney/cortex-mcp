//! SQLite-backed [`Store`] with an FTS5 keyword index (external content,
//! trigger-synced) and BLOB-stored embeddings (ADR-002).
//!
//! Encryption at rest (D-014): when opened with a [`StoreKey`], `memories.text`
//! stores only AEAD ciphertext (`enc:v1:...`) and the FTS index is built from
//! keyed HMAC token digests, so no plaintext reaches disk. The keyword pass
//! digests query tokens with the same key, keeping BM25 ranking identical.
//! Legacy plaintext databases are upgraded in place on the first keyed open.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use tracing::debug;

use crate::crypto::{StoreKey, key_check_context, text_context};
use crate::error::{RecallError, Result};
use crate::models::{
    Memory, MemoryUpdate, Namespace, NamespaceCount, NewMemory, RecallHit, RecallParams, Stats,
};
use crate::scorer::{HybridScorer, RawScore};
use crate::store::Store;
use crate::util::{blob_to_f32, f32_to_blob, tokenize};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
-- Hot-path knobs for large stores (D-016): the recall scan reads every
-- embedding BLOB in a namespace, so a bigger page cache and memory-mapped
-- I/O keep the 10k-row scan off the OS read path. Both are session-level
-- and never change query semantics.
PRAGMA cache_size = -8000;
PRAGMA mmap_size = 134217728;

CREATE TABLE IF NOT EXISTS store_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS namespaces (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
    id               TEXT PRIMARY KEY,
    namespace_id     TEXT NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
    text             TEXT NOT NULL,
    fts_tokens       TEXT NOT NULL DEFAULT '',
    tags             TEXT NOT NULL DEFAULT '[]',
    embedding        BLOB,
    source           TEXT NOT NULL DEFAULT 'agent',
    pinned           INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace_id);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    fts_tokens,
    content = 'memories',
    content_rowid = 'rowid'
);

CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, fts_tokens) VALUES (new.rowid, new.fts_tokens);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, fts_tokens)
    VALUES ('delete', old.rowid, old.fts_tokens);
END;

-- Keeps the external-content index exact for text rewrites (memory updates).
-- Access-time `last_accessed_at` updates must not churn the index, hence the
-- WHEN guard and the column restriction.
CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE OF text, fts_tokens ON memories
WHEN old.fts_tokens IS NOT new.fts_tokens
BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, fts_tokens)
    VALUES ('delete', old.rowid, old.fts_tokens);
    INSERT INTO memories_fts(rowid, fts_tokens) VALUES (new.rowid, new.fts_tokens);
END;
"#;

/// Minimal FTS5 expression builder: double-quoted OR of tokens, so arbitrary
/// user input can neither break MATCH syntax nor inject operators. When a key
/// is supplied (encrypted mode), each token is replaced by its keyed digest —
/// the same transformation applied to stored documents, so ranking is
/// unchanged while query and index stay plaintext-free.
pub fn build_fts_query(query: &str, key: Option<&StoreKey>) -> Option<String> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|t| {
                let term = match key {
                    Some(k) => k.token_digest(t),
                    None => t.replace('"', ""),
                };
                format!("\"{term}\"")
            })
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// SQL fragment enforcing the required-tag filter (AND semantics): the
/// correlated `json_each` count must equal the number of required tags.
/// `alias` is the `memories` table alias in the enclosing query; placeholders
/// are numbered from `start_index` (1-based). Empty tag list → empty fragment.
fn tag_filter_sql(alias: &str, tags: &[String], start_index: usize) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let placeholders = (start_index..start_index + tags.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        " AND (SELECT COUNT(DISTINCT je.value) FROM json_each({alias}.tags) AS je \
         WHERE je.value IN ({placeholders})) = {}",
        tags.len()
    )
}

/// One vector-pass survivor before its full row is fetched (D-016). Carries
/// exactly what candidate selection needs — rowid, similarity, and the total
/// the row scores if it stays outside the keyword set (there bm25 is 0, so
/// `HybridScorer` on these inputs IS the final total). Ids are resolved after
/// selection; `rowid` is only the last-resort tie-break, which merely orders
/// bit-identical duplicates (same total and timestamp) by insertion order.
struct VectorCandidate {
    rowid: i64,
    cosine: f64,
    vector_only_total: f64,
    created_at: i64,
}

impl PartialEq for VectorCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for VectorCandidate {}

impl Ord for VectorCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // The final ranking comparator (total desc, created_at desc),
        // inverted: the WORST candidate compares greatest, so a capped
        // max-heap evicts the weakest survivor in O(log k).
        other
            .vector_only_total
            .partial_cmp(&self.vector_only_total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| other.created_at.cmp(&self.created_at))
    }
}

impl PartialOrd for VectorCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Packed, dequantized embeddings for one namespace — the D-016 remedy for
/// the measured blob-read floor. The vector pass re-reads ~10 MB of embedding
/// BLOBs from SQLite on every recall; after the first scan the packed cache
/// serves the same rows from contiguous memory, so repeat recalls pay only
/// the (vectorized) cosine arithmetic.
///
/// All rows of the namespace (up to the fill-time `scan_cap`, in `rowid`
/// order — the same order the index scan returns) are kept, including rows
/// *without* a usable vector: they occupy a slot so `LIMIT scan_cap`
/// semantics match the SQL scan exactly. `has_vector` marks which slots hold
/// real data; everything else scores cosine 0.0, identical to how the SQL
/// path treats NULL blobs and dimension mismatches (`dim` is the *query*
/// dimension, so a mismatched stored vector is "no signal", as before).
struct PackedNamespace {
    namespace_id: String,
    /// Query-dimension the cache was packed for; rows whose stored BLOB does
    /// not have exactly this shape are treated as vector-less.
    dim: usize,
    /// Row-major packed vectors: entry `i` lives at `data[i*dim..(i+1)*dim]`.
    data: Vec<f32>,
    has_vector: Vec<bool>,
    rowids: Vec<i64>,
    created_at: Vec<i64>,
    pinned: Vec<bool>,
    /// True when the namespace has no rows beyond the ones cached (the fill
    /// query returned fewer rows than the cap), so the cache can serve any
    /// `scan_cap`. When false, a caller asking for more rows triggers a
    /// rebuild with the larger cap.
    complete: bool,
}

impl PackedNamespace {
    fn len(&self) -> usize {
        self.rowids.len()
    }

    /// Load up to `scan_cap` rows (rowid order) for `namespace_id`, packing
    /// vectors of the query's dimensionality. Reads happen once per
    /// (re)build; the per-recall scan afterwards never touches SQLite.
    fn load(conn: &Connection, namespace_id: &str, dim: usize, scan_cap: usize) -> Result<Self> {
        let sql = format!(
            "SELECT rowid, embedding, created_at, pinned FROM memories
             WHERE namespace_id = ?1 ORDER BY rowid LIMIT {scan_cap}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![namespace_id])?;
        let mut rowids = Vec::new();
        let mut created_at = Vec::new();
        let mut pinned = Vec::new();
        let mut blobs: Vec<Option<Vec<u8>>> = Vec::new();
        while let Some(row) = rows.next()? {
            rowids.push(row.get(0)?);
            created_at.push(row.get(2)?);
            pinned.push(row.get::<_, i64>(3)? != 0);
            // The only allocation on the fill path; the scan path never runs
            // this query while the cache stays valid.
            blobs.push(row.get(1)?);
        }
        let mut data = vec![0.0f32; rowids.len() * dim];
        let mut has_vector = vec![false; rowids.len()];
        for (i, blob) in blobs.iter().enumerate() {
            let Some(bytes) = blob else { continue };
            if bytes.len() != dim * 4 {
                continue;
            }
            for (j, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
                data[i * dim + j] = f32::from_le_bytes(*chunk);
            }
            has_vector[i] = true;
        }
        Ok(Self {
            namespace_id: namespace_id.to_string(),
            dim,
            data,
            has_vector,
            rowids,
            created_at,
            pinned,
            complete: blobs.len() < scan_cap,
        })
    }

    /// Borrow entry `i` as an `f32` slice (only valid when `has_vector[i]`).
    fn slice(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }
}

/// Reuse the cached packed vectors for `namespace_id`, or rebuild the cache
/// in place. Rebuild happens when the namespace changed, the query
/// dimensionality changed, or the caller asks for more rows than cached from
/// an incomplete fill (complete caches serve any cap; a *smaller* cap reuses
/// the cache as-is and the recall loop bounds the scan to the caller's cap,
/// preserving the SQL `LIMIT` semantics row-for-row).
#[allow(clippy::expect_used)] // invariant: the cache was just filled or reused above
fn packed_vectors_for<'a>(
    slot: &'a mut Option<PackedNamespace>,
    conn: &Connection,
    namespace_id: &str,
    dim: usize,
    scan_cap: usize,
) -> Result<&'a PackedNamespace> {
    let usable = match &*slot {
        Some(cached) => {
            cached.namespace_id == namespace_id
                && cached.dim == dim
                && (cached.complete || cached.len() >= scan_cap)
        }
        None => false,
    };
    if !usable {
        *slot = Some(PackedNamespace::load(conn, namespace_id, dim, scan_cap)?);
    }
    Ok(slot.as_ref().expect("cache was just filled or reused"))
}

pub struct SqliteStore {
    conn: Connection,
    key: Option<StoreKey>,
    /// Packed vector cache for the most recently scanned namespace (D-019).
    /// `RefCell` is sound here by the same argument as D-007: the store is
    /// `Send`-only and every access is serialized by the owning mutex, so no
    /// reference into the cell can ever be held across a re-entrant borrow
    /// (writes invalidate *after* their SQL runs, never mid-scan).
    vector_cache: std::cell::RefCell<Option<PackedNamespace>>,
}

/// Metadata keys in `store_meta`.
const META_AT_REST: &str = "at_rest";
const META_KEY_CHECK: &str = "key_check";
const AT_REST_PLAIN: &str = "plain";
const AT_REST_ENC: &str = "enc_v1";
/// Known plaintext encrypted as the key check: a wrong key fails at open.
const KEY_CHECK_PLAINTEXT: &str = "recall-mcp key check v1";

/// Raw column view of a `memories` row, before tag parsing and decryption.
struct MemoryRow {
    id: String,
    namespace_id: String,
    text_stored: String,
    tags_json: String,
    embedding: Option<Vec<u8>>,
    source: String,
    pinned: bool,
    created_at: i64,
    last_accessed_at: i64,
}

const MEMORY_COLUMNS: &str =
    "id, namespace_id, text, tags, embedding, source, pinned, created_at, last_accessed_at";

fn memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
    Ok(MemoryRow {
        id: row.get("id")?,
        namespace_id: row.get("namespace_id")?,
        text_stored: row.get("text")?,
        tags_json: row.get("tags")?,
        embedding: row.get("embedding")?,
        source: row.get("source")?,
        pinned: row.get::<_, i64>("pinned")? != 0,
        created_at: row.get("created_at")?,
        last_accessed_at: row.get("last_accessed_at")?,
    })
}

impl MemoryRow {
    /// Parse tags and decrypt text (keyed store) into a domain [`Memory`].
    fn into_memory(self, key: Option<&StoreKey>) -> Result<Memory> {
        let tags = serde_json::from_str(&self.tags_json).map_err(RecallError::Serialization)?;
        let text = match (key, StoreKey::is_encrypted(&self.text_stored)) {
            (Some(k), true) => k.decrypt(&text_context(&self.id), &self.text_stored)?,
            // Legacy/mixed rows written before the key existed read as-is.
            (_, false) => self.text_stored,
            (None, true) => {
                return Err(RecallError::Crypto(
                    "encrypted memory value present but no key was provided".into(),
                ));
            }
        };
        Ok(Memory {
            id: self.id,
            namespace_id: self.namespace_id,
            text,
            tags,
            embedding: self.embedding.as_deref().and_then(blob_to_f32),
            source: self.source,
            pinned: self.pinned,
            created_at: self.created_at,
            last_accessed_at: self.last_accessed_at,
        })
    }
}

impl SqliteStore {
    /// Open (creating if needed) a plaintext database file. `:memory:` is
    /// honored. Refuses databases that are encrypted at rest — open with
    /// [`SqliteStore::open_with_key`] instead.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_inner(path, None)
    }

    /// Open (creating if needed) with encryption at rest. New memories are
    /// stored encrypted; an existing plaintext database is upgraded in place
    /// on first open (D-014). The key is verified against a stored check
    /// value, so a wrong key fails immediately.
    pub fn open_with_key<P: AsRef<Path>>(path: P, key: &StoreKey) -> Result<Self> {
        Self::open_inner(path, Some(key.clone()))
    }

    /// In-memory database, mainly for tests and ephemeral tools (plaintext).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            key: None,
            vector_cache: std::cell::RefCell::new(None),
        })
    }

    /// In-memory encrypted database (tests).
    pub fn open_in_memory_with_key(key: &StoreKey) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let store = Self {
            conn,
            key: Some(key.clone()),
            vector_cache: std::cell::RefCell::new(None),
        };
        store.init_mode()?;
        Ok(store)
    }

    fn open_inner<P: AsRef<Path>>(path: P, key: Option<StoreKey>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let needs_upgrade = legacy_schema(&conn)?;
        if needs_upgrade {
            upgrade_legacy_schema(&conn)?;
        }
        let store = Self {
            conn,
            key,
            vector_cache: std::cell::RefCell::new(None),
        };
        // Forensic hygiene for the in-place plaintext→keyed upgrade (AR-013):
        // while plaintext rows are rewritten, freed pages must be zero-filled
        // rather than left with recoverable plaintext. Scoped to the upgrade
        // window — steady-state keyed rows are ciphertext, so deletes have
        // nothing sensitive to shred.
        let keyed_conn = store.key.is_some();
        if keyed_conn {
            store.conn.execute_batch("PRAGMA secure_delete = ON")?;
        }
        // Legacy layout always stored plaintext.
        if needs_upgrade {
            store.copy_plaintext_fts_tokens()?;
        }
        store.init_mode()?;
        if store.key.is_some() && needs_upgrade {
            // The plaintext era wrote legacy rows and (pre-upgrade) FTS
            // content into pages that may linger after deletion. VACUUM alone
            // is not enough: freed-page content persists without
            // secure_delete (set above), and WAL frames from the plaintext
            // era survive in the sidecar unless checkpointed with TRUNCATE —
            // so the upgrade finishes with both (D-014 keeps the caveat that
            // filesystem copies made before the upgrade remain).
            store.conn.execute_batch("VACUUM")?;
            store.checkpoint_wal()?;
        }
        if keyed_conn {
            store.conn.execute_batch("PRAGMA secure_delete = OFF")?;
        }
        Ok(store)
    }

    /// Enforce mode consistency and complete the keyed-mode setup:
    /// verify the stored key check, encrypt any remaining plaintext rows,
    /// and persist the mode marker.
    #[allow(clippy::expect_used)] // `self.key` is Some on the arm that dereferences it
    fn init_mode(&self) -> Result<()> {
        let marker = self.meta_get(META_AT_REST)?;
        match (&self.key, marker.as_deref()) {
            (None, Some(AT_REST_ENC)) => {
                return Err(RecallError::Crypto(
                    "database is encrypted at rest: set RECALL_MCP_KEY (64 hex chars) \
                     or RECALL_MCP_KEY_FILE to open it"
                        .into(),
                ));
            }
            (Some(_), Some(AT_REST_ENC)) => {
                let key = self.key.as_ref().expect("checked above");
                if !self.key_check_matches(key)? {
                    // Stores written before key separation (AR-002) sealed the
                    // key check with the raw master key as AEAD key. If that
                    // legacy layout opens the database, migrate it to the
                    // derived subkeys; anything else is a wrong key.
                    let legacy = StoreKey::legacy_from_bytes(key.master_bytes());
                    if !self.key_check_matches(&legacy)? {
                        return Err(RecallError::Crypto(
                            "decryption failed: wrong key or corrupted/tampered data".into(),
                        ));
                    }
                    self.rekey_from_legacy(&legacy)?;
                }
            }
            // First keyed open (fresh or legacy/marked-plain): encrypt everything.
            (Some(_), _) => {
                self.encrypt_remaining_plaintext()?;
                self.meta_set(META_AT_REST, AT_REST_ENC)?;
                self.meta_set(
                    META_KEY_CHECK,
                    &self
                        .key
                        .as_ref()
                        .expect("checked above")
                        .encrypt(&key_check_context(), KEY_CHECK_PLAINTEXT),
                )?;
            }
            (None, _) => {
                if marker.is_none() {
                    self.meta_set(META_AT_REST, AT_REST_PLAIN)?;
                }
            }
        }
        Ok(())
    }

    /// Open-time proof that the supplied key is the one the database was
    /// written with (AEAD decrypt fails on any other key).
    fn key_check_matches(&self, key: &StoreKey) -> Result<bool> {
        match self.meta_get(META_KEY_CHECK)? {
            Some(check) => Ok(key.decrypt(&key_check_context(), &check).is_ok()),
            // No check stored yet (fresh keyed open mid-upgrade): nothing to
            // verify against.
            None => Ok(true),
        }
    }

    /// One-time migration for stores written before key separation (AR-002),
    /// where the raw master key served as both AEAD and HMAC key. Decrypts
    /// every row with the legacy key, re-seals it under the derived subkeys,
    /// recomputes the FTS token digests, and re-wraps the key-check value —
    /// all in one transaction so a failure leaves the store exactly as it
    /// was. A VACUUM + WAL checkpoint afterwards shreds the legacy pages.
    #[allow(clippy::expect_used)] // only called with self.key = Some from init_mode
    fn rekey_from_legacy(&self, legacy: &StoreKey) -> Result<()> {
        let key = self.key.as_ref().expect("keyed open");
        let rows: Vec<(String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, text FROM memories WHERE text LIKE 'enc:v1:%'")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.filter_map(|r| {
                let (id, text) = r.ok()?;
                // Prefix-bearing rows that fail the shape check are plaintext
                // literals and stay untouched (same rule as the upgrade pass).
                StoreKey::is_encrypted(&text).then_some((id, text))
            })
            .collect()
        };
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<()> {
            let mut stmt = self
                .conn
                .prepare_cached("UPDATE memories SET text = ?1, fts_tokens = ?2 WHERE id = ?3")?;
            for (id, stored) in &rows {
                let plaintext = legacy.decrypt(&text_context(id), stored)?;
                stmt.execute(params![
                    key.encrypt(&text_context(id), &plaintext),
                    key.fts_tokens(&plaintext),
                    id
                ])?;
            }
            self.meta_set(
                META_KEY_CHECK,
                &key.encrypt(&key_check_context(), KEY_CHECK_PLAINTEXT),
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        self.conn.execute_batch("VACUUM")?;
        self.checkpoint_wal()?;
        debug!(
            count = rows.len(),
            "migrated store to separated key derivation"
        );
        Ok(())
    }

    /// Encrypt every remaining plaintext row in one transaction (idempotent:
    /// already-encrypted rows are skipped). The FTS update trigger keeps the
    /// index exact as each row's token digest replaces its plaintext copy.
    fn encrypt_remaining_plaintext(&self) -> Result<()> {
        let key = match &self.key {
            Some(k) => k,
            None => return Ok(()),
        };
        let mut ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM memories WHERE text NOT LIKE 'enc:v1:%'")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        // Rows wearing the prefix but not its ciphertext shape are plaintext
        // literals ("enc:v1:this is prose, not base64!") written before a key
        // existed; the prefix test alone would skip them and a later read
        // would fail decrypting. The shape-checking [`StoreKey::is_encrypted`]
        // separates them from real ciphertext.
        let literal_ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, text FROM memories WHERE text LIKE 'enc:v1:%'")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.filter_map(|r| {
                let (id, text) = r.ok()?;
                (!StoreKey::is_encrypted(&text)).then_some(id)
            })
            .collect()
        };
        ids.extend(literal_ids);
        if ids.is_empty() {
            return Ok(());
        }
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<()> {
            let mut stmt = self
                .conn
                .prepare_cached("UPDATE memories SET text = ?1, fts_tokens = ?2 WHERE id = ?3")?;
            for id in &ids {
                let memory = self
                    .get_memory(id)?
                    .ok_or_else(|| RecallError::MemoryNotFound(id.clone()))?;
                let encrypted = key.encrypt(&text_context(id), &memory.text);
                let tokens = key.fts_tokens(&memory.text);
                stmt.execute(params![encrypted, tokens, id])?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        debug!(count = ids.len(), "encrypted existing plaintext memories");
        Ok(())
    }

    /// Legacy (pre-encryption-schema) databases indexed `memories.text`
    /// directly. Copy it into the new `fts_tokens` column so the rebuilt
    /// index matches; the keyed-mode pass replaces it with digests next.
    /// The copy must run with triggers dropped: firing the update trigger
    /// with values that were never indexed corrupts an external-content
    /// FTS5 index.
    fn copy_plaintext_fts_tokens(&self) -> Result<()> {
        self.conn.execute_batch(
            "DROP TRIGGER IF EXISTS memories_fts_insert;
             DROP TRIGGER IF EXISTS memories_fts_delete;
             DROP TRIGGER IF EXISTS memories_fts_update;
             UPDATE memories SET fts_tokens = text;",
        )?;
        self.conn.execute_batch(SCHEMA)?;
        self.conn
            .execute_batch("INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');")?;
        Ok(())
    }

    fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT value FROM store_meta WHERE key = ?1")?;
        Ok(stmt
            .query_row(params![key], |r| r.get::<_, String>(0))
            .optional()?)
    }

    fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO store_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Storage form of memory text: ciphertext in keyed mode, plaintext
    /// otherwise; plus the FTS document (keyed digests or the text itself).
    fn stored_text(&self, id: &str, text: &str) -> (String, String) {
        match &self.key {
            Some(k) => (k.encrypt(&text_context(id), text), k.fts_tokens(text)),
            None => (text.to_string(), text.to_string()),
        }
    }

    fn namespace_id(&self, name: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM namespaces WHERE name = ?1")?;
        Ok(stmt
            .query_row(params![name], |r| r.get::<_, String>(0))
            .optional()?)
    }

    /// Drop the packed vector cache after any write to `memories` (insert,
    /// update, delete). Rebuilds are rare next to reads in this workload, so
    /// invalidation is total rather than surgical — correctness over
    /// cleverness.
    fn invalidate_vector_cache(&self) {
        self.vector_cache.borrow_mut().take();
    }
}

/// True when `memories` predates the encryption schema (no `fts_tokens`
/// column, FTS index over `text`).
fn legacy_schema(conn: &Connection) -> Result<bool> {
    let has_column: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'fts_tokens'",
        [],
        |r| r.get(0),
    )?;
    Ok(has_column == 0 && table_exists(conn, "memories")?)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// One-time migration of a pre-encryption database to schema v2:
/// add `fts_tokens`, recreate the FTS virtual table over it, and drop the
/// old text-based triggers (fresh ones are created by `SCHEMA` on next run /
/// below). Content is rebuilt by [`SqliteStore::copy_plaintext_fts_tokens`].
fn upgrade_legacy_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"BEGIN;
           DROP TRIGGER IF EXISTS memories_fts_insert;
           DROP TRIGGER IF EXISTS memories_fts_delete;
           DROP TRIGGER IF EXISTS memories_fts_update;
           ALTER TABLE memories ADD COLUMN fts_tokens TEXT NOT NULL DEFAULT '';
           DROP TABLE memories_fts;
           CREATE VIRTUAL TABLE memories_fts USING fts5(
               fts_tokens,
               content = 'memories',
               content_rowid = 'rowid'
           );
           COMMIT;"#,
    )?;
    conn.execute_batch(SCHEMA)?;
    debug!("upgraded legacy schema to encryption-capable layout");
    Ok(())
}

impl Store for SqliteStore {
    fn create_namespace(&self, name: &str) -> Result<Namespace> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RecallError::InvalidInput(
                "namespace name must not be empty".into(),
            ));
        }
        if let Some(existing) = self.find_namespace(name)? {
            return Err(RecallError::DuplicateNamespace(existing.name));
        }
        let id = new_id();
        let now = crate::unix_now();
        self.conn.execute(
            "INSERT INTO namespaces (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, now],
        )?;
        debug!(namespace = name, "created namespace");
        Ok(Namespace {
            id,
            name: name.to_string(),
            created_at: now,
        })
    }

    fn get_or_create_namespace(&self, name: &str) -> Result<Namespace> {
        if let Some(existing) = self.find_namespace(name)? {
            return Ok(existing);
        }
        self.create_namespace(name)
    }

    fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM namespaces ORDER BY created_at, name")?;
        let rows = stmt.query_map([], |r| {
            Ok(Namespace {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn find_namespace(&self, name: &str) -> Result<Option<Namespace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM namespaces WHERE name = ?1")?;
        Ok(stmt
            .query_row(params![name], |r| {
                Ok(Namespace {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })
            .optional()?)
    }

    fn insert_memory(&self, memory: &NewMemory) -> Result<Memory> {
        let text = memory.text.trim();
        if text.is_empty() {
            return Err(RecallError::InvalidInput(
                "memory text must not be empty".into(),
            ));
        }
        let ns = self
            .namespace_id(&memory.namespace)?
            .ok_or_else(|| RecallError::NamespaceNotFound(memory.namespace.clone()))?;

        let id = match &memory.id {
            Some(id) if id.trim().is_empty() => {
                return Err(RecallError::InvalidInput(
                    "memory id must not be blank".into(),
                ));
            }
            Some(id) => id.clone(),
            None => new_id(),
        };
        let now = crate::unix_now();
        let created_at = memory.created_at.unwrap_or(now);
        let tags = serde_json::to_string(&memory.tags)?;
        let embedding_blob = memory.embedding.as_deref().map(f32_to_blob);
        let source = memory.source.clone().unwrap_or_else(|| "agent".to_string());
        let (stored_text, fts_tokens) = self.stored_text(&id, text);

        // Distinguish "duplicate id" from other constraint failures for a precise error.
        let inserted = self.conn.execute(
            "INSERT INTO memories (id, namespace_id, text, fts_tokens, tags, embedding, source, pinned, created_at, last_accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                id,
                ns,
                stored_text,
                fts_tokens,
                tags,
                embedding_blob,
                source,
                i64::from(memory.pinned),
                created_at
            ],
        );
        match inserted {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(RecallError::InvalidInput(format!(
                    "memory id already exists: {id}"
                )));
            }
            Err(e) => return Err(e.into()),
        }
        debug!(memory_id = %id, namespace = %memory.namespace, "inserted memory");
        self.invalidate_vector_cache();
        self.get_memory(&id)?
            .ok_or_else(|| RecallError::InvalidInput("inserted memory not found".into()))
    }

    fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MEMORY_COLUMNS} FROM memories WHERE id = ?1"
        ))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(memory_row(row)?.into_memory(self.key.as_ref())?)),
            None => Ok(None),
        }
    }

    fn update_memory(&self, id: &str, update: &MemoryUpdate) -> Result<Memory> {
        if update.is_empty() {
            return Err(RecallError::InvalidInput(
                "update must set at least one field".into(),
            ));
        }
        if update.text.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return Err(RecallError::InvalidInput(
                "memory text must not be empty".into(),
            ));
        }
        let existing = self
            .get_memory(id)?
            .ok_or_else(|| RecallError::MemoryNotFound(id.to_string()))?;

        let new_text = match &update.text {
            Some(t) => t.trim().to_string(),
            None => existing.text.clone(),
        };
        let new_tags = update.tags.clone().unwrap_or_else(|| existing.tags.clone());
        let new_source = update
            .source
            .clone()
            .unwrap_or_else(|| existing.source.clone());
        let new_pinned = update.pinned.unwrap_or(existing.pinned);
        // A changed text invalidates the stored vector unless a fresh one is
        // supplied (callers with an embedder re-embed before updating).
        let new_embedding = match (&update.embedding, &update.text) {
            (Some(e), _) => Some(e.clone()),
            (None, Some(_)) => None,
            (None, None) => existing.embedding.clone(),
        };
        let tags_json = serde_json::to_string(&new_tags)?;
        let (stored_text, fts_tokens) = self.stored_text(id, &new_text);
        let embedding_blob = new_embedding.as_deref().map(f32_to_blob);
        let changed = self.conn.execute(
            "UPDATE memories SET text = ?1, fts_tokens = ?2, tags = ?3, source = ?4,
             pinned = ?5, embedding = ?6 WHERE id = ?7",
            params![
                stored_text,
                fts_tokens,
                tags_json,
                new_source,
                i64::from(new_pinned),
                embedding_blob,
                id
            ],
        )?;
        if changed == 0 {
            return Err(RecallError::MemoryNotFound(id.to_string()));
        }
        debug!(memory_id = id, "updated memory");
        self.invalidate_vector_cache();
        self.get_memory(id)?
            .ok_or_else(|| RecallError::MemoryNotFound(id.to_string()))
    }

    fn delete_memory(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        if n > 0 {
            self.invalidate_vector_cache();
        }
        Ok(n > 0)
    }

    fn list_memories(
        &self,
        namespace: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Memory>> {
        if limit == 0 {
            return Err(RecallError::InvalidInput("limit must be > 0".into()));
        }
        let sql = match namespace {
            Some(_) => format!(
                "SELECT {MEMORY_COLUMNS} FROM memories WHERE namespace_id = ?1
                 ORDER BY created_at DESC, id ASC LIMIT ?2 OFFSET ?3"
            ),
            None => format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 ORDER BY created_at DESC, id ASC LIMIT ?1 OFFSET ?2"
            ),
        };
        let ns = match namespace {
            Some(name) => Some(
                self.namespace_id(name)?
                    .ok_or_else(|| RecallError::NamespaceNotFound(name.to_string()))?,
            ),
            None => None,
        };
        let mut stmt = self.conn.prepare(&sql)?;
        // `usize as i64` would wrap values above i64::MAX negative, and SQLite
        // treats a negative LIMIT as unlimited — the exact hazard the REST/MCP
        // MAX_PAGE_LIMIT clamp guards against. Saturate here too so the store
        // itself can never produce a negative LIMIT bind (AR-004).
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        let rows = match ns {
            Some(ns) => stmt.query_map(params![ns, limit, offset], memory_row)?,
            None => stmt.query_map(params![limit, offset], memory_row)?,
        };
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_memory(self.key.as_ref())?);
        }
        Ok(out)
    }

    fn count_memories(&self, namespace: Option<&str>) -> Result<u64> {
        let count = match namespace {
            Some(name) => {
                let ns = self
                    .namespace_id(name)?
                    .ok_or_else(|| RecallError::NamespaceNotFound(name.to_string()))?;
                self.conn.query_row(
                    "SELECT COUNT(*) FROM memories WHERE namespace_id = ?1",
                    params![ns],
                    |r| r.get::<_, i64>(0),
                )?
            }
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0))?,
        };
        Ok(count as u64)
    }

    #[allow(clippy::expect_used)] // the candidate-heap peek is infallible: validate() guarantees k >= 1
    fn recall(
        &self,
        namespace: &str,
        query: &str,
        query_embedding: Option<&[f32]>,
        p: &RecallParams,
        now: i64,
    ) -> Result<Vec<RecallHit>> {
        p.validate()?;
        if query.trim().is_empty() {
            return Err(RecallError::InvalidInput("query must not be empty".into()));
        }
        let ns = self
            .namespace_id(namespace)?
            .ok_or_else(|| RecallError::NamespaceNotFound(namespace.to_string()))?;

        // Tag filter (D-015): ALL required tags must be present (exact,
        // case-sensitive match). Tags are stored as JSON arrays, so the
        // check is a correlated `json_each` count per row.
        let mut required_tags = p.tags.clone();
        required_tags.sort();
        required_tags.dedup();

        // Pass 1 — keyword candidates from FTS5 (bm25, more negative = better).
        // Keyed by rowid; ids are resolved when the surviving rows are fetched.
        let mut keyword: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
        if let Some(fts_query) = build_fts_query(query, self.key.as_ref()) {
            let tag_sql = tag_filter_sql("m", &required_tags, 3);
            let sql = format!(
                "SELECT m.rowid, bm25(memories_fts) AS rank_score
                 FROM memories_fts
                 JOIN memories m ON m.rowid = memories_fts.rowid
                 WHERE memories_fts MATCH ?1 AND m.namespace_id = ?2{tag_sql}
                 ORDER BY rank_score
                 LIMIT {}",
                p.candidate_cap
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut bind: Vec<String> = vec![fts_query, ns.clone()];
            bind.extend(required_tags.iter().cloned());
            let rows = stmt.query_map(rusqlite::params_from_iter(bind.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (rowid, rank) = row?;
                keyword.insert(rowid, -rank);
            }
        }
        let max_bm25 = keyword.values().copied().fold(0.0f64, f64::max);

        // Pass 2 — brute-force cosine over the namespace (exact, explainable;
        // ADR-002 as amended by D-016 + D-019). Rows are scanned slim (rowid +
        // embedding + created_at + pinned) and the query norm is hoisted out
        // of the loop. Without a tag filter the scan reads the packed vector
        // cache (contiguous memory, no per-recall SQLite BLOB reads); with a
        // tag filter it reads the same columns straight from SQLite, because
        // the cache does not carry tags. Text and tags are fetched
        // afterwards, for the surviving candidates only.
        //
        // Selection stays exact: outside the keyword set bm25 is 0, so a
        // row's vector-only total IS its final total; rows beyond the top-`k`
        // by that measure sort strictly below the heap's weakest member under
        // the final ranking comparator and can never enter the top-`k`.
        let query_vec: Option<&[f32]> = query_embedding.filter(|v| !v.is_empty());
        let scorer = HybridScorer {
            weights: p.weights,
            tau_days: p.tau_days,
            pinned_boost: p.pinned_boost,
        };
        let mut vector: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
        let mut best_vector_only: std::collections::BinaryHeap<VectorCandidate> =
            std::collections::BinaryHeap::with_capacity(p.k);
        if let Some(qv) = query_vec {
            let query_norm_sqrt = crate::util::f32_norm_sqrt(qv);
            // Per-row selection, shared by both scan paths: the heap
            // exactness argument applies identically to cached and SQL-sourced
            // rows.
            let mut consider = |rowid: i64, sim: f64, created_at: i64, pinned: bool| {
                if sim == 0.0 {
                    return;
                }
                if keyword.contains_key(&rowid) {
                    // Keyword rows are candidates regardless of vector rank.
                    vector.insert(rowid, sim);
                    return;
                }
                let vector_only_total = scorer
                    .score(
                        crate::scorer::RawScore {
                            bm25_normalized: 0.0,
                            cosine: sim,
                            created_at,
                            pinned,
                        },
                        now,
                    )
                    .total;
                let candidate = VectorCandidate {
                    rowid,
                    cosine: sim,
                    vector_only_total,
                    created_at,
                };
                if best_vector_only.len() < p.k {
                    best_vector_only.push(candidate);
                } else if *best_vector_only
                    .peek()
                    .expect("k >= 1 keeps the heap non-empty")
                    > candidate
                {
                    best_vector_only.pop();
                    best_vector_only.push(candidate);
                }
            };
            if required_tags.is_empty() {
                // Packed-cache path (D-019): same rows (rowid order, same
                // `scan_cap` limit), same arithmetic (`cosine_f32_with_query_norm`
                // is bit-identical to the BLOB variant), so results are
                // unchanged — only the blob-read round trips disappear.
                let mut cache_slot = self.vector_cache.borrow_mut();
                let packed =
                    packed_vectors_for(&mut cache_slot, &self.conn, &ns, qv.len(), p.scan_cap)?;
                // Bound the scan to the caller's cap: a smaller `scan_cap`
                // after a larger incomplete fill must not surface rows the
                // SQL `LIMIT` would never have returned. Rows are stored in
                // rowid order, so the first `scan_cap` entries are exactly
                // the rows the index scan returns.
                let rows = packed.len().min(p.scan_cap);
                for i in 0..rows {
                    let sim = if packed.has_vector[i] {
                        crate::util::cosine_f32_with_query_norm(
                            qv,
                            query_norm_sqrt,
                            packed.slice(i),
                        )
                    } else {
                        0.0
                    };
                    consider(
                        packed.rowids[i],
                        sim,
                        packed.created_at[i],
                        packed.pinned[i],
                    );
                }
            } else {
                // Tag-filtered path: identical to the pre-cache SQL scan; the
                // correlated `json_each` filter runs inside SQLite.
                let tag_sql = tag_filter_sql("memories", &required_tags, 2);
                let sql = format!(
                    "SELECT rowid, embedding, created_at, pinned FROM memories
                     WHERE namespace_id = ?1{tag_sql}
                     LIMIT {}",
                    p.scan_cap
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let mut bind: Vec<String> = vec![ns.clone()];
                bind.extend(required_tags.iter().cloned());
                let mut rows = stmt.query(rusqlite::params_from_iter(bind.iter()))?;
                while let Some(row) = rows.next()? {
                    let rowid: i64 = row.get(0)?;
                    let created_at: i64 = row.get(2)?;
                    let pinned = row.get::<_, i64>(3)? != 0;
                    // Borrowed blob: no per-row Vec allocation on the scan path.
                    let embedding = row.get_ref(1)?;
                    let sim = match &embedding {
                        rusqlite::types::ValueRef::Blob(bytes) => {
                            crate::util::cosine_f32_blob_with_query_norm(qv, query_norm_sqrt, bytes)
                        }
                        _ => continue,
                    };
                    consider(rowid, sim, created_at, pinned);
                }
            }
        }
        for candidate in best_vector_only {
            vector.insert(candidate.rowid, candidate.cosine);
        }

        // Full rows (text, tags, source, …) for the surviving candidates only,
        // resolved back from rowids to domain ids.
        let mut rows_by_id: std::collections::HashMap<String, Memory> =
            std::collections::HashMap::new();
        let mut candidate_rowids: std::collections::HashSet<&i64> = keyword.keys().collect();
        candidate_rowids.extend(vector.keys());
        let fetch_ids: Vec<i64> = candidate_rowids.iter().map(|rid| **rid).collect();
        let mut rowid_to_id: std::collections::HashMap<i64, String> =
            std::collections::HashMap::with_capacity(fetch_ids.len());
        // Chunked `IN` fetch keeps the placeholder count portable regardless
        // of `candidate_cap` (SQLite's host-parameter limit varies by build).
        for chunk in fetch_ids.chunks(500) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT rowid, {MEMORY_COLUMNS} FROM memories WHERE rowid IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                let memory = memory_row(row)?.into_memory(self.key.as_ref())?;
                rowid_to_id.insert(rowid, memory.id.clone());
                rows_by_id.insert(memory.id.clone(), memory);
            }
        }

        // Resolve the pass maps from rowids to domain ids (rows deleted
        // between passes were never fetched and drop out here).
        let keyword_by_id: std::collections::HashMap<&str, f64> = keyword
            .iter()
            .filter_map(|(rowid, rank)| rowid_to_id.get(rowid).map(|id| (id.as_str(), *rank)))
            .collect();
        let vector_by_id: std::collections::HashMap<&str, f64> = vector
            .iter()
            .filter_map(|(rowid, sim)| rowid_to_id.get(rowid).map(|id| (id.as_str(), *sim)))
            .collect();

        // Merge candidate sets and score. Every fetched row is a candidate.
        let mut hits: Vec<RecallHit> = Vec::new();
        for (id, memory) in &rows_by_id {
            let raw_bm25 = keyword_by_id.get(id.as_str()).copied().unwrap_or(0.0);
            let bm25_normalized = if max_bm25 > 0.0 {
                raw_bm25 / max_bm25
            } else {
                0.0
            };
            let cosine = vector_by_id.get(id.as_str()).copied().unwrap_or(0.0);
            let breakdown = scorer.score(
                RawScore {
                    bm25_normalized,
                    cosine,
                    created_at: memory.created_at,
                    pinned: memory.pinned,
                },
                now,
            );
            hits.push(RecallHit {
                memory: memory.clone(),
                breakdown,
            });
        }

        hits.sort_by(|a, b| {
            b.breakdown
                .total
                .partial_cmp(&a.breakdown.total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.memory.created_at.cmp(&a.memory.created_at))
                .then(a.memory.id.cmp(&b.memory.id))
        });
        hits.truncate(p.k);

        // `last_accessed_at` is maintained at write time only (AR-018): every
        // consumer of the column was removed with v0.2's eviction plans, so
        // recalls no longer pay a batched UPDATE (write amplification + WAL
        // churn in keyed mode) to keep an unread value fresh. The column
        // stays as the last-write timestamp, reserved for a future
        // eviction/recency-of-use policy that actually reads it.
        debug!(namespace, hits = hits.len(), "recall complete");
        Ok(hits)
    }

    fn checkpoint_wal(&self) -> Result<()> {
        // TRUNCATE also shrinks the WAL file back to zero bytes.
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    fn vacuum_into(&self, path: &std::path::Path) -> Result<()> {
        let target = path.as_os_str().to_string_lossy().to_string();
        self.conn.execute("VACUUM INTO ?1", params![target])?;
        Ok(())
    }

    fn write_tx(&self, f: &mut dyn FnMut(&dyn Store) -> Result<()>) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(())
    }

    fn stats(&self) -> Result<Stats> {
        let total_namespaces = self
            .conn
            .query_row("SELECT COUNT(*) FROM namespaces", [], |r| {
                r.get::<_, i64>(0)
            })?;
        let total_memories = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0))?
            as u64;
        let pinned =
            self.conn
                .query_row("SELECT COUNT(*) FROM memories WHERE pinned = 1", [], |r| {
                    r.get::<_, i64>(0)
                })?;
        let mut stmt = self.conn.prepare(
            "SELECT n.name, COUNT(m.id) AS c
             FROM namespaces n LEFT JOIN memories m ON m.namespace_id = n.id
             GROUP BY n.id ORDER BY c DESC, n.name",
        )?;
        let per = stmt
            .query_map([], |r| {
                Ok(NamespaceCount {
                    namespace: r.get(0)?,
                    count: r.get::<_, i64>(1)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Stats {
            total_namespaces: total_namespaces as u64,
            total_memories,
            pinned_memories: pinned as u64,
            per_namespace: per,
        })
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ENC_PREFIX;
    use crate::embed::Embedder as _;
    use crate::embed::HashEmbedder;
    use crate::models::Weights;

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("in-memory db")
    }

    fn keyed() -> SqliteStore {
        SqliteStore::open_in_memory_with_key(&test_key()).expect("in-memory keyed db")
    }

    fn test_key() -> StoreKey {
        StoreKey::from_hex("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")
            .expect("valid hex key")
    }

    fn ns(s: &SqliteStore, name: &str) -> Namespace {
        s.create_namespace(name).expect("namespace")
    }

    fn mem(namespace: &str, text: &str) -> NewMemory {
        NewMemory {
            namespace: namespace.into(),
            text: text.into(),
            tags: vec![],
            source: None,
            pinned: false,
            created_at: None,
            id: None,
            embedding: None,
        }
    }

    #[test]
    fn fts_query_builder_quotes_and_ors() {
        assert_eq!(
            build_fts_query("Deploy on Friday", None),
            Some("\"deploy\" OR \"on\" OR \"friday\"".to_string())
        );
        assert_eq!(build_fts_query("  ", None), None);
        assert_eq!(build_fts_query("", None), None);
        // Quotes inside tokens are stripped, so MATCH syntax can't be broken.
        assert_eq!(
            build_fts_query("a\"b OR c", None),
            Some("\"a\" OR \"b\" OR \"or\" OR \"c\"".to_string())
        );
        assert_eq!(build_fts_query("\"\"", None), None);
    }

    #[test]
    fn fts_query_builder_digests_tokens_in_keyed_mode() {
        let k = StoreKey::from_hex(&"ab".repeat(32)).unwrap();
        let plain = build_fts_query("deploy payments", None).unwrap();
        let keyed = build_fts_query("deploy payments", Some(&k)).unwrap();
        assert_eq!(
            keyed,
            format!(
                "\"{}\" OR \"{}\"",
                k.token_digest("deploy"),
                k.token_digest("payments")
            )
        );
        assert_ne!(plain, keyed);
        assert!(
            !keyed.contains("deploy"),
            "digest mode must not leak tokens"
        );
        assert_eq!(
            build_fts_query("deploy", Some(&k)),
            build_fts_query("deploy", Some(&k)),
            "digests are deterministic"
        );
    }

    #[test]
    fn namespace_crud_and_errors() {
        let s = store();
        assert!(s.list_namespaces().unwrap().is_empty());
        let w = ns(&s, "work");
        assert_eq!(w.name, "work");
        let dup = s.create_namespace("work").unwrap_err();
        assert!(matches!(dup, RecallError::DuplicateNamespace(_)));
        assert!(
            s.create_namespace("  ")
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
        let listed = s.list_namespaces().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, w.id);
        assert_eq!(s.find_namespace("work").unwrap().unwrap().id, w.id);
        assert!(s.find_namespace("nope").unwrap().is_none());
        // get_or_create returns existing without error.
        assert_eq!(s.get_or_create_namespace("work").unwrap().id, w.id);
        assert_eq!(s.get_or_create_namespace("fresh").unwrap().name, "fresh");
    }

    #[test]
    fn memory_crud_roundtrip_with_embedding_and_tags() {
        let s = store();
        ns(&s, "w");
        let embedding = vec![0.5f32; 8];
        let m = s
            .insert_memory(&NewMemory {
                tags: vec!["rust".into(), "db".into()],
                source: Some("human".into()),
                pinned: true,
                created_at: Some(1_700_000_000),
                embedding: Some(embedding.clone()),
                ..mem("w", "  SQLite stores embeddings as blobs.  ")
            })
            .unwrap();
        assert_eq!(m.text, "SQLite stores embeddings as blobs.");
        assert_eq!(m.tags, vec!["rust", "db"]);
        assert_eq!(m.source, "human");
        assert!(m.pinned);
        assert_eq!(m.created_at, 1_700_000_000);
        assert_eq!(m.embedding, Some(embedding));
        assert_eq!(m.last_accessed_at, m.created_at);

        let fetched = s.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(fetched, m);
        assert!(s.get_memory("missing").unwrap().is_none());
        assert!(s.delete_memory(&m.id).unwrap());
        assert!(!s.delete_memory(&m.id).unwrap());
        assert!(s.get_memory(&m.id).unwrap().is_none());
    }

    #[test]
    fn insert_validates_text_namespace_and_blank_id() {
        let s = store();
        assert!(matches!(
            s.insert_memory(&mem("ghost", "hi")).unwrap_err(),
            RecallError::NamespaceNotFound(_)
        ));
        ns(&s, "w");
        assert!(matches!(
            s.insert_memory(&mem("w", "   ")).unwrap_err(),
            RecallError::InvalidInput(_)
        ));
        let blank_id = NewMemory {
            id: Some("  ".into()),
            ..mem("w", "hello")
        };
        assert!(s.insert_memory(&blank_id).is_err());
    }

    #[test]
    fn duplicate_memory_id_is_reported_cleanly() {
        let s = store();
        ns(&s, "w");
        let m = NewMemory {
            id: Some("fixed-id".into()),
            ..mem("w", "first")
        };
        s.insert_memory(&m).unwrap();
        let again = NewMemory {
            id: Some("fixed-id".into()),
            ..mem("w", "second")
        };
        let err = s.insert_memory(&again).unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn insert_fills_defaults() {
        let s = store();
        ns(&s, "w");
        let m = s.insert_memory(&mem("w", "plain")).unwrap();
        assert_eq!(m.source, "agent");
        assert!(!m.pinned);
        assert!((crate::unix_now() - m.created_at).abs() < 5);
        assert!(m.embedding.is_none());
    }

    #[test]
    fn list_memories_filters_by_namespace_and_orders_newest_first() {
        let s = store();
        ns(&s, "a");
        ns(&s, "b");
        s.insert_memory(&NewMemory {
            created_at: Some(100),
            ..mem("a", "old a")
        })
        .unwrap();
        s.insert_memory(&NewMemory {
            created_at: Some(300),
            ..mem("a", "new a")
        })
        .unwrap();
        s.insert_memory(&NewMemory {
            created_at: Some(200),
            ..mem("b", "mid b")
        })
        .unwrap();

        let all_a = s.list_memories(Some("a"), 10, 0).unwrap();
        assert_eq!(
            all_a.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["new a", "old a"]
        );
        let all = s.list_memories(None, 10, 0).unwrap();
        assert_eq!(all.len(), 3);
        let one = s.list_memories(None, 1, 0).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].text, "new a");
        assert!(matches!(
            s.list_memories(Some("ghost"), 5, 0).unwrap_err(),
            RecallError::NamespaceNotFound(_)
        ));
        assert!(s.list_memories(None, 0, 0).is_err());
    }

    #[test]
    fn list_memories_saturates_wrapping_limit_and_offset() {
        // AR-004 root cause: `usize::MAX as i64` bound -1, which SQLite reads
        // as "unlimited". The bind must saturate instead of wrapping.
        let s = store();
        ns(&s, "w");
        s.insert_memory(&mem("w", "only row")).unwrap();
        let rows = s.list_memories(Some("w"), usize::MAX, 0).unwrap();
        assert_eq!(rows.len(), 1, "no error, no unlimited-bind surprise");
        // A wrapped offset yields an empty page (past the end), never an error.
        let rest = s.list_memories(Some("w"), 10, usize::MAX).unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn list_memories_paginates_with_offset_and_count_matches() {
        let s = store();
        ns(&s, "w");
        for i in 0..7 {
            s.insert_memory(&NewMemory {
                created_at: Some(i),
                ..mem("w", &format!("note {i}"))
            })
            .unwrap();
        }
        let page1 = s.list_memories(Some("w"), 3, 0).unwrap();
        let page2 = s.list_memories(Some("w"), 3, 3).unwrap();
        let page3 = s.list_memories(Some("w"), 3, 6).unwrap();
        let texts =
            |page: &[Memory]| -> Vec<String> { page.iter().map(|m| m.text.clone()).collect() };
        // Newest first: 6..4, then 3..1, then the last row.
        assert_eq!(texts(&page1), ["note 6", "note 5", "note 4"]);
        assert_eq!(texts(&page2), ["note 3", "note 2", "note 1"]);
        assert_eq!(texts(&page3), ["note 0"]);
        // Pagination covers the set exactly once.
        let mut seen: Vec<String> = Vec::new();
        for page in [&page1, &page2, &page3] {
            seen.extend(texts(page));
        }
        seen.sort();
        assert_eq!(seen.len(), 7);
        seen.dedup();
        assert_eq!(seen.len(), 7, "pages must not overlap");
        assert_eq!(s.count_memories(Some("w")).unwrap(), 7);
        assert_eq!(s.count_memories(None).unwrap(), 7);
        assert!(matches!(
            s.count_memories(Some("ghost")).unwrap_err(),
            RecallError::NamespaceNotFound(_)
        ));
    }

    #[test]
    fn maintenance_checkpoints_vacuums_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maint.db");
        {
            let s = SqliteStore::open(&path).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", "backup me")).unwrap();
            s.checkpoint_wal().unwrap();
            s.vacuum().unwrap();
        }
        // Backup into a fresh file, then open it independently.
        let backup = dir.path().join("backup.db");
        let s = SqliteStore::open(&path).unwrap();
        s.vacuum_into(&backup).unwrap();
        let restored = SqliteStore::open(&backup).unwrap();
        let listed = restored.list_memories(Some("w"), 10, 0).unwrap();
        assert_eq!(listed[0].text, "backup me");
        // An existing target is refused, not overwritten.
        assert!(s.vacuum_into(&backup).is_err());
        // Keyed stores produce keyed backups.
        let key = StoreKey::from_hex(&"b4".repeat(32)).unwrap();
        let keyed_path = dir.path().join("keyed.db");
        let ks = SqliteStore::open_with_key(&keyed_path, &key).unwrap();
        ns(&ks, "w");
        ks.insert_memory(&mem("w", SECRET)).unwrap();
        let keyed_backup = dir.path().join("keyed-backup.db");
        ks.vacuum_into(&keyed_backup).unwrap();
        let reopened = SqliteStore::open_with_key(&keyed_backup, &key).unwrap();
        assert_eq!(
            reopened.list_memories(Some("w"), 10, 0).unwrap()[0].text,
            SECRET
        );
        // The backup refuses an unkeyed open, proving it is encrypted.
        assert!(SqliteStore::open(&keyed_backup).is_err());
    }

    #[test]
    fn stats_counts_namespaces_memories_and_pinned() {
        let s = store();
        ns(&s, "a");
        ns(&s, "b");
        s.insert_memory(&NewMemory {
            pinned: true,
            ..mem("a", "p1")
        })
        .unwrap();
        s.insert_memory(&mem("a", "p2")).unwrap();
        let st = s.stats().unwrap();
        assert_eq!(st.total_namespaces, 2);
        assert_eq!(st.total_memories, 2);
        assert_eq!(st.pinned_memories, 1);
        assert_eq!(st.per_namespace.len(), 2);
        assert_eq!(st.per_namespace[0].namespace, "a");
        assert_eq!(st.per_namespace[0].count, 2);
        assert_eq!(st.per_namespace[1].count, 0);
    }

    #[test]
    fn recall_validates_params_query_and_namespace() {
        let s = store();
        let e = HashEmbedder::new_256();
        let p = RecallParams::default();
        assert!(matches!(
            s.recall("ghost", "q", Some(&e.embed("q")[..]), &p, 0)
                .unwrap_err(),
            RecallError::NamespaceNotFound(_)
        ));
        ns(&s, "w");
        assert!(matches!(
            s.recall("w", "   ", Some(&e.embed("q")[..]), &p, 0)
                .unwrap_err(),
            RecallError::InvalidInput(_)
        ));
        let bad_k = RecallParams {
            k: 0,
            ..Default::default()
        };
        assert!(s.recall("w", "q", None, &bad_k, 0).is_err());
    }

    #[test]
    fn recall_ranks_keyword_vector_and_pinned_with_breakdowns() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let now = 1_786_000_000;
        let day = 86_400;
        let text = "deploy the payments service on friday";
        // Four memories chosen so components can be compared in isolation:
        // fresh full match, stale identical match, pinned stale identical match, and a non-match.
        let target = s
            .insert_memory(&NewMemory {
                created_at: Some(now - 100),
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        let stale = s
            .insert_memory(&NewMemory {
                created_at: Some(now - 30 * day),
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        let pinned = s
            .insert_memory(&NewMemory {
                created_at: Some(now - 30 * day),
                pinned: true,
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        s.insert_memory(&NewMemory {
            created_at: Some(now),
            embedding: Some(e.embed("weekly team sync notes")),
            ..mem("w", "weekly team sync notes")
        })
        .unwrap();

        let q = e.embed("deploy payments service");
        let hits = s
            .recall(
                "w",
                "deploy payments service",
                Some(&q),
                &RecallParams::default(),
                now,
            )
            .unwrap();
        assert!(!hits.is_empty());

        // Every hit carries a complete, consistent breakdown.
        for h in &hits {
            let b = &h.breakdown;
            let recomputed =
                0.45f64.mul_add(b.bm25, 0.45 * b.vector) + 0.10 * b.recency + b.pinned_boost;
            assert!(
                (recomputed - b.total).abs() < 1e-9,
                "breakdown must sum to total"
            );
            assert!((0.0..=1.0).contains(&b.bm25));
        }

        let by_id = |id: &str| hits.iter().find(|h| h.memory.id == id).unwrap();
        // The pinned copy ranks above the otherwise-identical unpinned copy by exactly the boost.
        let pinned_hit = by_id(&pinned.id);
        let stale_hit = by_id(&stale.id);
        assert!(
            pinned_hit.breakdown.total > stale_hit.breakdown.total,
            "pinned should outrank its unpinned twin"
        );
        assert!((pinned_hit.breakdown.total - stale_hit.breakdown.total - 0.2).abs() < 1e-9);
        // The fresh match beats the 30-day-old one on recency alone (identical text).
        let target_hit = by_id(&target.id);
        assert!(target_hit.breakdown.recency > stale_hit.breakdown.recency);
        assert!(target_hit.breakdown.total > stale_hit.breakdown.total);
        // Deterministic total ordering: pinned twin > fresh match > stale twin
        // (boost 0.2 > fresh recency ~0.1 > stale recency ~0). This is the
        // documented ranking behavior of the default weights.
        let ids = hits
            .iter()
            .map(|h| h.memory.id.as_str())
            .collect::<Vec<_>>();
        let pos = |id: &str| ids.iter().position(|i| *i == id).unwrap();
        assert!(pos(&pinned.id) < pos(&target.id));
        assert!(pos(&target.id) < pos(&stale.id));
        assert!(pinned_hit.breakdown.total > target_hit.breakdown.total);
        assert!(target_hit.breakdown.total > stale_hit.breakdown.total);

        // `last_accessed_at` is write-time only now (AR-018): recall neither
        // touches it on disk nor rewrites the returned hits.
        assert_eq!(
            target_hit.memory.last_accessed_at,
            target_hit.memory.created_at
        );
        let untouched = s.get_memory(&target.id).unwrap().unwrap();
        assert_eq!(untouched.last_accessed_at, untouched.created_at);
    }

    #[test]
    fn recall_without_embedding_still_ranks_by_keyword() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let now = 1_786_000_000;
        let hit_text = "rust ownership rules";
        s.insert_memory(&NewMemory {
            embedding: Some(e.embed(hit_text)),
            created_at: Some(now),
            ..mem("w", hit_text)
        })
        .unwrap();
        let hits = s
            .recall("w", "rust ownership", None, &RecallParams::default(), now)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!((hits[0].breakdown.vector - 0.0).abs() < 1e-9);
        assert!(hits[0].breakdown.bm25 > 0.0);
        assert!(hits[0].breakdown.total > 0.0);
    }

    #[test]
    fn recall_with_zero_query_embedding_skips_vector_pass() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        s.insert_memory(&NewMemory {
            embedding: Some(e.embed("alpha beta")),
            created_at: Some(1),
            ..mem("w", "alpha beta")
        })
        .unwrap();
        let zero = vec![0.0f32; 256];
        let hits = s
            .recall("w", "alpha", Some(&zero), &RecallParams::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].breakdown.vector, 0.0);
    }

    #[test]
    fn recall_no_match_returns_empty_and_decay_disabled_zeroes_recency() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        s.insert_memory(&mem("w", "something unrelated")).unwrap();
        let q = e.embed("zzzqqq");
        let hits = s
            .recall("w", "zzzqqq", Some(&q), &RecallParams::default(), 5)
            .unwrap();
        assert!(hits.is_empty());

        let p = RecallParams {
            tau_days: None,
            ..Default::default()
        };
        let hits = s
            .recall("w", "something", Some(&e.embed("something")[..]), &p, 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].breakdown.recency, 0.0);
        assert!(hits[0].breakdown.total > 0.0);
    }

    #[test]
    fn recall_respects_k_and_fills_vector_component() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let now = 1_000;
        s.insert_memory(&NewMemory {
            created_at: Some(now),
            embedding: Some(e.embed("ferries")),
            ..mem("w", "ferries")
        })
        .unwrap();
        for i in 1..4 {
            let text = format!("note number {i} about ferries");
            s.insert_memory(&NewMemory {
                created_at: Some(now - i),
                embedding: Some(e.embed(&text)),
                ..mem("w", &text)
            })
            .unwrap();
        }
        let p = RecallParams {
            k: 2,
            ..Default::default()
        };
        let q = e.embed("ferries");
        let hits = s.recall("w", "ferries", Some(&q), &p, now).unwrap();
        assert_eq!(hits.len(), 2);
        // The exact-text, newest memory wins; bm25 normalization crowns it at 1.0
        // (shortest matching doc) and its embedding is identical to the query.
        assert!((hits[0].breakdown.bm25 - 1.0).abs() < 1e-9);
        assert_eq!(hits[0].memory.text, "ferries");
        assert!((hits[0].breakdown.vector - 1.0).abs() < 1e-5);
    }

    #[test]
    fn recall_is_deterministic() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        for text in ["alpha", "beta gamma", "gamma delta"] {
            s.insert_memory(&NewMemory {
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        }
        let q = e.embed("gamma");
        let a = s
            .recall("w", "gamma", Some(&q), &RecallParams::default(), 10)
            .unwrap();
        let b = s
            .recall("w", "gamma", Some(&q), &RecallParams::default(), 10)
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn fts_index_stays_synced_across_delete() {
        let s = store();
        ns(&s, "w");
        let m = s
            .insert_memory(&mem("w", "unique widget fingerprint"))
            .unwrap();
        let q = "widget";
        let p = RecallParams::default();
        assert_eq!(s.recall("w", q, None, &p, 1).unwrap().len(), 1);
        s.delete_memory(&m.id).unwrap();
        assert!(
            s.recall("w", q, None, &p, 1).unwrap().is_empty(),
            "FTS row must go with the memory"
        );
    }

    #[test]
    fn weights_honor_custom_values_in_ranking() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let now = 5_000;
        s.insert_memory(&NewMemory {
            created_at: Some(now),
            embedding: Some(e.embed("bright lizards fly")),
            ..mem("w", "bright lizards fly")
        })
        .unwrap();
        let q = e.embed("bright lizards fly");
        let p = RecallParams {
            weights: Weights {
                bm25: 0.0,
                vector: 1.0,
                recency: 0.0,
            },
            tau_days: Some(30.0),
            ..Default::default()
        };
        let hits = s.recall("w", "bright lizards", Some(&q), &p, now).unwrap();
        assert_eq!(hits.len(), 1);
        let b = &hits[0].breakdown;
        assert!(
            (b.total - b.vector).abs() < 1e-9,
            "only the vector weight should apply"
        );
    }

    // ---- Encryption at rest (D-014) ----

    const SECRET: &str = "the launch codes are hidden in the orchard";

    #[test]
    fn keyed_store_roundtrips_plaintext_through_api() {
        let s = keyed();
        ns(&s, "w");
        let m = s.insert_memory(&mem("w", SECRET)).unwrap();
        assert_eq!(m.text, SECRET);
        assert_eq!(s.get_memory(&m.id).unwrap().unwrap().text, SECRET);
        let listed = s.list_memories(Some("w"), 10, 0).unwrap();
        assert_eq!(listed[0].text, SECRET);
        assert_eq!(
            s.meta_get(META_AT_REST).unwrap().as_deref(),
            Some(AT_REST_ENC)
        );
    }

    #[test]
    fn keyed_store_recalls_by_keyword_and_vector() {
        let s = keyed();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let text = "deploy the payments service on friday";
        s.insert_memory(&NewMemory {
            embedding: Some(e.embed(text)),
            ..mem("w", text)
        })
        .unwrap();
        let q = e.embed(text);
        let hits = s
            .recall("w", text, Some(&q), &RecallParams::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].breakdown.bm25 > 0.0, "keyword pass must work keyed");
        assert!((hits[0].breakdown.vector - 1.0).abs() < 1e-5);
        assert_eq!(hits[0].memory.text, text);
    }

    #[test]
    fn encrypted_at_rest_keeps_plaintext_out_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enc.db");
        let key = StoreKey::from_hex(&"7f".repeat(32)).unwrap();
        {
            let s = SqliteStore::open_with_key(&path, &key).unwrap();
            s.create_namespace("w").unwrap();
            s.insert_memory(&mem("w", SECRET)).unwrap();
        }
        // Raw bytes of the database (and its WAL) must not contain the secret.
        let db = std::fs::read(&path).unwrap();
        let wal = std::fs::read(path.with_extension("db-wal")).unwrap_or_default();
        for buf in [&db, &wal] {
            let hay = String::from_utf8_lossy(buf);
            assert!(!hay.contains("launch codes"), "plaintext leaked to disk");
            assert!(!hay.contains("orchard"), "token leaked to disk");
        }
    }

    #[test]
    fn fts_index_stores_digests_not_plaintext() {
        let s = keyed();
        ns(&s, "w");
        s.insert_memory(&mem("w", "xylophone taxonomy")).unwrap();
        let tokens: String = {
            let mut stmt = s.conn.prepare("SELECT fts_tokens FROM memories").unwrap();
            stmt.query_row([], |r| r.get::<_, String>(0)).unwrap()
        };
        assert!(tokens.split(' ').all(|t| t.len() == 32), "digests only");
        assert!(!tokens.contains("xylophone"));
        // The FTS inverted index itself must also be digest-only.
        let mut stmt = s.conn.prepare("SELECT * FROM memories_fts").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let doc: String = row.get(0).unwrap();
            assert!(!doc.contains("xylophone"));
        }
    }

    #[test]
    fn keyed_store_roundtrips_and_recalls_unicode_text() {
        // Multi-byte text exercises the full keyed path: AEAD over UTF-8
        // bytes, FTS digests over unicode tokens, and digest-mode queries.
        let s = keyed();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let text = "中文笔记 — déploiement du service 🚀 vendredi";
        let m = s
            .insert_memory(&NewMemory {
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        assert_eq!(m.text, text);
        assert_eq!(s.get_memory(&m.id).unwrap().unwrap().text, text);
        // Keyword recall over an accented token that must survive tokenize.
        let q = e.embed("déploiement");
        let hits = s
            .recall("w", "déploiement", Some(&q), &RecallParams::default(), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory.text, text);
        // The stored FTS document holds only 32-hex digests, never the text.
        let tokens: String = {
            let mut stmt = s.conn.prepare("SELECT fts_tokens FROM memories").unwrap();
            stmt.query_row([], |r| r.get(0)).unwrap()
        };
        assert!(tokens.split(' ').all(|t| t.len() == 32));
        assert!(!tokens.contains("déploiement"));
    }

    #[test]
    fn wrong_key_fails_at_open_via_key_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.db");
        let good = StoreKey::from_hex(&"aa".repeat(32)).unwrap();
        let bad = StoreKey::from_hex(&"bb".repeat(32)).unwrap();
        {
            let s = SqliteStore::open_with_key(&path, &good).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", SECRET)).unwrap();
        }
        let err = match SqliteStore::open_with_key(&path, &bad) {
            Ok(_) => panic!("wrong key must be rejected"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("decryption failed"),
            "wrong key must be rejected: {err}"
        );
        // Correct key reopens and decrypts.
        let reopened = SqliteStore::open_with_key(&path, &good).unwrap();
        let m = reopened.list_memories(Some("w"), 10, 0).unwrap();
        assert_eq!(m[0].text, SECRET);
    }

    #[test]
    fn pre_hkdf_store_is_migrated_to_derived_subkeys_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pre-hkdf.db");
        let master = [0xddu8; 32];
        let legacy = StoreKey::legacy_from_bytes(&master);
        {
            // Hand-seal a database under the pre-key-separation layout: the
            // raw master is both AEAD and HMAC key.
            let s = SqliteStore::open_inner(&path, Some(legacy.clone())).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", SECRET)).unwrap();
            s.insert_memory(&mem("w", "deploy笔记 second")).unwrap();
        }
        // The same master through the derived-key path must migrate, not fail.
        let key = StoreKey::from_bytes(&master).unwrap();
        let s = SqliteStore::open_with_key(&path, &key).unwrap();
        let rows = s.list_memories(Some("w"), 10, 0).unwrap();
        assert_eq!(rows.len(), 2, "both memories survive the migration");
        assert!(rows.iter().any(|m| m.text == SECRET));

        // Rows are now sealed under the derived AEAD subkey and the FTS
        // digests under the derived MAC subkey.
        let m = &rows[0];
        let (stored_text, stored_tokens): (String, String) = {
            let mut stmt = s
                .conn
                .prepare("SELECT text, fts_tokens FROM memories WHERE id = ?1")
                .unwrap();
            stmt.query_row([&m.id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        assert_eq!(
            key.decrypt(&crate::crypto::text_context(&m.id), &stored_text)
                .unwrap(),
            m.text
        );
        assert!(
            legacy
                .decrypt(&crate::crypto::text_context(&m.id), &stored_text)
                .is_err(),
            "legacy raw-master key must no longer open the rows"
        );
        assert_eq!(stored_tokens, key.fts_tokens(&m.text));

        // Reopening with the same master is a plain verified open now.
        drop(s);
        let reopened = SqliteStore::open_with_key(&path, &key).unwrap();
        assert_eq!(reopened.list_memories(Some("w"), 10, 0).unwrap().len(), 2);
        // A different master still fails the key check after migration.
        let wrong = StoreKey::from_hex(&"ee".repeat(32)).unwrap();
        assert!(SqliteStore::open_with_key(&path, &wrong).is_err());
    }

    #[test]
    fn encrypted_database_requires_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.db");
        let key = StoreKey::from_hex(&"cc".repeat(32)).unwrap();
        {
            let s = SqliteStore::open_with_key(&path, &key).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", SECRET)).unwrap();
        }
        let err = match SqliteStore::open(&path) {
            Ok(_) => panic!("plain open must refuse an encrypted database"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("encrypted at rest"),
            "plain open must refuse: {err}"
        );
    }

    #[test]
    fn keyed_upgrade_shreds_plaintext_from_the_database_file() {
        // AR-013 regression: the in-place plaintext→keyed upgrade must not
        // leave the marker text recoverable in the main database file, and
        // the WAL must be truncated so plaintext-era frames cannot linger in
        // the sidecar.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("upgrade.db");
        let marker = "CANARY-topsecret-04f9c2d1";
        {
            let s = SqliteStore::open(&path).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", marker)).unwrap();
            s.checkpoint_wal().unwrap();
        }
        let key = StoreKey::from_hex(&"17".repeat(32)).unwrap();
        {
            let s = SqliteStore::open_with_key(&path, &key).unwrap();
            let rows = s.list_memories(Some("w"), 10, 0).unwrap();
            assert_eq!(rows[0].text, marker);
            // secure_delete is scoped to the upgrade window, not forever:
            // steady-state keyed rows are ciphertext.
            let secure_delete: i64 = s
                .conn
                .query_row("PRAGMA secure_delete", [], |r| r.get(0))
                .unwrap();
            assert_eq!(secure_delete, 0, "hygiene pragma must be restored");
            s.checkpoint_wal().unwrap();
        }
        let main = std::fs::read(&path).unwrap();
        assert!(
            !main.windows(marker.len()).any(|w| w == marker.as_bytes()),
            "marker plaintext must not survive the keyed upgrade in the main file"
        );
        let wal = std::fs::read(path.with_extension("db-wal")).unwrap_or_default();
        assert!(
            !wal.windows(marker.len()).any(|w| w == marker.as_bytes()),
            "no plaintext-era frames may remain in the WAL sidecar"
        );
    }

    #[test]
    fn legacy_plaintext_db_is_upgraded_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        // Build a database with the CURRENT code but plaintext... we cannot
        // produce a true v1 file without the old schema, so hand-craft one
        // with the exact v1 layout (memories without fts_tokens, FTS over
        // `text`, text-based triggers).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE namespaces (
                    id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, created_at INTEGER NOT NULL);
                CREATE TABLE memories (
                    id TEXT PRIMARY KEY,
                    namespace_id TEXT NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
                    text TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '[]', embedding BLOB,
                    source TEXT NOT NULL DEFAULT 'agent', pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL, last_accessed_at INTEGER NOT NULL);
                CREATE VIRTUAL TABLE memories_fts USING fts5(
                    text, content = 'memories', content_rowid = 'rowid');
                CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
                    INSERT INTO memories_fts(rowid, text) VALUES (new.rowid, new.text);
                END;
                CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
                    INSERT INTO memories_fts(memories_fts, rowid, text)
                    VALUES ('delete', old.rowid, old.text);
                END;
                INSERT INTO namespaces VALUES ('ns-1', 'w', 1);
                INSERT INTO memories (id, namespace_id, text, tags, created_at, last_accessed_at)
                    VALUES ('m-1', 'ns-1', 'the orchard launch codes', '[]', 1, 1);
                "#,
            )
            .unwrap();
        }
        // Keyed open upgrades: reads decrypt, keyword recall works, file is clean.
        let key = StoreKey::from_hex(&"d1".repeat(32)).unwrap();
        let s = SqliteStore::open_with_key(&path, &key).unwrap();
        let m = s.get_memory("m-1").unwrap().unwrap();
        assert_eq!(m.text, "the orchard launch codes");
        let hits = s
            .recall("w", "orchard", None, &RecallParams::default(), 2)
            .unwrap();
        assert_eq!(hits.len(), 1, "keyword recall must survive upgrade");
        // Marker written.
        assert_eq!(
            s.meta_get(META_AT_REST).unwrap().as_deref(),
            Some(AT_REST_ENC)
        );
        // Reopen without key: refused.
        assert!(SqliteStore::open(&path).is_err());
        // Reopen with key: still fine.
        let again = SqliteStore::open_with_key(&path, &key).unwrap();
        assert_eq!(again.get_memory("m-1").unwrap().unwrap().text, m.text);
    }

    #[test]
    fn mixed_legacy_rows_are_encrypted_on_keyed_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.db");
        let key = StoreKey::from_hex(&"e2".repeat(32)).unwrap();
        // Plaintext DB via current code, then enable encryption.
        {
            let s = SqliteStore::open(&path).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", "plain as day observation"))
                .unwrap();
        }
        {
            let s = SqliteStore::open_with_key(&path, &key).unwrap();
            // Explicit later timestamp: list_memories orders newest first.
            s.insert_memory(&NewMemory {
                created_at: Some(2_000_000_000),
                ..mem("w", SECRET)
            })
            .unwrap();
        }
        let db_bytes = std::fs::read(&path).unwrap();
        let hay = String::from_utf8_lossy(&db_bytes);
        assert!(
            !hay.contains("plain as day"),
            "legacy row must be re-encrypted"
        );
        assert!(!hay.contains("launch"), "no plaintext anywhere");
        let s = SqliteStore::open_with_key(&path, &key).unwrap();
        let texts = s
            .list_memories(Some("w"), 10, 0)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect::<Vec<_>>();
        // Newest first: the SECRET memory was created after the legacy row.
        assert_eq!(
            texts,
            vec![SECRET.to_string(), "plain as day observation".to_string()]
        );
    }

    #[test]
    fn plaintext_that_merely_looks_encrypted_survives_the_keyed_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sneaky.db");
        let sneaky = format!("{ENC_PREFIX}this is prose, not base64!");
        // Plaintext store holding a text that starts with the ciphertext
        // marker (a coincidental collision in user content).
        {
            let s = SqliteStore::open(&path).unwrap();
            ns(&s, "w");
            s.insert_memory(&mem("w", &sneaky)).unwrap();
        }
        // The keyed upgrade must treat it as plaintext and encrypt it, so
        // later reads decrypt back to the exact original instead of failing.
        let key = StoreKey::from_hex(&"f1".repeat(32)).unwrap();
        {
            let s = SqliteStore::open_with_key(&path, &key).unwrap();
            let m = s.list_memories(Some("w"), 10, 0).unwrap();
            assert_eq!(m.len(), 1);
            assert_eq!(
                m[0].text, sneaky,
                "literal prefix text must round-trip, not fail a doomed decrypt"
            );
            // And the file holds no plaintext of it. The upgrade ran in place,
            // so (per the D-014 limitation) the plaintext-era pages linger in
            // the freelist/WAL until a compact — exercise the documented
            // remedy (`vacuum` + checkpoint) before inspecting raw bytes.
            s.checkpoint_wal().unwrap();
            s.vacuum().unwrap();
            s.checkpoint_wal().unwrap();
            let db = std::fs::read(&path).unwrap();
            let wal = std::fs::read(path.with_extension("db-wal")).unwrap_or_default();
            for buf in [&db, &wal] {
                assert!(
                    !String::from_utf8_lossy(buf).contains("prose, not base64"),
                    "sneaky plaintext leaked to disk"
                );
            }
        }
        let reopened = SqliteStore::open_with_key(&path, &key).unwrap();
        let m = reopened.list_memories(Some("w"), 10, 0).unwrap();
        assert_eq!(m[0].text, sneaky);
        // Keyword recall over the (digest-indexed) literal text still works.
        assert_eq!(
            reopened
                .recall("w", "prose", None, &RecallParams::default(), 1)
                .unwrap()
                .len(),
            1
        );
    }

    // ---- Update path (D-015) ----

    #[test]
    fn update_memory_changes_only_requested_fields_and_keeps_fts_exact() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let m = s
            .insert_memory(&NewMemory {
                tags: vec!["old".into()],
                embedding: Some(e.embed("original text about ferries")),
                created_at: Some(1_000),
                ..mem("w", "original text about ferries")
            })
            .unwrap();

        // Text-only update: embedding cleared (stale vector worse than none),
        // tags/source/pinned/created_at preserved, FTS reindexed.
        let updated = s
            .update_memory(
                &m.id,
                &MemoryUpdate {
                    text: Some("rewritten text about catamarans".into()),
                    ..MemoryUpdate::default()
                },
            )
            .unwrap();
        assert_eq!(updated.text, "rewritten text about catamarans");
        assert_eq!(updated.tags, vec!["old"]);
        assert_eq!(updated.created_at, 1_000);
        assert_eq!(updated.embedding, None, "stale embedding must be dropped");

        // Keyword recall follows the new text, not the old one.
        assert_eq!(
            s.recall("w", "catamarans", None, &RecallParams::default(), 1)
                .unwrap()
                .len(),
            1
        );
        assert!(
            s.recall("w", "ferries", None, &RecallParams::default(), 1)
                .unwrap()
                .is_empty()
        );

        // Tags-only update keeps the embedding and updates tags.
        let retagged = s
            .update_memory(
                &m.id,
                &MemoryUpdate {
                    tags: Some(vec!["boats".into(), "new".into()]),
                    ..MemoryUpdate::default()
                },
            )
            .unwrap();
        assert_eq!(retagged.tags, vec!["boats", "new"]);
        assert_eq!(retagged.text, "rewritten text about catamarans");

        // Pinned-only update.
        let pinned = s
            .update_memory(
                &m.id,
                &MemoryUpdate {
                    pinned: Some(true),
                    ..MemoryUpdate::default()
                },
            )
            .unwrap();
        assert!(pinned.pinned);

        // Explicit embedding survives a text update.
        let v = e.embed("catamarans");
        let reembedded = s
            .update_memory(
                &m.id,
                &MemoryUpdate {
                    text: Some("catamarans again".into()),
                    embedding: Some(v.clone()),
                    ..MemoryUpdate::default()
                },
            )
            .unwrap();
        assert_eq!(reembedded.embedding, Some(v));
    }

    #[test]
    fn update_memory_validates_empty_text_and_unknown_ids() {
        let s = store();
        ns(&s, "w");
        let m = s.insert_memory(&mem("w", "keep me")).unwrap();
        assert!(matches!(
            s.update_memory(
                "ghost",
                &MemoryUpdate {
                    pinned: Some(true),
                    ..MemoryUpdate::default()
                }
            )
            .unwrap_err(),
            RecallError::MemoryNotFound(_)
        ));
        assert!(matches!(
            s.update_memory(&m.id, &MemoryUpdate::default())
                .unwrap_err(),
            RecallError::InvalidInput(_)
        ));
        assert!(matches!(
            s.update_memory(
                &m.id,
                &MemoryUpdate {
                    text: Some("   ".into()),
                    ..MemoryUpdate::default()
                }
            )
            .unwrap_err(),
            RecallError::InvalidInput(_)
        ));
        // The failed updates changed nothing.
        assert_eq!(s.get_memory(&m.id).unwrap().unwrap().text, "keep me");
    }

    #[test]
    fn update_memory_works_in_keyed_mode() {
        let s = keyed();
        ns(&s, "w");
        let m = s.insert_memory(&mem("w", SECRET)).unwrap();
        let updated = s
            .update_memory(
                &m.id,
                &MemoryUpdate {
                    text: Some("the codes moved to the boathouse".into()),
                    tags: Some(vec!["secret".into()]),
                    ..MemoryUpdate::default()
                },
            )
            .unwrap();
        assert_eq!(updated.text, "the codes moved to the boathouse");
        // Keyword recall tracks the new text through the digested index.
        let hits = s
            .recall("w", "boathouse", None, &RecallParams::default(), 1)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].breakdown.bm25 > 0.0);
        assert!(
            s.recall("w", "orchard", None, &RecallParams::default(), 1)
                .unwrap()
                .is_empty()
        );
        // And the file (in-memory here: index) holds no plaintext.
        let mut stmt = s.conn.prepare("SELECT fts_tokens FROM memories").unwrap();
        let tokens: String = stmt.query_row([], |r| r.get(0)).unwrap();
        assert!(!tokens.contains("boathouse"));
    }

    #[test]
    fn recall_vector_pass_returns_true_topk_by_cosine_at_scale() {
        // D-016 exactness: with vector-only weights, the bounded candidate
        // heap must return exactly the brute-force top-k by cosine — no
        // near-miss rows lost to the eviction.
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let now = 1_000_000;
        let mut texts = Vec::new();
        for i in 0..300 {
            let text = format!("topic {i} payload {}", ["alpha", "beta", "gamma"][i % 3]);
            texts.push(text.clone());
            s.insert_memory(&NewMemory {
                created_at: Some(now - i as i64),
                embedding: Some(e.embed(&text)),
                ..mem("w", &text)
            })
            .unwrap();
        }
        let query = "topic 42 alpha";
        let q = e.embed(query);
        let weights = Weights {
            bm25: 0.0,
            vector: 1.0,
            recency: 0.0,
        };
        let p = RecallParams {
            k: 7,
            weights,
            tau_days: None,
            pinned_boost: 0.0,
            ..Default::default()
        };
        let hits = s.recall("w", query, Some(&q), &p, now).unwrap();
        assert_eq!(hits.len(), 7);

        // Reference ranking: brute-force cosine over all 300 stored vectors,
        // same tie-breaks as the final sort (total desc = cosine desc here,
        // created_at desc, id asc).
        let all = s.list_memories(Some("w"), 1_000, 0).unwrap();
        let mut scored: Vec<(f64, i64, String)> = all
            .into_iter()
            .map(|m| {
                (
                    crate::util::cosine(&q, m.embedding.as_deref().unwrap()),
                    m.created_at,
                    m.id,
                )
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap()
                .then(b.1.cmp(&a.1))
                .then(a.2.cmp(&b.2))
        });
        let expected: Vec<String> = scored.iter().take(7).map(|(_, _, id)| id.clone()).collect();
        let got: Vec<String> = hits.iter().map(|h| h.memory.id.clone()).collect();
        assert_eq!(got, expected, "heap must reproduce the exact top-k");
        // Returned cosines must be non-increasing (correct rank order).
        let cosines: Vec<f64> = hits.iter().map(|h| h.breakdown.vector).collect();
        for pair in cosines.windows(2) {
            assert!(pair[0] >= pair[1]);
        }
    }

    #[test]
    fn recall_filters_by_all_required_tags() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let mk = |text: &str, tags: &[&str]| {
            s.insert_memory(&NewMemory {
                tags: tags.iter().map(|t| t.to_string()).collect(),
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap();
        };
        mk("rust async runtime deep dive", &["rust", "async"]);
        mk("rust ownership explained", &["rust"]);
        mk("typescript build tooling", &["ts"]);

        let p = RecallParams::default();
        // No filter: everything.
        assert_eq!(s.recall("w", "rust", None, &p, 1).unwrap().len(), 2);
        // Single tag filter.
        let only_rust = RecallParams {
            tags: vec!["rust".into()],
            ..Default::default()
        };
        assert_eq!(s.recall("w", "rust", None, &only_rust, 1).unwrap().len(), 2);
        // AND semantics: both tags must be present.
        let both = RecallParams {
            tags: vec!["rust".into(), "async".into()],
            ..Default::default()
        };
        let hits = s.recall("w", "rust runtime", None, &both, 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory.text, "rust async runtime deep dive");
        // Vector pass honors the filter too: query tokens overlap both rust
        // memories, but the tag filter narrows candidates to the AND match.
        let q = e.embed("rust async patterns");
        let hits = s
            .recall("w", "rust async patterns", Some(&q), &both, 1)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory.text, "rust async runtime deep dive");
        // Non-matching combination yields nothing.
        let mixed = RecallParams {
            tags: vec!["rust".into(), "ts".into()],
            ..Default::default()
        };
        assert!(s.recall("w", "rust", None, &mixed, 1).unwrap().is_empty());
        // Duplicate required tags behave like one.
        let dup = RecallParams {
            tags: vec!["rust".into(), "rust".into()],
            ..Default::default()
        };
        assert_eq!(s.recall("w", "rust", None, &dup, 1).unwrap().len(), 2);
    }

    // ---- Packed vector cache (D-019) ----

    #[test]
    fn packed_cache_stays_exact_across_writes_and_namespaces() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        let mk = |text: &str, at: i64| {
            s.insert_memory(&NewMemory {
                created_at: Some(at),
                embedding: Some(e.embed(text)),
                ..mem("w", text)
            })
            .unwrap()
        };
        let a = mk("alpha rooster", 100);
        // Shares the "alpha" token so it is always a keyword candidate (and
        // thus present in hits) regardless of its cosine.
        mk("alpha heron", 90);
        let q = e.embed("alpha rooster");
        let p = RecallParams::default();
        let now = 1_000;

        let first = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert_eq!(first[0].memory.id, a.id);
        // Warm cache: the repeat query is served from packed memory and must
        // be identical (proves the cache path does not change results).
        let warm = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert_eq!(first, warm);

        // Insert invalidates: a newer exact duplicate takes the top spot.
        let c = mk("alpha rooster", 200);
        let after_insert = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert_eq!(
            after_insert[0].memory.id, c.id,
            "insert must invalidate the packed cache"
        );

        // Update invalidates: rewriting the text drops the vector (D-015),
        // and the row no longer surfaces as a vector match.
        s.update_memory(
            &c.id,
            &MemoryUpdate {
                text: Some("gamma ibis".into()),
                ..MemoryUpdate::default()
            },
        )
        .unwrap();
        let after_update = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert!(!after_update.iter().any(|h| h.memory.id == c.id));

        // Pinned-only update invalidates: the boost shows through the
        // rebuilt cache.
        let b_id = s
            .list_memories(Some("w"), 10, 0)
            .unwrap()
            .into_iter()
            .find(|m| m.text == "alpha heron")
            .map(|m| m.id)
            .unwrap();
        s.update_memory(
            &b_id,
            &MemoryUpdate {
                pinned: Some(true),
                ..MemoryUpdate::default()
            },
        )
        .unwrap();
        let after_pin = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        let pinned = after_pin.iter().find(|h| h.memory.id == b_id).unwrap();
        assert!((pinned.breakdown.pinned_boost - p.pinned_boost).abs() < 1e-9);

        // Delete invalidates: the pinned row disappears from results.
        assert!(s.delete_memory(&b_id).unwrap());
        let after_delete = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert!(!after_delete.iter().any(|h| h.memory.id == b_id));

        // Namespace switch: the cache is keyed by namespace id and must
        // repopulate — first for x, then correctly again for w.
        ns(&s, "x");
        let x_id = s.find_namespace("x").unwrap().unwrap().id;
        s.insert_memory(&NewMemory {
            created_at: Some(50),
            embedding: Some(e.embed("alpha rooster")),
            ..mem("x", "alpha rooster")
        })
        .unwrap();
        let in_x = s.recall("x", "alpha rooster", Some(&q), &p, now).unwrap();
        assert_eq!(in_x.len(), 1);
        assert_eq!(in_x[0].memory.namespace_id, x_id);
        let back_in_w = s.recall("w", "alpha rooster", Some(&q), &p, now).unwrap();
        assert_eq!(back_in_w[0].memory.id, a.id);
    }

    #[test]
    fn recall_scan_cap_semantics_survive_the_packed_cache() {
        let s = store();
        let e = HashEmbedder::new_256();
        ns(&s, "w");
        // Six rows in insertion (= rowid) order. The query text is gibberish
        // (no FTS matches), and only the third row carries the query's exact
        // vector, so the scan cap alone decides whether it can surface.
        // Its created_at is the newest so it also wins recency among matches.
        for (i, text) in ["t-one", "t-two", "carrier", "t-four", "t-five", "t-six"]
            .into_iter()
            .enumerate()
        {
            let carrier = text == "carrier";
            s.insert_memory(&NewMemory {
                created_at: Some(if carrier { 900 } else { i as i64 }),
                embedding: Some(if carrier {
                    e.embed("zzq wvvx")
                } else {
                    e.embed(text)
                }),
                ..mem("w", text)
            })
            .unwrap();
        }
        let q = e.embed("zzq wvvx");
        let mut p = RecallParams {
            pinned_boost: 0.0,
            ..Default::default()
        };

        p.scan_cap = 2;
        let hits = s.recall("w", "zzq wvvx", Some(&q), &p, 1_000).unwrap();
        assert!(
            hits.iter().all(|h| h.memory.text != "carrier"),
            "scan_cap 2 must not scan the third rowid"
        );

        // A larger cap rebuilds the incomplete cache with more rows.
        p.scan_cap = 3;
        let hits = s.recall("w", "zzq wvvx", Some(&q), &p, 1_000).unwrap();
        assert_eq!(hits[0].memory.text, "carrier");
        assert!((hits[0].breakdown.vector - 1.0).abs() < 1e-5);

        // Shrinking the cap again must re-apply the LIMIT row-for-row: the
        // cache still holds 3 rows from the cap-3 fill, so the scan is
        // bounded back to 2 and the carrier row (rowid 3) is invisible —
        // exactly what the SQL scan with LIMIT 2 returns.
        p.scan_cap = 2;
        let hits = s.recall("w", "zzq wvvx", Some(&q), &p, 1_000).unwrap();
        assert!(
            hits.iter().all(|h| h.memory.text != "carrier"),
            "a shrunk scan_cap must not surface rows beyond the cap"
        );
    }

    #[test]
    fn packed_cache_reuses_and_rebuilds_by_namespace_dim_and_cap() {
        let s = store();
        ns(&s, "w");
        let e = HashEmbedder::new_256();
        for i in 0..3 {
            s.insert_memory(&NewMemory {
                created_at: Some(i),
                embedding: Some(e.embed(&format!("note {i}"))),
                ..mem("w", &format!("note {i}"))
            })
            .unwrap();
        }
        let ns_id = s.find_namespace("w").unwrap().unwrap().id;

        let mut slot = None;
        let first = packed_vectors_for(&mut slot, &s.conn, &ns_id, 256, 2).unwrap();
        assert_eq!(first.len(), 2, "fill respects the cap");
        assert!(!first.complete, "3 rows exist, only 2 cached");
        let first_data = first.data.as_ptr() as usize;
        let reused = packed_vectors_for(&mut slot, &s.conn, &ns_id, 256, 2).unwrap();
        assert_eq!(
            reused.data.as_ptr() as usize,
            first_data,
            "same namespace/dim/cap must reuse the cache"
        );

        // A bigger cap on an incomplete cache forces a rebuild.
        let bigger = packed_vectors_for(&mut slot, &s.conn, &ns_id, 256, 10).unwrap();
        assert_eq!(bigger.len(), 3);
        assert!(bigger.complete);
        let bigger_data = bigger.data.as_ptr() as usize;
        // A complete cache serves any cap without a rebuild.
        let any = packed_vectors_for(&mut slot, &s.conn, &ns_id, 256, 500).unwrap();
        assert_eq!(any.data.as_ptr() as usize, bigger_data);

        // A different namespace rebuilds (empty → complete cache).
        ns(&s, "x");
        let x_id = s.find_namespace("x").unwrap().unwrap().id;
        let other = packed_vectors_for(&mut slot, &s.conn, &x_id, 256, 10).unwrap();
        assert_eq!(other.len(), 0);
        assert!(other.complete);
        // A different query dimension rebuilds too (mixed-dim stores treat
        // mismatched rows as vector-less, mirroring the SQL path's 0.0).
        let redim = packed_vectors_for(&mut slot, &s.conn, &ns_id, 384, 10).unwrap();
        assert_eq!(redim.dim, 384);
        assert!(redim.has_vector.iter().all(|v| !*v));
    }
}

// Keyword-pass query-plan probe (ignored, release): seeds the 10k fixture,
// prints EXPLAIN QUERY PLAN for the current FTS candidate query and candidate
// replacements, then times each variant in isolation. Evidence for the
// "attack the FTS pass" remediation documented in docs/EVALUATION.md.
//
//   cargo test -p recall-core --release --lib fts_keyword_pass_query_plan_probe -- --ignored --nocapture
#[cfg(test)]
mod fts_plan_probe {
    use super::*;
    use std::time::Instant;

    fn seed_10k(s: &SqliteStore) {
        let topics = [
            "deploy",
            "database",
            "incident review",
            "design decision",
            "meeting notes",
            "onboarding",
            "performance",
            "security audit",
            "release checklist",
            "customer feedback",
        ];
        for i in 0..10_000usize {
            let text = format!(
                "{} note {}: follow up with the platform team about {}",
                topics[i % topics.len()],
                i,
                topics[(i * 7) % topics.len()]
            );
            s.insert_memory(&NewMemory {
                namespace: "bench".into(),
                text,
                tags: vec![],
                source: None,
                pinned: false,
                created_at: None,
                id: None,
                embedding: None,
            })
            .expect("insert");
        }
    }

    #[test]
    #[ignore = "seeds a 10k-memory store; run with --ignored --nocapture (release mode)"]
    fn fts_keyword_pass_query_plan_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = SqliteStore::open(dir.path().join("fts.db")).expect("open store");
        s.create_namespace("bench").expect("namespace");
        let seed_start = Instant::now();
        seed_10k(&s);
        eprintln!("seeded 10k in {:?}", seed_start.elapsed());
        let ns_id = s.find_namespace("bench").unwrap().unwrap().id;

        let fts_query = build_fts_query("incident review note 42 root cause", None).unwrap();
        let variants: Vec<(&str, String)> = vec![
            (
                "v0_current_join",
                "SELECT m.rowid, bm25(memories_fts) AS rank_score \
                 FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid \
                 WHERE memories_fts MATCH ?1 AND m.namespace_id = ?2 \
                 ORDER BY rank_score LIMIT 200"
                    .into(),
            ),
            (
                "v1_materialized_cte_in",
                "WITH ns AS MATERIALIZED (SELECT rowid FROM memories WHERE namespace_id = ?2) \
                 SELECT f.rowid, bm25(f.memories_fts) AS rank_score \
                 FROM memories_fts f \
                 WHERE f.memories_fts MATCH ?1 AND f.rowid IN (SELECT rowid FROM ns) \
                 ORDER BY rank_score LIMIT 200"
                    .into(),
            ),
            (
                "v2_no_join_fts_only",
                "SELECT rowid, bm25(memories_fts) AS rank_score \
                 FROM memories_fts \
                 WHERE memories_fts MATCH ?1 \
                 ORDER BY rank_score LIMIT 200"
                    .into(),
            ),
            (
                "v3_subquery_join",
                "SELECT f.rowid, bm25(f.memories_fts) AS rank_score \
                 FROM memories_fts f \
                 JOIN (SELECT rowid FROM memories WHERE namespace_id = ?2) n \
                   ON n.rowid = f.rowid \
                 WHERE f.memories_fts MATCH ?1 \
                 ORDER BY rank_score LIMIT 200"
                    .into(),
            ),
        ];

        for (name, sql) in &variants {
            println!("\n=== {name} ===");
            // 1) The plan.
            let mut plan_stmt = s
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("explain prepares");
            let mut plan_rows = plan_stmt
                .query(params![fts_query, ns_id])
                .expect("explain queries");
            while let Some(row) = plan_rows.next().expect("plan row") {
                let detail: String = row.get(3).expect("plan detail");
                println!("  plan: {detail}");
            }
            // 2) The time (200 iterations, rows consumed like the real pass).
            let mut stmt = s.conn.prepare(sql).expect("variant prepares");
            let mut total = std::time::Duration::ZERO;
            let mut rows_seen = 0usize;
            for _ in 0..200 {
                let t0 = Instant::now();
                let rows = stmt
                    .query_map(params![fts_query, ns_id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                    })
                    .expect("variant queries");
                let n = rows.filter_map(|r| r.ok()).count();
                rows_seen = n;
                total += t0.elapsed();
            }
            println!(
                "  rows: {rows_seen}, avg {:>6.3} ms/call over 200 iters",
                total.as_secs_f64() * 1000.0 / 200.0
            );
        }
    }
}
