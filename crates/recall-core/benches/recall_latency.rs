//! Criterion benchmark for the recall hot path (PRD success metric:
//! p95 `recall` < 25 ms on 10k memories).
//!
//! Run with `cargo bench -p recall-core` (criterion always uses the release
//! profile). The exact p50/p95/p99 numbers against the same fixture come from
//! `tests/p95_latency.rs` (`cargo test -p recall-core --release --test
//! p95_latency -- --ignored --nocapture`).
//!
//! Fixture: one namespace, 10,000 memories with 256-dim HashEmbedder vectors
//! (the default embedder), which is the documented worst case for the
//! brute-force cosine scan (ADR-002, scan_cap = 10k).

use std::hint::black_box;
use std::sync::OnceLock;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use recall_core::{
    Embedder as _, HashEmbedder, NewMemory, RecallParams, SqliteStore, Store, shared, unix_now,
};

const CORPUS_SIZE: usize = 10_000;

fn fixture() -> &'static recall_core::SharedStore {
    static FIXTURE: OnceLock<recall_core::SharedStore> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bench.db");
        std::mem::forget(dir);
        let store = SqliteStore::open(path).expect("open bench db");
        let embedder = HashEmbedder::new_256();
        store.create_namespace("bench").expect("namespace");
        for i in 0..CORPUS_SIZE {
            let text = fixture_text(i);
            store
                .insert_memory(&NewMemory {
                    namespace: "bench".into(),
                    text: text.clone(),
                    tags: vec![format!("topic-{}", i % 20)],
                    source: Some("bench".into()),
                    pinned: i % 500 == 0,
                    created_at: Some(unix_now() - (i as i64 % 3600)),
                    id: None,
                    embedding: Some(embedder.embed(&text)),
                })
                .expect("insert bench memory");
        }
        shared(store)
    })
}

/// Deterministic, topic-rotating text so the corpus is non-degenerate for
/// both the FTS and the vector pass.
fn fixture_text(i: usize) -> String {
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
    let details = [
        "latency improved after the cache change",
        "rollout pending approval from the owner",
        "root cause was a misconfigured replica",
        "docs need a rewrite before the next audit",
        "follow up with the platform team next sprint",
    ];
    format!(
        "{} note {}: {} and {}",
        topics[i % topics.len()],
        i,
        details[i % details.len()],
        topics[(i * 7) % topics.len()]
    )
}

fn bench_recall(c: &mut Criterion) {
    let store = fixture();
    let embedder = HashEmbedder::new_256();
    let mut group = c.benchmark_group("recall_10k_memories");
    group.throughput(Throughput::Elements(1));

    // Typical agent query against the 10k fixture, hybrid scoring, k = 5.
    group.bench_function("hybrid_k5", |b| {
        b.iter(|| {
            let q = embedder.embed(black_box("incident review latency root cause"));
            let hits = store
                .lock()
                .expect("bench mutex")
                .recall(
                    "bench",
                    black_box("incident review latency root cause"),
                    Some(&q),
                    &RecallParams::default(),
                    unix_now(),
                )
                .expect("recall");
            black_box(hits.len())
        })
    });

    // Keyword-only variant isolates the FTS pass (no query vector).
    group.bench_function("keyword_only_k5", |b| {
        b.iter(|| {
            let hits = store
                .lock()
                .expect("bench mutex")
                .recall(
                    "bench",
                    black_box("release checklist follow up"),
                    None,
                    &RecallParams::default(),
                    unix_now(),
                )
                .expect("recall");
            black_box(hits.len())
        })
    });

    group.finish();
}

criterion_group!(benches, bench_recall);
criterion_main!(benches);
