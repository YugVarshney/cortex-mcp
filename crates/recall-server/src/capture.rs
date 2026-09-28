//! Capture ingestion (D-018) shared by the REST `POST /v1/capture` endpoint
//! and the MCP `capture` tool: one request type, one chunking/embedding
//! pipeline, one storage path — REST and MCP cannot drift.

use recall_core::{Embedder, Memory, NewMemory, RecallError, Store};
use schemars::JsonSchema;
use serde::Deserialize;

/// One transcript message. `role` is prefixed into the stored text and added
/// as a `role:<role>` tag for analytics; `at` becomes the chunks'
/// `created_at` when present.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureMessage {
    pub role: Option<String>,
    pub content: String,
    /// Optional message timestamp (unix seconds).
    #[serde(default)]
    pub at: Option<i64>,
}

/// Body of `POST /v1/capture` and the arguments of the MCP `capture` tool:
/// an agent transcript chunked into memories server-side.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureRequest {
    /// Namespace (workspace) to store the transcript in; created
    /// automatically if unknown (the agent-shaped path, D-009).
    pub namespace: String,
    /// Transcript messages; each becomes one or more memories.
    pub transcript: Vec<CaptureMessage>,
    /// Extra tags applied to every captured chunk.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Chunk size cap in characters (default 1000, clamped to 100..=8000).
    #[serde(default)]
    pub max_chunk_chars: Option<usize>,
}

/// A validated, fully embedded capture: chunks carry their vectors so the
/// storage commit never runs model inference under the store lock.
pub struct PreparedCapture {
    pub namespace: String,
    pub memories: Vec<NewMemory>,
}

/// Validate the request, chunk every message deterministically, and embed
/// each chunk. Inference happens here — outside any store lock.
pub fn prepare(
    embedder: &dyn Embedder,
    body: &CaptureRequest,
) -> Result<PreparedCapture, RecallError> {
    if body.namespace.trim().is_empty() {
        return Err(RecallError::InvalidInput(
            "namespace must not be empty".into(),
        ));
    }
    if body.transcript.is_empty() {
        return Err(RecallError::InvalidInput(
            "transcript must contain at least one message".into(),
        ));
    }
    let max_chars = body.max_chunk_chars.unwrap_or(1000).clamp(100, 8000);
    let mut memories: Vec<NewMemory> = Vec::new();
    for message in &body.transcript {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        let text = match message.role.as_deref().map(str::trim) {
            Some(role) if !role.is_empty() => format!("{role}: {content}"),
            _ => content.to_string(),
        };
        for chunk in recall_core::capture::chunk_text(&text, max_chars)? {
            let mut tags = body.tags.clone();
            if let Some(role) = message
                .role
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
            {
                tags.push(format!("role:{role}"));
            }
            memories.push(NewMemory {
                namespace: body.namespace.trim().to_string(),
                embedding: Some(embedder.embed(&chunk)),
                text: chunk,
                tags,
                source: Some("capture".into()),
                pinned: false,
                created_at: message.at,
                id: None,
            });
        }
    }
    if memories.is_empty() {
        return Err(RecallError::InvalidInput(
            "transcript contained no captureable text".into(),
        ));
    }
    Ok(PreparedCapture {
        namespace: body.namespace.trim().to_string(),
        memories,
    })
}

/// Store a prepared capture: the namespace is auto-created (D-009) and every
/// chunk is inserted under one store lock. Callers hold the lock via
/// `AppState::with_store`.
pub fn commit(store: &dyn Store, prepared: &PreparedCapture) -> Result<Vec<Memory>, RecallError> {
    store.get_or_create_namespace(&prepared.namespace)?;
    prepared
        .memories
        .iter()
        .map(|m| store.insert_memory(m))
        .collect()
}
