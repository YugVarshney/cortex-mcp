//! Local semantic embeddings via ONNX, behind the non-default `onnx` feature
//! (D-013). Uses `fastembed` 7.x on `ort` 2.0.0-rc.13 with ONNX Runtime loaded
//! at runtime (`ort-load-dynamic`): no ort prebuilt binaries ship for every
//! toolchain, but the official `onnxruntime.dll`/`.so` is C ABI and loads fine
//! via `ORT_DYLIB_PATH`.
//!
//! Model download: first construction fetches the model from Hugging Face
//! (rustls) into the cache dir (`.fastembed_cache/` under the process cwd by
//! default; override with [`OnnxEmbedder::with_cache_dir`]). Later runs are
//! fully offline.
//!
//! Determinism: embeddings are produced one text per call, so batch shape is
//! constant; identical inputs yield bit-identical vectors (verified by the
//! spike and the unit tests). Cross-batch ONNX variance does not apply.

use std::sync::Mutex;

use crate::embed::Embedder;
use crate::error::{RecallError, Result};

/// Models exposed by [`OnnxEmbedder`]. All are 384-dimensional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnnxModel {
    /// `all-MiniLM-L6-v2` (int8 quantized): ~23 MB download, fastembed's
    /// default general-purpose English model.
    AllMiniLmL6V2Q,
    /// `bge-small-en-v1.5` (int8 quantized): higher retrieval quality, larger
    /// download.
    BgeSmallEnV15Q,
}

impl OnnxModel {
    fn fastembed(self) -> fastembed::EmbeddingModel {
        match self {
            OnnxModel::AllMiniLmL6V2Q => fastembed::EmbeddingModel::AllMiniLML6V2Q,
            OnnxModel::BgeSmallEnV15Q => fastembed::EmbeddingModel::BGESmallENV15Q,
        }
    }

    /// Dimensionality of the model's output vectors.
    pub fn dimensions(self) -> usize {
        384
    }

    /// Stable embedder name recorded in configs and eval reports.
    pub fn as_str(self) -> &'static str {
        match self {
            OnnxModel::AllMiniLmL6V2Q => "onnx:all-minilm-l6-v2-q",
            OnnxModel::BgeSmallEnV15Q => "onnx:bge-small-en-v1.5-q",
        }
    }
}

/// [`Embedder`] backed by a local ONNX sentence-transformer (offline after the
/// first model download). Inference is serialized through a mutex; failures
/// are logged and yield a zero vector (the same "no signal" value the rest of
/// the system already handles for absent embeddings).
pub struct OnnxEmbedder {
    model: OnnxModel,
    inner: Mutex<fastembed::TextEmbedding>,
}

/// ONNX Runtime intra-op threads used by the production constructors
/// ([`OnnxEmbedder::new`], [`OnnxEmbedder::with_model`],
/// [`OnnxEmbedder::with_cache_dir`]): the machine's logical CPU count capped
/// at 4. Measured on the 6-vCPU reference VM (D-020): 4 threads beat the
/// runtime's use-all-CPUs default by 1.3–1.8× on capture-sized chunks and
/// never lost a run overall, while 1–2 threads were clearly worse on long
/// inputs. Capping (instead of using every CPU) also leaves headroom for the
/// store and the HTTP runtime on small multi-tenant hosts.
pub fn default_intra_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(4)
        .max(1)
}

impl OnnxEmbedder {
    /// Default model: `all-MiniLM-L6-v2` quantized (384 dims).
    pub fn new() -> Result<Self> {
        Self::with_model(OnnxModel::AllMiniLmL6V2Q)
    }

    /// Build with an explicit model; downloads it if not cached yet. Uses
    /// [`default_intra_threads`].
    pub fn with_model(model: OnnxModel) -> Result<Self> {
        let inner = fastembed::TextEmbedding::try_new(
            fastembed::TextInitOptions::new(model.fastembed())
                .with_intra_threads(default_intra_threads()),
        )
        .map_err(|e| RecallError::Embedder(format!("ONNX model init failed: {e}")))?;
        Ok(Self {
            model,
            inner: Mutex::new(inner),
        })
    }

    /// Build with an explicit model and cache directory for the download.
    /// Uses [`default_intra_threads`].
    pub fn with_cache_dir(model: OnnxModel, cache_dir: std::path::PathBuf) -> Result<Self> {
        Self::with_cache_dir_and_threads(model, cache_dir, Some(default_intra_threads()))
    }

    /// Build with an explicit model, cache directory, and ONNX Runtime
    /// intra-op thread count. `Some(n)` pins exactly `n` threads; `None`
    /// restores the runtime default (one thread per logical CPU — measured
    /// slower than the tuned default in D-020, kept as an explicit escape
    /// hatch and used by the probe harness).
    pub fn with_cache_dir_and_threads(
        model: OnnxModel,
        cache_dir: std::path::PathBuf,
        intra_threads: Option<usize>,
    ) -> Result<Self> {
        let mut options = fastembed::TextInitOptions::new(model.fastembed())
            .with_cache_dir(cache_dir)
            .with_show_download_progress(true);
        if let Some(threads) = intra_threads {
            options = options.with_intra_threads(threads);
        }
        let inner = fastembed::TextEmbedding::try_new(options)
            .map_err(|e| RecallError::Embedder(format!("ONNX model init failed: {e}")))?;
        Ok(Self {
            model,
            inner: Mutex::new(inner),
        })
    }
}

impl Embedder for OnnxEmbedder {
    fn dimensions(&self) -> usize {
        self.model.dimensions()
    }

    fn embedding_invertible(&self) -> bool {
        // Dense MiniLM-class vectors: embedding-inversion attacks (vec2text-
        // style) recover input text with usable fidelity (AR-003, D-014).
        true
    }

    fn name(&self) -> &'static str {
        self.model.as_str()
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let dim = self.model.dimensions();
        if text.trim().is_empty() {
            return vec![0.0; dim];
        }
        // One text per call keeps the batch shape constant (see module docs).
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match guard.embed(vec![text], None) {
            Ok(mut vecs) if vecs.len() == 1 && vecs[0].len() == dim => {
                vecs.pop().unwrap_or_default()
            }
            Ok(vecs) => {
                tracing::error!(
                    count = vecs.len(),
                    "ONNX embedder returned unexpected shape"
                );
                vec![0.0; dim]
            }
            Err(e) => {
                tracing::error!(error = %e, "ONNX embedding failed; yielding zero vector");
                vec![0.0; dim]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Model download happens once into the crate-local cache; these tests need
    // network on first run and are therefore feature-gated (`--features onnx`).
    fn embedder() -> OnnxEmbedder {
        OnnxEmbedder::with_cache_dir(
            OnnxModel::AllMiniLmL6V2Q,
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache"),
        )
        .expect("load ONNX model")
    }

    #[test]
    fn metadata_and_shapes() {
        let e = embedder();
        assert_eq!(e.dimensions(), 384);
        assert_eq!(e.name(), "onnx:all-minilm-l6-v2-q");
        let v = e.embed("deploy the payments service on friday");
        assert_eq!(v.len(), 384);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "L2 norm should be ~1, got {norm}"
        );
    }

    #[test]
    fn empty_text_is_zero_vector_without_inference() {
        let e = embedder();
        assert_eq!(e.embed(""), vec![0.0f32; 384]);
        assert_eq!(e.embed("   "), vec![0.0f32; 384]);
    }

    #[test]
    fn semantic_paraphrase_beats_unrelated_and_is_deterministic() {
        let e = embedder();
        let base = e.embed("deploy the payments service on friday");
        let paraphrase = e.embed("ship the payments service at the end of the week");
        let unrelated = e.embed("the cat sat on the mat");
        let dot = |a: &[f32], b: &[f32]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum()
        };
        assert!(
            dot(&base, &paraphrase) > dot(&base, &unrelated),
            "semantic ordering must hold"
        );
        // Same-shape determinism: identical text, fresh call, identical vector.
        let repeat = e.embed("deploy the payments service on friday");
        assert_eq!(base, repeat);
    }

    // Intra-op thread-count probe (ignored, release; D-020 evidence): builds
    // one ORT session per setting up front, then times single-text embedding
    // latency in *interleaved* rounds — each setting experiences the same
    // machine drift, so no ordering bias (this VM throttles visibly over a
    // long sequential run). Three representative shapes: tool-call-sized
    // line, one-sentence note, capture-default ~1000-char chunk.
    //
    //   ORT_DYLIB_PATH=<onnxruntime.dll> cargo test -p recall-core --release \
    //     --features onnx onnx_intra_op_thread_probe -- --ignored --nocapture
    #[test]
    #[ignore = "builds several ORT sessions; run with --ignored --nocapture (release mode)"]
    fn onnx_intra_op_thread_probe() {
        let cache =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache");
        let shapes: [(&str, String); 3] = [
            ("short", "deploy the payments service on friday".to_string()),
            (
                "note",
                "decided to keep the storage layer boring: one sqlite file, wal mode, \
                 no extensions, exact cosine scan until the numbers say otherwise"
                    .to_string(),
            ),
            ("chunk", "Release plan. ".repeat(100)), // ~1400 chars
        ];
        let settings: [(&str, Option<usize>); 4] = [
            ("default(=cpus)", None),
            ("1", Some(1)),
            ("2", Some(2)),
            ("4", Some(4)),
        ];

        // One session per setting; init cost is not what we measure. Building
        // every session also proves the `with_intra_threads` wiring end to end
        // (a bad knob fails the whole probe).
        let embedders: Vec<(&str, OnnxEmbedder)> = settings
            .iter()
            .map(|(label, threads)| {
                (
                    *label,
                    OnnxEmbedder::with_cache_dir_and_threads(
                        OnnxModel::AllMiniLmL6V2Q,
                        cache.clone(),
                        *threads,
                    )
                    .expect("session builds at every setting"),
                )
            })
            .collect();

        // Warm every session and tokenizer cache before timing.
        for (_, e) in &embedders {
            for (_, text) in &shapes {
                for _ in 0..3 {
                    assert_eq!(e.embed(text).len(), 384);
                }
            }
        }

        const ROUNDS: usize = 5;
        const ITERS: usize = 12;
        let mut totals = vec![0.0f64; settings.len() * shapes.len()];
        for _ in 0..ROUNDS {
            for (s_idx, (_, e)) in embedders.iter().enumerate() {
                for (shape_idx, (_, text)) in shapes.iter().enumerate() {
                    let start = std::time::Instant::now();
                    for _ in 0..ITERS {
                        assert_eq!(e.embed(text).len(), 384);
                    }
                    totals[s_idx * shapes.len() + shape_idx] +=
                        start.elapsed().as_secs_f64() * 1000.0 / ITERS as f64;
                }
            }
        }

        println!(
            "\n{:>15} | {:>9} | {:>9} | {:>9}  (ms/call, mean of {ROUNDS} interleaved rounds x {ITERS} iters)",
            "intra_threads", "short", "note", "chunk"
        );
        for (s_idx, (label, _)) in settings.iter().enumerate() {
            println!(
                "{:>15} | {:>9.3} | {:>9.3} | {:>9.3}",
                label,
                totals[s_idx * shapes.len()],
                totals[s_idx * shapes.len() + 1],
                totals[s_idx * shapes.len() + 2],
            );
        }
    }
}
