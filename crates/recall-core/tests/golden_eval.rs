//! Golden-set retrieval eval (PRD success metric: hit-rate@5 >= 0.85).
//!
//! Seeds `evals/corpus.json` verbatim into a fresh SQLite store with the
//! default HashEmbedder, runs every query in `evals/golden.json` through the
//! hybrid recall with default parameters (k=5), and reports the actual
//! hit-rate. Any expected memory inside the top 5 counts as a hit.

use recall_core::{
    Embedder as _, HashEmbedder, NewMemory, RecallParams, SqliteStore, Store, unix_now,
};
use serde::Deserialize;
use std::path::PathBuf;

fn eval_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals")
}

#[derive(Deserialize)]
struct Corpus {
    namespaces: Vec<String>,
    memories: Vec<CorpusMemory>,
}

#[derive(Deserialize)]
struct CorpusMemory {
    id: String,
    namespace: String,
    text: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct Golden {
    hit_rate_target: f64,
    queries: Vec<GoldenQuery>,
}

#[derive(Deserialize)]
struct GoldenQuery {
    namespace: String,
    query: String,
    expect_id: String,
}

#[test]
fn golden_set_hit_rate_at_5_meets_target() {
    let corpus: Corpus = serde_json::from_str(
        &std::fs::read_to_string(eval_dir().join("corpus.json")).expect("read corpus.json"),
    )
    .expect("parse corpus.json");
    let golden: Golden = serde_json::from_str(
        &std::fs::read_to_string(eval_dir().join("golden.json")).expect("read golden.json"),
    )
    .expect("parse golden.json");

    // The PRD metric is defined over exactly 50 queries.
    assert_eq!(
        golden.queries.len(),
        50,
        "golden set must contain exactly 50 queries"
    );
    assert!(
        (golden.hit_rate_target - 0.85).abs() < f64::EPSILON,
        "gate must stay at the PRD-mandated 0.85"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(dir.path().join("eval.db")).expect("open store");
    let embedder = HashEmbedder::new_256();

    for namespace in &corpus.namespaces {
        store.create_namespace(namespace).expect("create namespace");
    }
    let now = unix_now();
    let mut expected_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for memory in &corpus.memories {
        store
            .insert_memory(&NewMemory {
                namespace: memory.namespace.clone(),
                text: memory.text.clone(),
                tags: memory.tags.clone(),
                source: Some("eval".to_string()),
                pinned: false,
                created_at: Some(now),
                id: Some(memory.id.clone()),
                embedding: Some(embedder.embed(&memory.text)),
            })
            .expect("insert corpus memory");
        expected_ids.insert(memory.id.clone());
    }
    assert_eq!(
        store.stats().expect("stats").total_memories,
        corpus.memories.len() as u64
    );

    let params = RecallParams::default();
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();
    for q in &golden.queries {
        assert!(
            expected_ids.contains(&q.expect_id),
            "query {:?} references unknown id {}",
            q.query,
            q.expect_id
        );
        let query_embedding = embedder.embed(&q.query);
        let results = store
            .recall(&q.namespace, &q.query, Some(&query_embedding), &params, now)
            .expect("recall");
        if results.iter().take(5).any(|h| h.memory.id == q.expect_id) {
            hits += 1;
        } else {
            misses.push(format!(
                "{} -> {} (wanted {})",
                q.namespace, q.query, q.expect_id
            ));
        }
    }

    let hit_rate = hits as f64 / golden.queries.len() as f64;
    println!(
        "golden-set hit-rate@5 = {hit_rate:.3} ({hits}/{}), target = {}",
        golden.queries.len(),
        golden.hit_rate_target
    );
    for m in &misses {
        println!("MISS {m}");
    }
    assert!(
        hit_rate >= golden.hit_rate_target,
        "hit-rate@5 {hit_rate:.3} below target {}; misses: {misses:?}",
        golden.hit_rate_target
    );
}

/// Same golden set, same 0.85 gate, but with the local ONNX embedder
/// (`--features onnx`). Requires onnxruntime.dll at runtime via
/// `ORT_DYLIB_PATH` on windows-gnu (D-013) and network for the first model
/// download; skipped in default builds where the feature is off.
#[cfg(feature = "onnx")]
#[test]
fn golden_set_hit_rate_at_5_with_onnx_embedder_meets_target() {
    use recall_core::{OnnxEmbedder, OnnxModel};

    let corpus: Corpus = serde_json::from_str(
        &std::fs::read_to_string(eval_dir().join("corpus.json")).expect("read corpus.json"),
    )
    .expect("parse corpus.json");
    let golden: Golden = serde_json::from_str(
        &std::fs::read_to_string(eval_dir().join("golden.json")).expect("read golden.json"),
    )
    .expect("parse golden.json");
    assert_eq!(golden.queries.len(), 50);

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(dir.path().join("eval-onnx.db")).expect("open store");
    let embedder = OnnxEmbedder::with_model(OnnxModel::AllMiniLmL6V2Q).expect("load ONNX model");

    for namespace in &corpus.namespaces {
        store.create_namespace(namespace).expect("create namespace");
    }
    let now = unix_now();
    for memory in &corpus.memories {
        store
            .insert_memory(&NewMemory {
                namespace: memory.namespace.clone(),
                text: memory.text.clone(),
                tags: memory.tags.clone(),
                source: Some("eval".to_string()),
                pinned: false,
                created_at: Some(now),
                id: Some(memory.id.clone()),
                embedding: Some(embedder.embed(&memory.text)),
            })
            .expect("insert corpus memory");
    }

    let params = RecallParams::default();
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();
    for q in &golden.queries {
        let query_embedding = embedder.embed(&q.query);
        let results = store
            .recall(&q.namespace, &q.query, Some(&query_embedding), &params, now)
            .expect("recall");
        if results.iter().take(5).any(|h| h.memory.id == q.expect_id) {
            hits += 1;
        } else {
            misses.push(format!(
                "{} -> {} (wanted {})",
                q.namespace, q.query, q.expect_id
            ));
        }
    }

    let hit_rate = hits as f64 / golden.queries.len() as f64;
    println!(
        "golden-set hit-rate@5 with {} = {hit_rate:.3} ({hits}/{}), target = {}",
        embedder.name(),
        golden.queries.len(),
        golden.hit_rate_target
    );
    for m in &misses {
        println!("MISS {m}");
    }
    assert!(
        hit_rate >= golden.hit_rate_target,
        "hit-rate@5 {hit_rate:.3} below target {}; misses: {misses:?}",
        golden.hit_rate_target
    );
}
