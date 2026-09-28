//! Embedding abstraction: deterministic hash embedder by default, pluggable API
//! embedders behind features (ADR-003).

use crate::error::{RecallError, Result};
use crate::util::{fnv1a64, tokenize};

/// Turns text into a fixed-length dense vector.
pub trait Embedder: Send + Sync {
    fn dimensions(&self) -> usize;
    fn name(&self) -> &'static str;
    /// Embed `text`; never panics. Empty/whitespace text yields a zero vector.
    fn embed(&self, text: &str) -> Vec<f32>;
    /// Whether this embedder produces dense neural sentence embeddings — the
    /// class that published embedding-inversion attacks (vec2text-style) can
    /// partially reverse into text. The server/CLI use this to warn when
    /// keyed (encrypted-at-rest) mode runs with such an embedder: the stored
    /// plaintext embedding then leaks memory text despite the AEAD layer
    /// (AR-003, D-014). Lexical embedders (hash) are not invertible this way.
    fn embedding_invertible(&self) -> bool {
        false
    }
}

/// Feature-hashing embedder: deterministic, offline, no model download (ADR-003).
///
/// Each token is FNV-1a hashed to a bucket index and a sign; accumulated values
/// are L2-normalized. Same text always yields the same vector, on any platform.
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    /// Default 256-dimensional embedder.
    pub fn new_256() -> Self {
        Self { dim: 256 }
    }

    /// Custom dimensionality; must be non-zero.
    pub fn new(dim: usize) -> Result<Self> {
        if dim == 0 {
            return Err(RecallError::InvalidInput(
                "embedding dimension must be > 0".into(),
            ));
        }
        Ok(Self { dim })
    }
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self::new_256()
    }
}

impl Embedder for HashEmbedder {
    fn dimensions(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &'static str {
        "hash-256"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut buckets = vec![0.0f32; self.dim];
        for token in tokenize(text) {
            let h = fnv1a64(token.as_bytes());
            let idx = (h % self.dim as u64) as usize;
            let sign = if h >> 63 & 1 == 1 { 1.0f32 } else { -1.0 };
            buckets[idx] += sign;
        }
        let norm = buckets
            .iter()
            .map(|v| {
                let v = f64::from(*v);
                v * v
            })
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            return buckets;
        }
        buckets.iter().map(|v| v / norm as f32).collect()
    }
}

/// The embedder kinds accepted by [`parse_embedder_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedderKind {
    /// Deterministic feature hashing (ADR-003 default).
    Hash,
    /// Local ONNX sentence-transformer (D-013; needs the `onnx` feature).
    Onnx,
    /// OpenAI-compatible HTTP API (ADR-003 opt-in; needs the `openai` feature).
    OpenAi,
}

/// Parse an embedder name (`hash` | `onnx` | `openai`) without constructing
/// it. Case-insensitive; blank means the default (`hash`). Unknown names are
/// typed errors that name the accepted values, so a typo can never silently
/// degrade retrieval quality.
pub fn parse_embedder_name(name: &str) -> Result<EmbedderKind> {
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "hash" => Ok(EmbedderKind::Hash),
        "onnx" => Ok(EmbedderKind::Onnx),
        "openai" => Ok(EmbedderKind::OpenAi),
        other => Err(RecallError::InvalidInput(format!(
            "unknown embedder {other:?}: expected one of \"hash\", \"onnx\", \"openai\""
        ))),
    }
}

/// Parse and construct the embedder named `name` (the CLI/server
/// `--embedder` selection, D-003 + D-013). Construction can require runtime
/// resources — `onnx` loads the ONNX Runtime library (`ORT_DYLIB_PATH`) and
/// downloads the model on first use — and both `onnx` and `openai` require
/// their cargo features at build time; asking for them elsewhere is an
/// error, never a silent fallback to `hash`.
pub fn select_embedder(name: &str) -> Result<std::sync::Arc<dyn Embedder>> {
    match parse_embedder_name(name)? {
        EmbedderKind::Hash => Ok(std::sync::Arc::new(HashEmbedder::new_256())),
        #[cfg(feature = "onnx")]
        EmbedderKind::Onnx => Ok(std::sync::Arc::new(crate::onnx_embed::OnnxEmbedder::new()?)),
        #[cfg(not(feature = "onnx"))]
        EmbedderKind::Onnx => Err(RecallError::Embedder(
            "embedder \"onnx\" requires a build with the `onnx` cargo feature \
             (plus ORT_DYLIB_PATH pointing at onnxruntime.dll at runtime)"
                .into(),
        )),
        #[cfg(feature = "openai")]
        EmbedderKind::OpenAi => Ok(std::sync::Arc::new(openai_from_env()?)),
        #[cfg(not(feature = "openai"))]
        EmbedderKind::OpenAi => Err(RecallError::Embedder(
            "embedder \"openai\" requires a build with the `openai` cargo feature".into(),
        )),
    }
}

/// Build [`crate::openai_embed::OpenAIEmbedder`] from the environment:
/// `RECALL_MCP_OPENAI_API_KEY` (required), `RECALL_MCP_OPENAI_MODEL`
/// (default `text-embedding-3-small`), `RECALL_MCP_OPENAI_ENDPOINT` (default
/// OpenAI's endpoint), `RECALL_MCP_OPENAI_DIMS` (default 1536). `DIMS` must
/// match what the model/endpoint actually returns — every response vector is
/// validated against it and a mismatch fails loudly instead of silently
/// zeroing all vector scores (AR-005).
#[cfg(feature = "openai")]
fn openai_from_env() -> Result<crate::openai_embed::OpenAIEmbedder> {
    let api_key = std::env::var("RECALL_MCP_OPENAI_API_KEY").unwrap_or_default();
    let model = std::env::var("RECALL_MCP_OPENAI_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "text-embedding-3-small".into());
    let dims = std::env::var("RECALL_MCP_OPENAI_DIMS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1536);
    match std::env::var("RECALL_MCP_OPENAI_ENDPOINT") {
        Ok(endpoint) if !endpoint.trim().is_empty() => {
            crate::openai_embed::OpenAIEmbedder::new(endpoint, api_key, model, dims)
        }
        _ => crate::openai_embed::OpenAIEmbedder::openai(api_key, model, dims),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_256_dims_by_default() {
        let e = HashEmbedder::default();
        assert_eq!(e.dimensions(), 256);
        assert_eq!(e.name(), "hash-256");
        // Lexical hashing is not invertible: no keyed-mode leak warning.
        assert!(!e.embedding_invertible());
        let a = e.embed("deploy the service on friday");
        let b = e.embed("deploy the service on friday");
        assert_eq!(a.len(), 256);
        assert_eq!(a, b);
    }

    #[test]
    fn different_text_differs_but_stays_normalized() {
        let e = HashEmbedder::new_256();
        let a = e.embed("kubernetes rollout");
        let b = e.embed("database migration");
        assert_ne!(a, b);
        let norm: f64 = a
            .iter()
            .map(|v| (*v as f64) * (*v as f64))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "L2 norm should be 1, got {norm}");
        // Unit vectors: cosine of identical text is 1, and components are bounded.
        assert!(a.iter().all(|v| v.abs() <= 1.0));
    }

    #[test]
    fn empty_and_whitespace_are_zero_vectors() {
        let e = HashEmbedder::new_256();
        for text in ["", "   ", "!!!"] {
            let v = e.embed(text);
            assert!(v.iter().all(|f| *f == 0.0), "{text:?} should embed to zero");
        }
    }

    #[test]
    fn case_and_punctuation_invariant() {
        let e = HashEmbedder::new_256();
        assert_eq!(e.embed("Postgres WAL"), e.embed("postgres wal"));
    }

    #[test]
    fn custom_dim_and_zero_dim_error() {
        assert_eq!(HashEmbedder::new(64).expect("64 dims").dimensions(), 64);
        let err = HashEmbedder::new(0).unwrap_err();
        assert!(err.to_string().contains("dimension must be > 0"));
    }

    #[test]
    fn dim_one_still_works() {
        let e = HashEmbedder::new(1).expect("1 dim");
        assert_eq!(e.embed("hello").len(), 1);
    }

    #[test]
    fn embedder_name_parses_all_kinds_and_rejects_unknown() {
        assert_eq!(parse_embedder_name("hash").unwrap(), EmbedderKind::Hash);
        // Blank names mean the default; matching is case-insensitive and
        // trims whitespace (flag/env values are human input).
        assert_eq!(parse_embedder_name("").unwrap(), EmbedderKind::Hash);
        assert_eq!(parse_embedder_name("   ").unwrap(), EmbedderKind::Hash);
        assert_eq!(parse_embedder_name("  ONNX ").unwrap(), EmbedderKind::Onnx);
        assert_eq!(parse_embedder_name("OpenAI").unwrap(), EmbedderKind::OpenAi);
        let err = parse_embedder_name("gpt4").unwrap_err();
        assert!(err.to_string().contains("unknown embedder"), "got: {err}");
        assert!(
            err.to_string().contains("openai"),
            "must name the accepted values"
        );
    }

    #[test]
    fn select_embedder_default_builds_hash() {
        for name in ["", "hash", "HASH"] {
            let e = select_embedder(name).expect("hash selection");
            assert_eq!(e.name(), "hash-256");
            assert_eq!(e.dimensions(), 256);
        }
    }

    #[cfg(not(feature = "onnx"))]
    #[test]
    fn select_embedder_onnx_requires_the_feature() {
        // `.err()` (not `unwrap_err()`): the success side is a trait object
        // without Debug, and the error text is what the test asserts on.
        let err = select_embedder("onnx")
            .err()
            .expect("onnx must fail without the feature");
        let msg = err.to_string();
        assert!(msg.contains("onnx"), "got: {msg}");
        assert!(
            msg.contains("feature"),
            "error must point at the build flag: {msg}"
        );
    }

    #[cfg(not(feature = "openai"))]
    #[test]
    fn select_embedder_openai_requires_the_feature() {
        let err = select_embedder("openai")
            .err()
            .expect("openai must fail without the feature");
        let msg = err.to_string();
        assert!(msg.contains("openai"), "got: {msg}");
        assert!(
            msg.contains("feature"),
            "error must point at the build flag: {msg}"
        );
    }

    #[cfg(feature = "openai")]
    #[test]
    fn select_embedder_openai_builds_from_env() {
        // Serialized: mutates process-global env (edition-2024 unsafe access;
        // this lock is the safety argument).
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: ENV_LOCK serializes process-global env access in tests.
        unsafe { std::env::set_var("RECALL_MCP_OPENAI_API_KEY", "sk-test") };
        let e = select_embedder("openai").expect("openai selection");
        assert_eq!(e.name(), "openai");
        assert_eq!(e.dimensions(), 1536);
        // A missing key is an error, never a silent fallback.
        // SAFETY: ENV_LOCK serializes process-global env access in tests.
        unsafe { std::env::remove_var("RECALL_MCP_OPENAI_API_KEY") };
        assert!(select_embedder("openai").is_err());
    }
}
