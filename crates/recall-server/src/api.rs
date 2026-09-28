//! REST API handlers (`/v1/*`, `/healthz`, `/openapi.json`).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use recall_core::{Memory, MemoryUpdate, NewMemory, RecallError, RecallHit, RecallParams, Weights};
use serde::Deserialize;

use crate::error::HttpError;
use crate::state::AppState;

pub async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "cortex-mcp",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn openapi() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        crate::openapi::DOCUMENT,
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateNamespace {
    pub name: String,
}

pub async fn create_namespace(
    State(state): State<AppState>,
    Json(body): Json<CreateNamespace>,
) -> Result<Response, HttpError> {
    let ns = state.with_store(|store| store.create_namespace(&body.name))?;
    tracing::info!(namespace = %ns.name, "created namespace via API");
    Ok((StatusCode::CREATED, Json(ns)).into_response())
}

pub async fn list_namespaces(State(state): State<AppState>) -> Result<Response, HttpError> {
    let namespaces = state.with_store(|store| store.list_namespaces())?;
    Ok(Json(namespaces).into_response())
}

pub async fn create_memory(
    State(state): State<AppState>,
    Json(mut body): Json<NewMemory>,
) -> Result<Response, HttpError> {
    if body.text.trim().is_empty() {
        return Err(RecallError::InvalidInput("text must not be empty".into()).into());
    }
    // A caller-supplied vector of the wrong length would silently zero every
    // cosine against this store's embedder (the mismatch rule returns 0.0) —
    // reject it instead of accepting silent scoring loss (AR-005).
    if let Some(vec) = &body.embedding
        && vec.len() != state.embedder.dimensions()
    {
        return Err(RecallError::InvalidInput(format!(
            "embedding has {} dims but the configured embedder {:?} produces {}",
            vec.len(),
            state.embedder.name(),
            state.embedder.dimensions()
        ))
        .into());
    }
    // Embed here when the caller did not supply a vector (imports pass their own).
    if body.embedding.is_none() {
        body.embedding = Some(state.embedder.embed(&body.text));
    }
    let memory = state.with_store(|store| store.insert_memory(&body))?;
    tracing::info!(memory_id = %memory.id, namespace = %memory.namespace_id, "created memory via API");
    Ok((StatusCode::CREATED, Json(memory)).into_response())
}

#[derive(Debug, Deserialize)]
pub struct ListMemoriesQuery {
    pub namespace: Option<String>,
    pub limit: Option<usize>,
    /// Rows to skip (pagination); the response carries `X-Total-Count`.
    pub offset: Option<usize>,
}

pub async fn list_memories(
    State(state): State<AppState>,
    Query(query): Query<ListMemoriesQuery>,
) -> Result<Response, HttpError> {
    // `page_limit` shares MAX_PAGE_LIMIT with the MCP `list_memories` tool;
    // see its docs for the negative-LIMIT hazard this clamp exists for.
    let limit = crate::page_limit(query.limit, 100);
    let offset = query.offset.unwrap_or(0);
    let (memories, total) = state.with_store(|store| {
        let total = store.count_memories(query.namespace.as_deref())?;
        let memories = store.list_memories(query.namespace.as_deref(), limit, offset)?;
        std::result::Result::Ok::<_, recall_core::RecallError>((memories, total))
    })?;
    Ok((
        [(
            axum::http::HeaderName::from_static("x-total-count"),
            total.to_string(),
        )],
        Json(memories),
    )
        .into_response())
}

pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, HttpError> {
    let deleted = state.with_store(|store| store.delete_memory(&id))?;
    if deleted {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(RecallError::MemoryNotFound(id).into())
    }
}

/// Request body for `PATCH /v1/memories/{id}` — every field optional.
#[derive(Debug, Deserialize)]
pub struct UpdateMemoryRequest {
    pub text: Option<String>,
    pub tags: Option<Vec<String>>,
    pub source: Option<String>,
    pub pinned: Option<bool>,
}

pub async fn update_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateMemoryRequest>,
) -> Result<Json<Memory>, HttpError> {
    if body.text.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(RecallError::InvalidInput("text must not be empty".into()).into());
    }
    let mut update = MemoryUpdate {
        text: body.text,
        tags: body.tags,
        source: body.source,
        pinned: body.pinned,
        embedding: None,
    };
    // Re-embed when the text changes (same policy as create).
    if let Some(text) = &update.text {
        update.embedding = Some(state.embedder.embed(text));
    }
    let memory = state.with_store(|store| store.update_memory(&id, &update))?;
    tracing::info!(memory_id = %memory.id, "updated memory via API");
    Ok(Json(memory))
}

/// Standard serde helper: distinguish absent (`None`) from explicit `null` (`Some(None)`).
fn deserialize_double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

/// Request body for `/v1/recall` — all knobs from ARCHITECTURE.md are tunable.
#[derive(Debug, Deserialize)]
pub struct RecallRequest {
    pub namespace: String,
    pub query: String,
    pub k: Option<usize>,
    /// `null` explicitly disables the recency term; absent keeps the 30-day default.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub tau_days: Option<Option<f64>>,
    pub weights: Option<Weights>,
    pub pinned_boost: Option<f64>,
    /// Restrict hits to memories carrying ALL of these tags.
    pub tags: Option<Vec<String>>,
}

pub async fn recall(
    State(state): State<AppState>,
    Json(body): Json<RecallRequest>,
) -> Result<Json<Vec<RecallHit>>, HttpError> {
    let mut params = RecallParams::default();
    if let Some(k) = body.k {
        params.k = k;
    }
    if let Some(tau) = body.tau_days {
        params.tau_days = tau;
    }
    if let Some(w) = body.weights {
        params.weights = w;
    }
    if let Some(boost) = body.pinned_boost {
        params.pinned_boost = boost;
    }
    if let Some(tags) = body.tags {
        params.tags = tags;
    }
    params.validate().map_err(HttpError)?;

    let query_embedding = state.embedder.embed(&body.query);
    let hits = state.with_store(|store| {
        store.recall(
            &body.namespace,
            &body.query,
            Some(&query_embedding),
            &params,
            recall_core::unix_now(),
        )
    })?;
    tracing::debug!(namespace = %body.namespace, hits = hits.len(), "recall served");
    Ok(Json(hits))
}

pub async fn stats(State(state): State<AppState>) -> Result<Response, HttpError> {
    let stats = state.with_store(|store| store.stats())?;
    Ok(Json(stats).into_response())
}

/// Prometheus text exposition (0.0.4): request counters and latencies,
/// embedding timings, and live store gauges. Protected by the API-key
/// middleware like the rest of `/v1/*`.
pub async fn metrics(State(state): State<AppState>) -> Response {
    let stats = state.with_store(|store| store.stats()).unwrap_or_else(|_| {
        // A failing store still reports the request metrics.
        recall_core::Stats {
            total_namespaces: 0,
            total_memories: 0,
            pinned_memories: 0,
            per_namespace: Vec::new(),
        }
    });
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(&stats),
    )
        .into_response()
}

// ---- Capture ingestion (D-018): auto-capture hook ----
// Types and the chunk/embed/commit pipeline live in `crate::capture`, shared
// verbatim with the MCP `capture` tool.

pub async fn capture(
    State(state): State<AppState>,
    Json(body): Json<crate::capture::CaptureRequest>,
) -> Result<Response, HttpError> {
    let prepared = crate::capture::prepare(state.embedder.as_ref(), &body)?;
    let captured = state.with_store(|store| crate::capture::commit(store, &prepared))?;
    tracing::info!(
        namespace = %prepared.namespace,
        memories = captured.len(),
        "captured transcript via API"
    );
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "namespace": prepared.namespace,
            "captured": captured.len(),
            "memories": captured,
        })),
    )
        .into_response())
}
