//! Optional OpenAI-compatible embedding client, behind the `openai` feature
//! (ADR-003: API embedders are pluggable opt-ins; the default remains offline).
//!
//! The HTTP call itself is intentionally untested in CI (no live key); request
//! construction and response parsing are pure functions with unit tests.

use serde::{Deserialize, Serialize};

use crate::embed::Embedder;
use crate::error::{RecallError, Result};

#[derive(Debug, Clone)]
pub struct OpenAIEmbedder {
    endpoint: String,
    api_key: String,
    model: String,
    dims: usize,
    /// Shared HTTP client: reqwest pools connections per client, so building
    /// one per embed call paid TLS/DNS setup on every single embedding
    /// (AR-016). Built once here; capturing a 50-chunk transcript reuses it
    /// for all 50 calls.
    client: reqwest::blocking::Client,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

impl OpenAIEmbedder {
    /// Create a client. `endpoint` is the full embeddings URL,
    /// e.g. `https://api.openai.com/v1/embeddings`.
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(RecallError::InvalidInput(
                "OpenAI api key must not be empty".into(),
            ));
        }
        if dims == 0 {
            return Err(RecallError::InvalidInput(
                "embedding dimension must be > 0".into(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| RecallError::Embedder(e.to_string()))?;
        Ok(Self {
            endpoint: endpoint.into(),
            api_key,
            model: model.into(),
            dims,
            client,
        })
    }

    /// OpenAI's default embeddings endpoint.
    pub fn openai(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Result<Self> {
        Self::new("https://api.openai.com/v1/embeddings", api_key, model, dims)
    }

    /// Build the JSON request body sent to the API.
    pub(crate) fn request_body(&self, text: &str) -> String {
        serde_json::to_string(&EmbeddingRequest {
            model: &self.model,
            input: text,
        })
        .unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse an API response body into the embedding vector. The returned
    /// vector must have exactly `expected_dims` entries: a mismatch would
    /// otherwise silently zero every cosine against stored vectors (the
    /// mismatch rule returns 0.0 for different lengths), degrading recall to
    /// keyword-only with no error anywhere (AR-005). Fail loudly instead.
    pub(crate) fn parse_response(body: &str, expected_dims: usize) -> Result<Vec<f32>> {
        let parsed: EmbeddingResponse = serde_json::from_str(body)
            .map_err(|e| RecallError::Embedder(format!("bad embedding response: {e}")))?;
        let mut items = parsed.data;
        if items.len() != 1 {
            return Err(RecallError::Embedder(format!(
                "expected exactly 1 embedding, got {}",
                items.len()
            )));
        }
        let vec = items.swap_remove(0).embedding;
        if vec.is_empty() {
            return Err(RecallError::Embedder("embedding is empty".into()));
        }
        if vec.len() != expected_dims {
            return Err(RecallError::Embedder(format!(
                "embedding dimension mismatch: endpoint returned {} dims but {} are configured \
                 (RECALL_MCP_OPENAI_DIMS); align the configuration with the model/endpoint",
                vec.len(),
                expected_dims
            )));
        }
        Ok(vec)
    }
}

impl Embedder for OpenAIEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embedding_invertible(&self) -> bool {
        // Dense API embeddings (text-embedding-3-class): published
        // embedding-inversion attacks recover input text with usable
        // fidelity (AR-003, D-014).
        true
    }

    fn name(&self) -> &'static str {
        "openai"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        // Blocking HTTP; shells call this via `spawn_blocking`. Failures surface
        // as a zero-vector-free error path: we return an empty vector and let
        // callers treat missing/empty embeddings as "no vector signal".
        // (Errors are logged; the hybrid scorer degrades to keyword-only.)
        match self.embed_checked(text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "OpenAI embed failed; degrading to keyword-only");
                Vec::new()
            }
        }
    }
}

impl OpenAIEmbedder {
    fn embed_checked(&self, text: &str) -> Result<Vec<f32>> {
        let response = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(self.request_body(text))
            .send()
            .map_err(|e| RecallError::Embedder(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|e| RecallError::Embedder(e.to_string()))?;
        if !status.is_success() {
            return Err(RecallError::Embedder(format!(
                "embeddings API returned {status}"
            )));
        }
        Self::parse_response(&body, self.dims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_requires_key_and_dims() {
        assert!(OpenAIEmbedder::openai("", "text-embedding-3-small", 1536).is_err());
        assert!(OpenAIEmbedder::openai("  ", "m", 1536).is_err());
        assert!(OpenAIEmbedder::new("https://x/v1/embeddings", "key", "m", 0).is_err());
        let e = OpenAIEmbedder::openai("sk-test", "text-embedding-3-small", 1536).unwrap();
        assert_eq!(e.dimensions(), 1536);
        assert_eq!(e.name(), "openai");
    }

    #[test]
    fn request_body_matches_api_schema() {
        let e = OpenAIEmbedder::new("https://x", "k", "text-embedding-3-small", 1536).unwrap();
        assert_eq!(
            e.request_body("hello world"),
            r#"{"model":"text-embedding-3-small","input":"hello world"}"#
        );
    }

    #[test]
    fn parse_response_validates_payload() {
        let good = r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#;
        assert_eq!(
            OpenAIEmbedder::parse_response(good, 3).unwrap(),
            vec![0.1, 0.2, 0.3]
        );

        assert!(OpenAIEmbedder::parse_response("not json", 3).is_err());
        assert!(OpenAIEmbedder::parse_response(r#"{"data":[]}"#, 3).is_err());
        assert!(OpenAIEmbedder::parse_response(r#"{"data":[{"embedding":[]}]}"#, 3).is_err());
        assert!(
            OpenAIEmbedder::parse_response(r#"{"data":[{"embedding":[1]},{"embedding":[2]}]}"#, 3)
                .is_err()
        );
    }

    #[test]
    fn parse_response_rejects_dimension_mismatch_instead_of_silently_zeroing() {
        // AR-005: a 1536-dim API response while 512 dims are configured used
        // to be accepted verbatim; every stored cosine then silently returned
        // 0.0 (the length-mismatch rule) and recall degraded to keyword-only
        // with no error anywhere.
        let body = r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#;
        let err = OpenAIEmbedder::parse_response(body, 512).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("dimension mismatch")
                && message.contains("512")
                && message.contains("3"),
            "error must name both sides of the mismatch: {message}"
        );
        // The embed() wrapper surfaces the same failure (degrades to empty).
        let e = OpenAIEmbedder::new("http://127.0.0.1:1", "k", "m", 512).unwrap();
        assert_eq!(e.dimensions(), 512);
    }

    #[test]
    fn embed_degrades_to_empty_vector_on_connection_error() {
        // Port 1 on localhost refuses connections quickly; proves the error path.
        let e = OpenAIEmbedder::new("http://127.0.0.1:1/v1/embeddings", "k", "m", 4).unwrap();
        assert!(e.embed("hello").is_empty());
        // Dense API embeddings must flag invertibility (keyed-mode warning).
        assert!(e.embedding_invertible());
    }
}
