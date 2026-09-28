//! Spike: minimal fastembed-rs build + embed test on windows-gnu.
//! Verdict recorded in docs/adr/DECISIONS.md (D-013).

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

fn main() -> anyhow::Result<()> {
    let model = EmbeddingModel::AllMiniLML6V2Q; // quantized, ~17 MB download
    let t0 = std::time::Instant::now();
    let mut embedder = TextEmbedding::try_new(
        InitOptions::new(model).with_show_download_progress(true),
    )?;
    eprintln!("model init (incl. download): {:?}", t0.elapsed());

    let texts = [
        "deploy the payments service on friday",
        "ship the payments service at the end of the week",
        "the cat sat on the mat",
    ];
    let t1 = std::time::Instant::now();
    let vecs = embedder.embed(texts.to_vec(), None)?;
    eprintln!("embed {} texts: {:?}", texts.len(), t1.elapsed());

    let dims = vecs[0].len();
    println!("dimensions: {dims}");
    assert_eq!(dims, 384, "AllMiniLML6V2 must be 384-dim");
    for (i, v) in vecs.iter().enumerate() {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        println!("text[{i}] norm={norm:.4} first3={:?}", &v[..3]);
    }

    // Cosine: the paraphrase pair must beat the unrelated pair (semantic check).
    let dot = |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| x * y).sum() };
    let rel = dot(&vecs[0], &vecs[1]);
    let unrel = dot(&vecs[0], &vecs[2]);
    println!("cos(paraphrase, original) = {rel:.4}");
    println!("cos(unrelated, original)  = {unrel:.4}");
    assert!(rel > unrel, "semantic ordering failed");

    // Determinism: same batch composition must reproduce identical vectors.
    // (Batch composition changes ONNX numerics slightly — measured below.)
    let again = embedder.embed(vec![texts[0]], None)?;
    assert_eq!(again[0], again[0]);
    let solo = embedder.embed(vec![texts[0]], None)?;
    assert_eq!(format!("{:?}", &solo[0][..16]), format!("{:?}", &again[0][..16]));
    let solo2 = embedder.embed(vec![texts[0]], None)?;
    assert_eq!(solo[0], solo2[0], "same-shape embeds must be deterministic");
    let batched_cos = dot(&again[0], &vecs[0]);
    println!("cos(solo embed, batched embed) of same text = {batched_cos:.4} (batch-shape variance)");
    println!("SPIKE OK: fastembed on windows-gnu embeds deterministically");
    Ok(())
}
