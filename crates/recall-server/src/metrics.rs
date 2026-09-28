//! In-process metrics exposed at `/metrics` in Prometheus text format.
//!
//! Deliberately dependency-free: a fixed-bucket histogram and counters under
//! one mutex render to the 0.0.4 text exposition format. Store gauges are
//! computed at scrape time so they never go stale.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Latency histogram buckets in seconds (recall's PRD budget is 25 ms).
const LATENCY_BUCKETS_SECS: [f64; 11] = [
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
];

#[derive(Default)]
struct Histogram {
    buckets: [u64; LATENCY_BUCKETS_SECS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, seconds: f64) {
        for (i, le) in LATENCY_BUCKETS_SECS.iter().enumerate() {
            if seconds <= *le {
                self.buckets[i] += 1;
            }
        }
        self.sum += seconds;
        self.count += 1;
    }
}

#[derive(Default)]
struct Inner {
    requests_total: u64,
    status_total: HashMap<u16, u64>,
    rate_limited_total: u64,
    route_latency: HashMap<String, Histogram>,
    embedding_calls: u64,
    embedding_seconds_sum: f64,
}

/// Shared metrics registry.
#[derive(Default)]
pub struct Metrics {
    inner: Mutex<Inner>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe_request(&self, route: &str, status: u16, elapsed: Duration) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.requests_total += 1;
        *inner.status_total.entry(status).or_default() += 1;
        inner
            .route_latency
            .entry(route.to_string())
            .or_default()
            .observe(elapsed.as_secs_f64());
    }

    pub fn observe_rate_limited(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.rate_limited_total += 1;
    }

    pub fn observe_embedding(&self, elapsed: Duration) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.embedding_calls += 1;
        inner.embedding_seconds_sum += elapsed.as_secs_f64();
    }

    /// Render the Prometheus text exposition (0.0.4), including live store
    /// gauges from `stats`.
    pub fn render(&self, stats: &recall_core::Stats) -> String {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = String::with_capacity(4096);

        out.push_str("# HELP recall_requests_total Total HTTP requests handled.\n");
        out.push_str("# TYPE recall_requests_total counter\n");
        out.push_str(&format!("recall_requests_total {}\n", inner.requests_total));

        out.push_str("# HELP recall_responses_total HTTP responses by status code.\n");
        out.push_str("# TYPE recall_responses_total counter\n");
        let mut statuses: Vec<_> = inner.status_total.iter().collect();
        statuses.sort();
        for (status, count) in statuses {
            out.push_str(&format!(
                "recall_responses_total{{status=\"{status}\"}} {count}\n"
            ));
        }

        out.push_str("# HELP recall_rate_limited_total Requests rejected by the rate limiter.\n");
        out.push_str("# TYPE recall_rate_limited_total counter\n");
        out.push_str(&format!(
            "recall_rate_limited_total {}\n",
            inner.rate_limited_total
        ));

        out.push_str("# HELP recall_request_duration_seconds Request latency by route.\n");
        out.push_str("# TYPE recall_request_duration_seconds histogram\n");
        let mut routes: Vec<_> = inner.route_latency.iter().collect();
        routes.sort_by(|a, b| a.0.cmp(b.0));
        for (route, hist) in routes {
            let label = format!("route=\"{}\"", escape_label(route));
            for (i, le) in LATENCY_BUCKETS_SECS.iter().enumerate() {
                out.push_str(&format!(
                    "recall_request_duration_seconds_bucket{{{label},le=\"{le}\"}} {}\n",
                    hist.buckets[i]
                ));
            }
            out.push_str(&format!(
                "recall_request_duration_seconds_bucket{{{label},le=\"+Inf\"}} {}\n",
                hist.count
            ));
            out.push_str(&format!(
                "recall_request_duration_seconds_sum{{{label}}} {}\n",
                hist.sum
            ));
            out.push_str(&format!(
                "recall_request_duration_seconds_count{{{label}}} {}\n",
                hist.count
            ));
        }

        out.push_str("# HELP recall_embeddings_total Embedding calls (REST, MCP, capture).\n");
        out.push_str("# TYPE recall_embeddings_total counter\n");
        out.push_str(&format!(
            "recall_embeddings_total {}\n",
            inner.embedding_calls
        ));
        out.push_str("# HELP recall_embedding_seconds_total Time spent computing embeddings.\n");
        out.push_str("# TYPE recall_embedding_seconds_total counter\n");
        out.push_str(&format!(
            "recall_embedding_seconds_total {}\n",
            inner.embedding_seconds_sum
        ));

        out.push_str("# HELP recall_store_namespaces Namespaces in the store.\n");
        out.push_str("# TYPE recall_store_namespaces gauge\n");
        out.push_str(&format!(
            "recall_store_namespaces {}\n",
            stats.total_namespaces
        ));
        out.push_str("# HELP recall_store_memories Memories in the store.\n");
        out.push_str("# TYPE recall_store_memories gauge\n");
        out.push_str(&format!("recall_store_memories {}\n", stats.total_memories));
        out.push_str("# HELP recall_store_pinned_memories Pinned memories in the store.\n");
        out.push_str("# TYPE recall_store_pinned_memories gauge\n");
        out.push_str(&format!(
            "recall_store_pinned_memories {}\n",
            stats.pinned_memories
        ));
        out
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Wrapper that times every embed call (REST, MCP and capture all embed
/// through `AppState::embedder`, so one wrapper sees them all).
pub(crate) struct TimedEmbedder {
    pub inner: Arc<dyn recall_core::Embedder>,
    pub metrics: std::sync::Arc<Metrics>,
}

impl recall_core::Embedder for TimedEmbedder {
    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn embedding_invertible(&self) -> bool {
        self.inner.embedding_invertible()
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let start = Instant::now();
        let vector = self.inner.embed(text);
        self.metrics.observe_embedding(start.elapsed());
        vector
    }
}

/// Axum middleware: observe every request (route label, status, latency).
pub async fn observe(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|matched| matched.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let start = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .observe_request(&route, response.status().as_u16(), start.elapsed());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use recall_core::Embedder as _;
    use recall_core::HashEmbedder;

    #[test]
    fn render_contains_counters_histograms_and_gauges() {
        let metrics = Metrics::new();
        metrics.observe_request("/v1/recall", 200, Duration::from_millis(3));
        metrics.observe_request("/v1/recall", 200, Duration::from_millis(30));
        metrics.observe_request("/healthz", 200, Duration::from_micros(200));
        metrics.observe_request("/v1/memories", 404, Duration::from_millis(1));
        metrics.observe_rate_limited();
        metrics.observe_embedding(Duration::from_micros(23));

        let stats = recall_core::Stats {
            total_namespaces: 2,
            total_memories: 10,
            pinned_memories: 1,
            per_namespace: vec![],
        };
        let text = metrics.render(&stats);
        for needle in [
            "recall_requests_total 4",
            "recall_responses_total{status=\"200\"} 3",
            "recall_responses_total{status=\"404\"} 1",
            "recall_rate_limited_total 1",
            "recall_request_duration_seconds_bucket{route=\"/v1/recall\",le=\"0.005\"} 1",
            "recall_request_duration_seconds_bucket{route=\"/v1/recall\",le=\"0.05\"} 2",
            "recall_request_duration_seconds_count{route=\"/v1/recall\"} 2",
            "recall_embeddings_total 1",
            "recall_store_memories 10",
            "recall_store_pinned_memories 1",
            "# TYPE recall_request_duration_seconds histogram",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    #[test]
    fn timed_embedder_wraps_and_counts() {
        let metrics = Arc::new(Metrics::new());
        let embedder = TimedEmbedder {
            inner: Arc::new(HashEmbedder::new_256()),
            metrics: metrics.clone(),
        };
        assert_eq!(embedder.dimensions(), 256);
        assert_eq!(embedder.name(), "hash-256");
        let v = embedder.embed("hello world");
        assert_eq!(v.len(), 256);
        let inner = metrics.inner.lock().unwrap();
        assert_eq!(inner.embedding_calls, 1);
    }
}
