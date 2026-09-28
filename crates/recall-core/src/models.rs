//! Domain models shared by the store, scorer, HTTP API, MCP tools and CLI.

use serde::{Deserialize, Serialize};

use crate::error::{RecallError, Result};

/// A workspace namespace: memories never leak across namespaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Namespace {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

/// A single memory record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub id: String,
    pub namespace_id: String,
    pub text: String,
    pub tags: Vec<String>,
    /// Dense embedding (`f32` little-endian in SQLite). Absent if the writer
    /// had no embedder.
    pub embedding: Option<Vec<f32>>,
    pub source: String,
    pub pinned: bool,
    pub created_at: i64,
    /// Last-write timestamp (AR-018): recalls no longer refresh it — the
    /// write was pure amplification with no consumer. Kept as the hook for a
    /// future eviction/recency-of-use policy.
    pub last_accessed_at: i64,
}

/// Payload for creating a memory. `id`/`created_at`/`embedding` are optional so
/// importers can preserve originals while interactive callers let the store fill them.
#[derive(Debug, Clone, Deserialize)]
pub struct NewMemory {
    pub namespace: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

/// Payload for updating an existing memory in place (D-015). Every field is
/// optional; `None` means "leave unchanged". At least one field must be set.
/// If `text` changes and `embedding` is `None`, the store clears the stored
/// embedding rather than keep a vector that no longer matches the text —
/// callers with an embedder supply the fresh vector.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MemoryUpdate {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

impl MemoryUpdate {
    /// Whether any field would actually change something.
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.tags.is_none()
            && self.source.is_none()
            && self.pinned.is_none()
            && self.embedding.is_none()
    }
}

/// Score decomposition for one recalled memory. Exactly what the UI table renders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoreBreakdown {
    /// Normalized BM25 keyword score in `[0,1]` (1.0 = best keyword match of the candidate set).
    pub bm25: f64,
    /// Cosine similarity between query and memory embeddings in `[-1,1]`.
    /// Zero also when a keyword hit falls outside the `scan_cap` vector-scan
    /// window (namespaces larger than the cap, default 10 000 — documented
    /// approximation, AR-015).
    pub vector: f64,
    /// Recency factor `exp(-days/tau)` in `(0,1]`; 0 when decay is disabled.
    pub recency: f64,
    /// The flat boost actually applied because the memory is pinned (0 otherwise).
    pub pinned_boost: f64,
    /// `w_bm25*bm25 + w_vector*vector + w_recency*recency + pinned_boost`.
    pub total: f64,
}

/// A recalled memory plus its explainable ranking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallHit {
    #[serde(flatten)]
    pub memory: Memory,
    pub breakdown: ScoreBreakdown,
}

/// Tunable retrieval weights. Defaults per ARCHITECTURE.md.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Weights {
    #[serde(default = "default_bm25")]
    pub bm25: f64,
    #[serde(default = "default_vector")]
    pub vector: f64,
    #[serde(default = "default_recency")]
    pub recency: f64,
}

fn default_bm25() -> f64 {
    0.45
}
fn default_vector() -> f64 {
    0.45
}
fn default_recency() -> f64 {
    0.10
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            bm25: 0.45,
            vector: 0.45,
            recency: 0.10,
        }
    }
}

impl Weights {
    /// All weights must be finite and non-negative.
    pub fn validate(&self) -> Result<()> {
        for (name, w) in [
            ("bm25", self.bm25),
            ("vector", self.vector),
            ("recency", self.recency),
        ] {
            if !w.is_finite() || w < 0.0 {
                return Err(RecallError::InvalidInput(format!(
                    "weight {name} must be finite and >= 0"
                )));
            }
        }
        Ok(())
    }
}

/// Parameters for a recall query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallParams {
    /// Number of hits to return.
    #[serde(default = "default_k")]
    pub k: usize,
    /// Recency half-life knob in days (`tau` in `exp(-days/tau)`). `None` disables the recency term.
    #[serde(default = "default_tau")]
    pub tau_days: Option<f64>,
    #[serde(default)]
    pub weights: Weights,
    /// Flat boost added to the total for pinned memories.
    #[serde(default = "default_pinned_boost")]
    pub pinned_boost: f64,
    /// Max FTS candidates considered (upper bound for bm25 normalization set).
    #[serde(default = "default_candidate_cap")]
    pub candidate_cap: usize,
    /// Max rows scanned for the vector pass (brute-force cosine; see ADR-002).
    #[serde(default = "default_scan_cap")]
    pub scan_cap: usize,
    /// Restrict recall to memories carrying ALL of these tags (exact string
    /// match, case-sensitive). Empty = no tag filter.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_k() -> usize {
    5
}
fn default_tau() -> Option<f64> {
    Some(30.0)
}
fn default_pinned_boost() -> f64 {
    0.2
}
fn default_candidate_cap() -> usize {
    200
}
fn default_scan_cap() -> usize {
    10_000
}

impl Default for RecallParams {
    fn default() -> Self {
        Self {
            k: 5,
            tau_days: Some(30.0),
            weights: Weights::default(),
            pinned_boost: 0.2,
            candidate_cap: 200,
            scan_cap: 10_000,
            tags: Vec::new(),
        }
    }
}

impl RecallParams {
    pub fn validate(&self) -> Result<()> {
        if self.k == 0 || self.k > 100 {
            return Err(RecallError::InvalidInput(
                "k must be between 1 and 100".into(),
            ));
        }
        if !self.pinned_boost.is_finite() || self.pinned_boost < 0.0 {
            return Err(RecallError::InvalidInput(
                "pinned_boost must be finite and >= 0".into(),
            ));
        }
        if let Some(tau) = self.tau_days
            && (!tau.is_finite() || tau <= 0.0)
        {
            return Err(RecallError::InvalidInput(
                "tau_days must be finite and > 0".into(),
            ));
        }
        if self.candidate_cap == 0 || self.scan_cap == 0 {
            return Err(RecallError::InvalidInput("caps must be > 0".into()));
        }
        self.weights.validate()
    }
}

/// Server-level statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub total_namespaces: u64,
    pub total_memories: u64,
    pub pinned_memories: u64,
    pub per_namespace: Vec<NamespaceCount>,
}

/// Memory count for one namespace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamespaceCount {
    pub namespace: String,
    pub count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_defaults_match_architecture() {
        let w = Weights::default();
        assert_eq!(
            w,
            Weights {
                bm25: 0.45,
                vector: 0.45,
                recency: 0.10
            }
        );
        assert!(w.validate().is_ok());
    }

    #[test]
    fn weights_reject_negative_and_non_finite() {
        assert!(
            Weights {
                bm25: -0.1,
                ..Weights::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Weights {
                vector: f64::NAN,
                ..Weights::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Weights {
                recency: f64::INFINITY,
                ..Weights::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn recall_params_defaults_validate() {
        assert!(RecallParams::default().validate().is_ok());
    }

    #[test]
    fn recall_params_reject_bad_k() {
        for k in [0usize, 101] {
            let p = RecallParams {
                k,
                ..RecallParams::default()
            };
            assert!(p.validate().is_err(), "k={k} should be rejected");
        }
        assert!(
            RecallParams {
                k: 1,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            RecallParams {
                k: 100,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn recall_params_reject_bad_boost_tau_and_caps() {
        let bad_boost = RecallParams {
            pinned_boost: -1.0,
            ..Default::default()
        };
        assert!(bad_boost.validate().is_err());
        let nan_boost = RecallParams {
            pinned_boost: f64::NAN,
            ..Default::default()
        };
        assert!(nan_boost.validate().is_err());
        let bad_tau = RecallParams {
            tau_days: Some(0.0),
            ..Default::default()
        };
        assert!(bad_tau.validate().is_err());
        let neg_tau = RecallParams {
            tau_days: Some(-5.0),
            ..Default::default()
        };
        assert!(neg_tau.validate().is_err());
        let nan_tau = RecallParams {
            tau_days: Some(f64::NAN),
            ..Default::default()
        };
        assert!(nan_tau.validate().is_err());
        let no_caps = RecallParams {
            candidate_cap: 0,
            ..Default::default()
        };
        assert!(no_caps.validate().is_err());
        let no_scan = RecallParams {
            scan_cap: 0,
            ..Default::default()
        };
        assert!(no_scan.validate().is_err());
    }

    #[test]
    fn recall_params_disable_decay_via_none() {
        let p = RecallParams {
            tau_days: None,
            ..Default::default()
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn new_memory_defaults_deserialize() {
        let m: NewMemory = serde_json::from_str(r#"{"namespace":"w","text":"hello"}"#).unwrap();
        assert_eq!(m.namespace, "w");
        assert!(m.tags.is_empty());
        assert_eq!(m.source, None);
        assert!(!m.pinned);
        assert_eq!(m.created_at, None);
        assert_eq!(m.id, None);
        assert_eq!(m.embedding, None);
    }

    #[test]
    fn memory_update_defaults_and_emptiness() {
        let u: MemoryUpdate = serde_json::from_str("{}").unwrap();
        assert!(u.is_empty(), "no fields set");
        let u: MemoryUpdate = serde_json::from_str(r#"{"pinned":true}"#).unwrap();
        assert!(!u.is_empty());
        assert_eq!(u.pinned, Some(true));
        let u: MemoryUpdate =
            serde_json::from_str(r#"{"text":"rewritten","tags":["a","b"]}"#).unwrap();
        assert_eq!(u.text.as_deref(), Some("rewritten"));
        assert_eq!(
            u.tags.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
    }

    #[test]
    fn recall_params_tags_default_to_no_filter() {
        let p: RecallParams = serde_json::from_str("{}").unwrap();
        assert!(p.tags.is_empty());
        let p: RecallParams = serde_json::from_str(r#"{"k":3,"tags":["ops","rust"]}"#).unwrap();
        assert_eq!(p.tags, vec!["ops", "rust"]);
        assert!(p.validate().is_ok());
    }
}
