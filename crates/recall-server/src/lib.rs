//! recall-server — Axum HTTP API and MCP transports around the pure core.

// Curated clippy opt-ins (docs/style-guides/rust-tooling-conventions.md).
#![warn(clippy::dbg_macro, clippy::todo, clippy::unimplemented)]

pub mod api;
pub mod auth;
pub mod capture;
pub mod error;
pub mod guard;
pub mod mcp;
pub mod metrics;
pub mod openapi;
pub mod ratelimit;
pub mod state;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderName, Method};
use axum::routing::{delete, get, post};
use recall_core::HashEmbedder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::metrics::{Metrics, TimedEmbedder};
use crate::state::AppState;

/// Default request-body cap: memory texts are short by design; captures are
/// chunked server-side. 1 MiB leaves generous headroom for bulk imports that
/// bring their own vectors.
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Upper bound for caller-supplied pagination limits on every surface (REST
/// `GET /v1/memories` and the MCP `list_memories` tool). Without it, a
/// `usize` limit near `usize::MAX` would wrap negative through the store's
/// `i64` `LIMIT` bind and SQLite's "negative = unlimited" rule would
/// materialize an entire namespace into one response — a memory-exhaustion
/// DoS and an unbounded dump into an agent's context (AR-004).
pub const MAX_PAGE_LIMIT: usize = 10_000;

/// Apply the shared pagination clamp: caller limit (or the surface default)
/// capped at [`MAX_PAGE_LIMIT`].
pub fn page_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).min(MAX_PAGE_LIMIT)
}

/// Server settings supplied by the binary (CLI `serve`).
#[derive(Clone)]
pub struct ServerConfig {
    /// When set, all `/v1/*`, `/metrics` and `/mcp` requests must present this
    /// key via `Authorization: Bearer <key>` or `X-API-Key: <key>`. Unset =
    /// open access (local-first default; see README security notes).
    pub api_key: Option<String>,
    /// Directory of the built web UI to serve at `/` (production e2e setup).
    pub web_dir: Option<std::path::PathBuf>,
    /// When set, protected routes get per-key token-bucket rate limiting.
    pub rate_limit: Option<ratelimit::RateLimit>,
    /// Maximum accepted request body size in bytes.
    pub body_limit_bytes: usize,
    /// Embedder used for every server-side embed (REST, MCP, capture). The
    /// CLI resolves `--embedder` / env / config-file into this; the default
    /// is the offline `HashEmbedder`.
    pub embedder: Arc<dyn recall_core::Embedder>,
    /// Browser origins allowed to call the API cross-origin (AR-001 opt-in,
    /// CLI `--allow-origin`). Empty keeps the same-origin default: no CORS
    /// headers are emitted and foreign `Origin` requests are rejected.
    pub allowed_origins: Vec<String>,
    /// Extra `Host` values the guard accepts besides loopback hosts (AR-001
    /// opt-in, CLI `--allow-host`) for non-loopback bindings.
    pub allowed_hosts: Vec<String>,
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Arc<dyn Embedder>` has no Debug; the embedder's name is the
        // stable, useful part for logs. The API key is redacted: a Debug
        // of the config must never be the thing that leaks it into a log.
        f.debug_struct("ServerConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("web_dir", &self.web_dir)
            .field("rate_limit", &self.rate_limit)
            .field("body_limit_bytes", &self.body_limit_bytes)
            .field("embedder", &self.embedder.name())
            .field("allowed_origins", &self.allowed_origins)
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            web_dir: None,
            rate_limit: None,
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            embedder: Arc::new(HashEmbedder::new_256()),
            allowed_origins: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }
}

/// CORS policy for the configured opt-in origins (AR-001). The same-origin
/// default emits no CORS headers at all: the production web UI is served
/// same-origin and the Vite dev server proxies `/v1`, so nothing needs
/// cross-origin permissions unless the operator lists origins here.
fn cors_layer(allowed_origins: &[String]) -> CorsLayer {
    if allowed_origins.is_empty() {
        return CorsLayer::new();
    }
    let origins: Vec<_> = allowed_origins
        .iter()
        .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            CONTENT_TYPE,
            AUTHORIZATION,
            HeaderName::from_static("x-api-key"),
        ])
        .max_age(std::time::Duration::from_secs(600))
}

/// Build the complete application router: REST API, MCP streamable-HTTP,
/// metrics, optional static UI, CORS, tracing, body limit, rate limiting.
pub fn build_router(store: recall_core::SharedStore, config: &ServerConfig) -> Router {
    let metrics = Arc::new(Metrics::new());
    // Every embed (REST, MCP, capture) flows through the timed wrapper over
    // the configured embedder (hash by default; `--embedder` to switch).
    let embedder: Arc<dyn recall_core::Embedder> = Arc::new(TimedEmbedder {
        inner: config.embedder.clone(),
        metrics: metrics.clone(),
    });
    let mut state = AppState::new(store, embedder, config.api_key.clone());
    state.metrics = metrics;
    state.allowed_origins = config.allowed_origins.clone();
    state.allowed_hosts = config.allowed_hosts.clone();
    state.rate_limiter = config
        .rate_limit
        .map(ratelimit::RateLimiter::new)
        .map(Arc::new);

    // Probes and the OpenAPI document never require auth or rate limiting
    // (monitoring + spec discovery must survive load).
    let open: Router<AppState> = Router::new()
        .route("/healthz", get(api::healthz))
        .route("/openapi.json", get(api::openapi));

    let auth = axum::middleware::from_fn_with_state(state.clone(), auth::require_api_key);
    let limit = axum::middleware::from_fn_with_state(state.clone(), ratelimit::enforce);

    let protected: Router<AppState> = Router::new()
        .route(
            "/v1/namespaces",
            post(api::create_namespace).get(api::list_namespaces),
        )
        .route(
            "/v1/memories",
            post(api::create_memory).get(api::list_memories),
        )
        .route(
            "/v1/memories/{id}",
            delete(api::delete_memory).patch(api::update_memory),
        )
        .route("/v1/recall", post(api::recall))
        .route("/v1/capture", post(api::capture))
        .route("/v1/stats", get(api::stats))
        .route("/metrics", get(api::metrics))
        // The limiter wraps auth (runs before it, AR-012): wrong/missing keys
        // consume the anonymous bucket, so brute-force key guessing costs the
        // attacker requests even with auth enabled.
        .layer(auth.clone())
        .layer(limit.clone());

    // The MCP endpoint mounts as its own scoped router so the API-key layer
    // wraps the streamable-HTTP transport cleanly (including its session routes).
    let mcp: Router<AppState> = Router::new()
        .nest_service("/mcp", mcp::streamable_http_service(&state))
        .layer(auth)
        .layer(limit);

    let app = Router::new()
        .merge(open)
        .merge(protected)
        .merge(mcp)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::observe,
        ))
        .layer(DefaultBodyLimit::max(config.body_limit_bytes))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer(&config.allowed_origins))
        // Host/Origin guard outermost (AR-001): foreign hosts and cross-origin
        // browser requests are rejected before any other layer runs.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::enforce_same_origin,
        ))
        .with_state(state);

    match &config.web_dir {
        Some(dir) => app.fallback_service(
            tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true),
        ),
        None => app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_router(api_key: Option<String>) -> Router {
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        std::mem::forget(dir); // outlives the test; OS cleans the temp dir
        build_router(
            store,
            &ServerConfig {
                api_key,
                ..ServerConfig::default()
            },
        )
    }

    #[test]
    fn server_config_debug_redacts_the_api_key() {
        let config = ServerConfig {
            api_key: Some("super-secret".into()),
            ..ServerConfig::default()
        };
        let rendered = format!("{config:?}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(!rendered.contains("super-secret"), "{rendered}");
    }

    #[tokio::test]
    async fn healthz_is_open_even_with_api_key() {
        let app = test_router(Some("k".into()));
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/healthz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn body_over_the_limit_is_rejected_with_413() {
        let app = test_router(None);
        let big = "x".repeat(DEFAULT_BODY_LIMIT_BYTES + 1);
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/memories")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        "{{\"namespace\":\"w\",\"text\":\"{big}\"}}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn rate_limiter_rejects_floods_with_429_and_retry_after() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        let app = build_router(
            store,
            &ServerConfig {
                rate_limit: Some(ratelimit::RateLimit {
                    rate: 1.0,
                    burst: 2.0,
                }),
                ..ServerConfig::default()
            },
        );
        let request = || {
            Request::builder()
                .method("GET")
                .uri("/v1/stats")
                .header("x-api-key", "bucket-a")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let first = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), axum::http::StatusCode::OK);
        let second = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(second.status(), axum::http::StatusCode::OK);
        let third = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(third.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(
            third
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                >= 1,
            "Retry-After must carry a positive hint"
        );
        // A different key still has its own bucket.
        let other = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/stats")
                    .header("x-api-key", "bucket-b")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn failed_api_key_requests_are_rate_limited() {
        // AR-012 regression: with auth on, wrong/missing-key requests used to
        // be rejected by the auth layer before the limiter was consulted, so
        // brute-force key guessing cost nothing. The limiter now wraps auth:
        // after the anonymous bucket is drained, guesses get 429, not 401.
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        std::mem::forget(dir);
        let app = build_router(
            store,
            &ServerConfig {
                api_key: Some("secret".into()),
                rate_limit: Some(ratelimit::RateLimit {
                    rate: 1.0,
                    burst: 2.0,
                }),
                ..ServerConfig::default()
            },
        );
        let guess = || {
            Request::builder()
                .method("GET")
                .uri("/v1/stats")
                .header("x-api-key", "wrong-guess")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let first = app.clone().oneshot(guess()).await.unwrap();
        assert_eq!(first.status(), axum::http::StatusCode::UNAUTHORIZED);
        let second = app.clone().oneshot(guess()).await.unwrap();
        assert_eq!(second.status(), axum::http::StatusCode::UNAUTHORIZED);
        // Bucket drained: the third guess is throttled before auth even runs.
        let third = app.oneshot(guess()).await.unwrap();
        assert_eq!(
            third.status(),
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "wrong-key guessing must be throttled by the anonymous bucket"
        );
    }

    #[tokio::test]
    async fn metrics_endpoint_reports_seen_requests() {
        let app = test_router(None);
        let stats: axum::response::Response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/stats")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stats.status(), axum::http::StatusCode::OK);
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("recall_requests_total 1"),
            "metrics must count both requests:\n{text}"
        );
        assert!(text.contains("recall_request_duration_seconds"));
        assert!(text.contains("recall_store_memories 0"));
    }

    /// Embeds every text to one fixed vector, so a stored memory and its
    /// query share cosine exactly 1.0 — any other embedder would not.
    #[derive(Debug)]
    struct FixedEmbedder {
        vector: Vec<f32>,
    }
    impl recall_core::Embedder for FixedEmbedder {
        fn dimensions(&self) -> usize {
            self.vector.len()
        }
        fn name(&self) -> &'static str {
            "fixed-test"
        }
        fn embed(&self, _text: &str) -> Vec<f32> {
            self.vector.clone()
        }
    }

    #[tokio::test]
    async fn router_embeds_with_the_configured_embedder() {
        // Glue check for the `--embedder` selection: ServerConfig.embedder
        // must be the embedder REST/MCP/capture actually use.
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        std::mem::forget(dir); // outlives the test; OS cleans the temp dir
        let app = build_router(
            store,
            &ServerConfig {
                embedder: Arc::new(FixedEmbedder {
                    vector: vec![0.5f32; 8],
                }),
                ..ServerConfig::default()
            },
        );
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/namespaces")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"name":"w"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::CREATED);
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/memories")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"namespace":"w","text":"any text"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::CREATED);
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recall")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"namespace":"w","query":"any text"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(res.status().is_success());
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let hits: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let vector = hits[0]["breakdown"]["vector"]
            .as_f64()
            .expect("vector component");
        assert!(
            (vector - 1.0).abs() < 1e-5,
            "the server must embed with the configured embedder, got vector {vector}"
        );
    }

    // ---- AR-001 regressions: the loopback default is not web-page-reachable ----

    #[test]
    fn page_limit_clamps_wrap_sized_requests() {
        // The exact AR-004 hazard: usize::MAX as i64 wraps to -1, which SQLite
        // treats as an unlimited LIMIT.
        assert_eq!(page_limit(Some(usize::MAX), 50), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(Some(MAX_PAGE_LIMIT + 1), 50), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(Some(MAX_PAGE_LIMIT), 50), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(Some(7), 50), 7);
        assert_eq!(page_limit(None, 50), 50);
        assert_eq!(page_limit(None, 100), 100);
    }

    fn guarded_request(
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> Request<axum::body::Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn foreign_host_header_is_rejected_as_misdirected() {
        // DNS-rebinding signature: the browser resolves the attacker's
        // hostname to 127.0.0.1, so the request arrives with a foreign Host.
        let app = test_router(None);
        let res = app
            .oneshot(guarded_request(
                "GET",
                "/healthz",
                &[("host", "attacker.example")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn loopback_hosts_are_accepted_with_any_port() {
        let app = test_router(None);
        for host in [
            "127.0.0.1:8787",
            "localhost:8787",
            "[::1]:8787",
            "localhost",
        ] {
            let res = app
                .clone()
                .oneshot(guarded_request("GET", "/healthz", &[("host", host)]))
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                axum::http::StatusCode::OK,
                "loopback host {host} must be served"
            );
        }
    }

    #[tokio::test]
    async fn cross_origin_browser_request_is_rejected_by_default() {
        // The P1 exploit path: a visited web page scripting
        // fetch("http://127.0.0.1:8787/v1/stats") — the browser attaches
        // Origin and the guard must reject before anything is disclosed.
        let app = test_router(None);
        let res = app
            .oneshot(guarded_request(
                "GET",
                "/v1/stats",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "https://evil.example"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::FORBIDDEN);
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "the default must not hand out any CORS grant"
        );
    }

    #[tokio::test]
    async fn same_origin_request_is_served() {
        let app = test_router(None);
        let res = app
            .oneshot(guarded_request(
                "GET",
                "/v1/stats",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "http://127.0.0.1:8787"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn requests_without_host_or_origin_headers_are_served() {
        // Non-browser clients (curl, MCP clients, monitors) send neither
        // header; the tests' own oneshot requests rely on this too.
        let app = test_router(None);
        let res = app
            .oneshot(guarded_request("GET", "/v1/stats", &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn allow_origin_opt_in_grants_only_listed_origins() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        std::mem::forget(dir);
        let app = build_router(
            store,
            &ServerConfig {
                allowed_origins: vec!["https://agent.example".into()],
                ..ServerConfig::default()
            },
        );
        // Listed origin: served, with the origin echoed (not `*`).
        let res = app
            .clone()
            .oneshot(guarded_request(
                "GET",
                "/v1/stats",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "https://agent.example"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        assert_eq!(
            res.headers().get("access-control-allow-origin").unwrap(),
            "https://agent.example"
        );
        // Preflight for the listed origin succeeds.
        let preflight = app
            .clone()
            .oneshot(guarded_request(
                "OPTIONS",
                "/v1/recall",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "https://agent.example"),
                    ("access-control-request-method", "POST"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(preflight.status(), axum::http::StatusCode::OK);
        // Any other origin stays rejected.
        let rejected = app
            .oneshot(guarded_request(
                "GET",
                "/v1/stats",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "https://other.example"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allow_host_opt_in_extends_the_host_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            recall_core::shared(recall_core::SqliteStore::open(dir.path().join("t.db")).unwrap());
        std::mem::forget(dir);
        let app = build_router(
            store,
            &ServerConfig {
                allowed_hosts: vec!["mem.internal".into()],
                ..ServerConfig::default()
            },
        );
        let res = app
            .clone()
            .oneshot(guarded_request(
                "GET",
                "/healthz",
                &[("host", "mem.internal:8787")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        // A host that is neither loopback nor listed stays rejected.
        let res = app
            .oneshot(guarded_request(
                "GET",
                "/healthz",
                &[("host", "other.internal:8787")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn cors_preflight_without_opt_in_grants_nothing() {
        let app = test_router(None);
        let res = app
            .oneshot(guarded_request(
                "OPTIONS",
                "/v1/recall",
                &[
                    ("host", "127.0.0.1:8787"),
                    ("origin", "https://evil.example"),
                    ("access-control-request-method", "POST"),
                ],
            ))
            .await
            .unwrap();
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "no CORS grant may exist without --allow-origin"
        );
    }
}
