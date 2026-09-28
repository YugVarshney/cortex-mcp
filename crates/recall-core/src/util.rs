//! Tokenization, hashing and vector math shared by the embedder and the store.

/// FNV-1a 64-bit hash of a byte slice — deterministic across platforms.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Lowercase alphanumeric tokens, order preserved, duplicates kept
/// (later weighted by feature hashing anyway).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Cosine similarity in `[-1, 1]`. Zero-length or mismatched vectors yield 0.0.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())).clamp(-1.0, 1.0)
}

/// `sqrt` of the sum of squared `f32`s widened to `f64` — the same norm term
/// [`cosine`] computes internally, hoisted out of hot per-row loops where the
/// query vector is constant across rows.
pub fn f32_norm_sqrt(a: &[f32]) -> f64 {
    a.iter()
        .fold(0.0f64, |acc, x| acc + f64::from(*x) * f64::from(*x))
        .sqrt()
}

/// [`cosine_f32_blob`] with the query norm precomputed via [`f32_norm_sqrt`].
/// Bit-identical to [`cosine`] (the norm product merely commutes).
pub fn cosine_f32_blob_with_query_norm(a: &[f32], norm_a_sqrt: f64, bytes: &[u8]) -> f64 {
    if a.is_empty() || bytes.is_empty() || a.len() * 4 != bytes.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, c) in a.iter().zip(bytes.as_chunks::<4>().0) {
        let (x, y) = (
            f64::from(*x),
            f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        );
        dot += x * y;
        norm_b += y * y;
    }
    if norm_a_sqrt == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a_sqrt * norm_b.sqrt())).clamp(-1.0, 1.0)
}

/// Cosine similarity between an `f32` query vector and a stored
/// little-endian `f32` BLOB — identical arithmetic to
/// `cosine(a, &blob_to_f32(bytes).unwrap())` but without materializing the
/// decoded vector, so hot recall scans never allocate per row.
/// Mismatched dimensions, non-multiple-of-4 lengths, and zero norms yield 0.0
/// exactly like [`cosine`].
pub fn cosine_f32_blob(a: &[f32], bytes: &[u8]) -> f64 {
    cosine_f32_blob_with_query_norm(a, f32_norm_sqrt(a), bytes)
}

/// [`cosine`] over a decoded `f32` slice with the query norm precomputed via
/// [`f32_norm_sqrt`]. Bit-identical to [`cosine`] (same accumulation order,
/// the norm product merely commutes) — this is the variant the packed
/// in-memory vector cache scans with, so cache hits and SQLite BLOB reads
/// produce identical breakdowns.
pub fn cosine_f32_with_query_norm(a: &[f32], norm_a_sqrt: f64, b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_b += y * y;
    }
    if norm_a_sqrt == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a_sqrt * norm_b.sqrt())).clamp(-1.0, 1.0)
}

/// Encode an f32 vector as little-endian bytes for SQLite BLOB storage.
pub fn f32_to_blob(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode a BLOB back into f32s. `None` when the byte length is not a multiple of 4.
pub fn blob_to_f32(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_is_deterministic_and_separated() {
        let a = fnv1a64(b"rust");
        assert_eq!(a, fnv1a64(b"rust"));
        assert_ne!(a, fnv1a64(b"rest"));
        // Known FNV-1a vector: "" -> offset basis.
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
    }

    #[test]
    fn tokenize_lowercases_and_drops_punctuation() {
        assert_eq!(
            tokenize("Hello, World! rust-tool"),
            vec!["hello", "world", "rust", "tool"]
        );
        assert!(tokenize("  ,, ").is_empty());
        assert!(tokenize("").is_empty());
        assert_eq!(tokenize("Ünïcode_Ω"), vec!["ünïcode", "ω"]);
    }

    #[test]
    fn cosine_handles_edge_shapes() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
        assert!((cosine(&[1.0, 0.0], &[0.0, 2.0])).abs() < 1e-9);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn blob_roundtrip() {
        let v = vec![0.25f32, -1.5, 3.0e10, f32::MIN];
        let blob = f32_to_blob(&v);
        assert_eq!(blob.len(), 16);
        assert_eq!(blob_to_f32(&blob).unwrap(), v);
        assert_eq!(blob_to_f32(&[1, 2, 3]), None);
        assert_eq!(blob_to_f32(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn cosine_f32_blob_matches_cosine_bit_for_bit() {
        // Deterministic pseudo-random vectors (LCG, no rand dependency).
        let mut state = 0x1234_5678_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as f32 / (u32::MAX >> 1) as f32 - 1.0
        };
        for dim in [1usize, 4, 8, 256] {
            let a: Vec<f32> = (0..dim).map(|_| next()).collect();
            let b: Vec<f32> = (0..dim).map(|_| next()).collect();
            let blob = f32_to_blob(&b);
            assert_eq!(
                cosine_f32_blob(&a, &blob),
                cosine(&a, &b),
                "dim {dim} must match the reference cosine"
            );
            // The hoisted-norm fast path must be bit-identical too.
            assert_eq!(
                cosine_f32_blob_with_query_norm(&a, f32_norm_sqrt(&a), &blob),
                cosine(&a, &b),
                "dim {dim} precomputed-norm variant"
            );
        }
        // Edge shapes mirror `cosine` exactly.
        assert_eq!(cosine_f32_blob(&[], &[]), 0.0);
        assert_eq!(cosine_f32_blob(&[1.0], &[1, 2, 3]), 0.0);
        assert_eq!(cosine_f32_blob(&[1.0, 2.0], &[1, 2, 3, 4, 5]), 0.0);
        assert_eq!(cosine_f32_blob(&[0.0, 0.0], &f32_to_blob(&[1.0, 1.0])), 0.0);
        let unit = f32_to_blob(&[1.0, 0.0]);
        assert!((cosine_f32_blob(&[1.0, 0.0], &unit) - 1.0).abs() < 1e-9);
        let neg = f32_to_blob(&[-1.0, 0.0]);
        assert!((cosine_f32_blob(&[1.0, 0.0], &neg) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_f32_slice_matches_cosine_bit_for_bit() {
        // The packed-vector cache path must be arithmetically indistinguishable
        // from the reference cosine: same accumulation order, same result bits.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as f32 / (u32::MAX >> 1) as f32 - 1.0
        };
        for dim in [1usize, 4, 8, 256] {
            let a: Vec<f32> = (0..dim).map(|_| next()).collect();
            let b: Vec<f32> = (0..dim).map(|_| next()).collect();
            assert_eq!(
                cosine_f32_with_query_norm(&a, f32_norm_sqrt(&a), &b),
                cosine(&a, &b),
                "dim {dim} must match the reference cosine"
            );
        }
        // Edge shapes mirror `cosine` exactly.
        assert_eq!(cosine_f32_with_query_norm(&[], 0.0, &[]), 0.0);
        assert_eq!(cosine_f32_with_query_norm(&[1.0], 1.0, &[1.0, 2.0]), 0.0);
        assert_eq!(
            cosine_f32_with_query_norm(&[0.0, 0.0], 0.0, &[1.0, 1.0]),
            0.0
        );
        assert!((cosine_f32_with_query_norm(&[1.0, 0.0], 1.0, &[1.0, 0.0]) - 1.0).abs() < 1e-9);
    }
}
