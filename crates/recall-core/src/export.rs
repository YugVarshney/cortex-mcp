//! JSON export/import of the whole store (backup and migration, PRD user story 4).

use serde::{Deserialize, Serialize};

use crate::error::{RecallError, Result};
use crate::models::{Memory, Namespace, NewMemory};
use crate::store::Store;

/// On-disk export format. `version` guards future format changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportData {
    pub version: u32,
    pub exported_at: i64,
    pub namespaces: Vec<Namespace>,
    pub memories: Vec<Memory>,
}

pub const EXPORT_VERSION: u32 = 1;

/// Page size for the internal full scan. Comfortably above any v1-scale store
/// while staying a positive `i64` through the store's `LIMIT` bind — passing
/// `usize::MAX` would wrap to a negative value, silently relying on SQLite's
/// "negative LIMIT = unlimited" rule.
const EXPORT_SCAN_LIMIT: usize = 1 << 40;

/// Snapshot every namespace and memory. Listing caps are internal-only here
/// (see `EXPORT_SCAN_LIMIT`; larger stores are documented out of v1 scope).
pub fn export(store: &dyn Store) -> Result<ExportData> {
    Ok(ExportData {
        version: EXPORT_VERSION,
        exported_at: crate::unix_now(),
        namespaces: store.list_namespaces()?,
        memories: store.list_memories(None, EXPORT_SCAN_LIMIT, 0)?,
    })
}

/// Result of an import run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportReport {
    pub namespaces_created: u64,
    pub memories_inserted: u64,
    /// Memories whose id already existed and were left untouched.
    pub memories_skipped: u64,
}

fn to_new(memory: &Memory, namespace: &str) -> NewMemory {
    NewMemory {
        namespace: namespace.to_string(),
        text: memory.text.clone(),
        tags: memory.tags.clone(),
        source: Some(memory.source.clone()),
        pinned: memory.pinned,
        created_at: Some(memory.created_at),
        id: Some(memory.id.clone()),
        embedding: memory.embedding.clone(),
    }
}

/// Import an export: namespaces are created if missing, memories keep their ids
/// and timestamps; duplicates are skipped, never overwritten. The whole import
/// runs in one transaction (AR-023): a failure anywhere — a dangling namespace
/// reference, an invalid row — rolls back every change instead of leaving a
/// partially imported store behind.
pub fn import(store: &dyn Store, data: &ExportData) -> Result<ImportReport> {
    if data.version != EXPORT_VERSION {
        return Err(RecallError::InvalidInput(format!(
            "unsupported export version {} (expected {EXPORT_VERSION})",
            data.version
        )));
    }
    let ns_by_id: std::collections::HashMap<String, String> = data
        .namespaces
        .iter()
        .map(|ns| (ns.id.clone(), ns.name.clone()))
        .collect();
    // Fail fast on structurally invalid exports before touching the store,
    // resolving each memory's namespace name up front.
    let rows: Vec<(&Memory, String)> = data
        .memories
        .iter()
        .map(|memory| {
            let namespace = ns_by_id.get(&memory.namespace_id).ok_or_else(|| {
                RecallError::InvalidInput(format!(
                    "export references unknown namespace id {}",
                    memory.namespace_id
                ))
            })?;
            Ok((memory, namespace.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut report = ImportReport {
        namespaces_created: 0,
        memories_inserted: 0,
        memories_skipped: 0,
    };
    let before = store.list_namespaces()?;
    store.write_tx(&mut |s: &dyn Store| {
        for ns in &data.namespaces {
            s.get_or_create_namespace(&ns.name)?;
        }
        let after = s.list_namespaces()?;
        report.namespaces_created = (after.len() - before.len()) as u64;
        for (memory, namespace) in &rows {
            if s.get_memory(&memory.id)?.is_some() {
                report.memories_skipped += 1;
                continue;
            }
            s.insert_memory(&to_new(memory, namespace))?;
            report.memories_inserted += 1;
        }
        Ok(())
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{Embedder as _, HashEmbedder};
    use crate::sqlite::SqliteStore;

    fn seeded() -> SqliteStore {
        let s = SqliteStore::open_in_memory().unwrap();
        let w = s.create_namespace("work").unwrap();
        s.insert_memory(&NewMemory {
            namespace: "work".into(),
            text: "rust test memory".into(),
            tags: vec!["t".into()],
            source: Some("human".into()),
            pinned: true,
            created_at: Some(1_234),
            id: Some("mem-1".into()),
            embedding: Some(HashEmbedder::new_256().embed("rust test memory")),
        })
        .unwrap();
        s.get_or_create_namespace("empty").unwrap();
        let _ = w;
        s
    }

    #[test]
    fn export_roundtrip_into_fresh_store_preserves_everything() {
        let src = seeded();
        let data = export(&src).unwrap();
        assert_eq!(data.version, 1);
        assert_eq!(data.namespaces.len(), 2);
        assert_eq!(data.memories.len(), 1);

        // The export itself must be valid JSON (the format backup users rely on).
        let json = serde_json::to_string(&data).unwrap();
        let parsed: ExportData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, data);

        let dst = SqliteStore::open_in_memory().unwrap();
        let report = import(&dst, &parsed).unwrap();
        assert_eq!(report.namespaces_created, 2);
        assert_eq!(report.memories_inserted, 1);
        assert_eq!(report.memories_skipped, 0);

        let restored = dst.get_memory("mem-1").unwrap().unwrap();
        assert_eq!(restored.text, "rust test memory");
        assert_eq!(restored.created_at, 1_234);
        assert!(restored.pinned);
        assert!(restored.embedding.is_some());
        assert!(dst.find_namespace("empty").unwrap().is_some());
    }

    #[test]
    fn import_is_idempotent_by_skipping_existing_ids() {
        let src = seeded();
        let data = export(&src).unwrap();
        let report = import(&src, &data).unwrap();
        assert_eq!(report.memories_inserted, 0);
        assert_eq!(report.memories_skipped, 1);
        assert_eq!(report.namespaces_created, 0);
        assert_eq!(src.stats().unwrap().total_memories, 1);
    }

    #[test]
    fn import_rejects_unknown_versions_and_dangling_namespace_ids() {
        let dst = SqliteStore::open_in_memory().unwrap();
        let mut bad = ExportData {
            version: 99,
            exported_at: 0,
            namespaces: vec![],
            memories: vec![],
        };
        assert!(import(&dst, &bad).is_err());

        bad.version = 1;
        bad.memories.push(Memory {
            id: "m".into(),
            namespace_id: "ghost".into(),
            text: "x".into(),
            tags: vec![],
            embedding: None,
            source: "agent".into(),
            pinned: false,
            created_at: 0,
            last_accessed_at: 0,
        });
        let err = import(&dst, &bad).unwrap_err();
        assert!(err.to_string().contains("unknown namespace id"));
    }

    #[test]
    fn failed_import_is_atomic_and_leaves_no_partial_state() {
        // AR-023 regression: an invalid row mid-import used to abort with the
        // earlier rows already committed. The whole import must roll back.
        let dst = SqliteStore::open_in_memory().unwrap();
        let data = ExportData {
            version: 1,
            exported_at: 0,
            namespaces: vec![crate::Namespace {
                id: "ns-1".into(),
                name: "work".into(),
                created_at: 0,
            }],
            memories: vec![
                Memory {
                    id: "m-1".into(),
                    namespace_id: "ns-1".into(),
                    text: "valid first row".into(),
                    tags: vec![],
                    embedding: None,
                    source: "agent".into(),
                    pinned: false,
                    created_at: 0,
                    last_accessed_at: 0,
                },
                Memory {
                    id: "m-2".into(),
                    namespace_id: "ns-1".into(),
                    text: "   ".into(), // blank text: insert_memory rejects it
                    tags: vec![],
                    embedding: None,
                    source: "agent".into(),
                    pinned: false,
                    created_at: 0,
                    last_accessed_at: 0,
                },
            ],
        };
        let err = import(&dst, &data).unwrap_err();
        assert!(err.to_string().contains("text"), "invalid row: {err}");
        // Nothing from the aborted import survives: no memories, no namespace.
        assert_eq!(
            dst.stats().unwrap().total_memories,
            0,
            "a failed import must not leave partial rows"
        );
        assert!(dst.list_namespaces().unwrap().is_empty());
        // The store keeps working and a fixed import succeeds afterwards.
        let mut fixed = data;
        fixed.memories[1].text = "valid second row".into();
        let report = import(&dst, &fixed).unwrap();
        assert_eq!(report.memories_inserted, 2);
    }

    #[test]
    fn export_of_empty_store_is_empty_and_importable() {
        let empty = SqliteStore::open_in_memory().unwrap();
        let data = export(&empty).unwrap();
        assert!(data.namespaces.is_empty() && data.memories.is_empty());
        let report = import(&SqliteStore::open_in_memory().unwrap(), &data).unwrap();
        assert_eq!(
            report,
            ImportReport {
                namespaces_created: 0,
                memories_inserted: 0,
                memories_skipped: 0
            }
        );
    }
}
