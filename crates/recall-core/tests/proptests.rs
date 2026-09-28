//! Property-based tests (adversarial review AR-007): exact invariants that
//! example-based tests only spot-check, checked over many generated inputs.

use proptest::prelude::*;
use recall_core::crypto::{StoreKey, text_context};
use recall_core::scorer::{HybridScorer, RawScore};
use recall_core::{capture, models::Weights, sqlite::build_fts_query, util::tokenize};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Crypto roundtrip (AR-007): `decrypt(ctx, encrypt(ctx, pt)) == pt` for
    /// arbitrary UTF-8, the blob always wears the encrypted shape, and a
    /// different AAD context always fails.
    #[test]
    fn crypto_roundtrip_and_aad_binding(
        plaintext in any::<String>(),
        context in "[a-z0-9:]{0,40}",
        other_context in proptest::string::string_regex("[a-z0-9:]{1,41}").unwrap(),
    ) {
        prop_assume!(context != other_context);
        let key = StoreKey::from_bytes(&[0x5au8; 32]).unwrap();
        let ct = key.encrypt(&text_context(&context), &plaintext);
        prop_assert!(ct.starts_with("enc:v1:"));
        prop_assert!(StoreKey::is_encrypted(&ct));
        prop_assert_eq!(key.decrypt(&text_context(&context), &ct).unwrap(), plaintext);
        prop_assert!(key.decrypt(&text_context(&other_context), &ct).is_err(),
            "ciphertext must not decrypt under a different AAD context");
    }

    /// FTS builder (AR-007): the output is always valid MATCH syntax — a
    /// double-quoted OR of terms with no embedded quote that could break the
    /// phrase or inject an operator — and deterministic. In keyed mode every
    /// term is a 32-hex digest and never equals its input token.
    #[test]
    fn fts_query_is_always_valid_match_syntax(
        query in any::<String>(),
        keyed in proptest::bool::ANY,
    ) {
        let key = StoreKey::from_bytes(&[0x42u8; 32]).unwrap();
        let built = build_fts_query(&query, keyed.then_some(&key));
        if query.trim().is_empty() || tokenize(&query).is_empty() {
            prop_assert!(built.is_none(), "tokenless input must yield no MATCH");
        } else {
            let q = built.unwrap();
            for term in q.split(" OR ") {
                prop_assert!(term.starts_with('"') && term.ends_with('"') && term.len() >= 2,
                    "each OR term must be a quoted phrase: {term:?} in {q:?}");
                prop_assert!(!term[1..term.len() - 1].contains('"'),
                    "no raw quote may survive inside a term: {term:?}");
            }
            let again = build_fts_query(&query, keyed.then_some(&key)).unwrap();
            prop_assert_eq!(&q, &again, "builder must be deterministic");
            if keyed {
                for term in q.split(" OR ") {
                    let digest = &term[1..term.len() - 1];
                    prop_assert!(digest.len() == 32
                        && digest.chars().all(|c| c.is_ascii_hexdigit()),
                        "keyed terms must be 128-bit hex digests: {digest:?}");
                }
            }
        }
    }

    /// Chunking (AR-007): every chunk respects `max_chars` (char-counted,
    /// unicode-safe), rejoining preserves all non-whitespace content, and the
    /// function is deterministic with empty → empty.
    #[test]
    fn chunking_preserves_content_within_the_cap(
        text in "\\PC{0,2000}",
        max_chars in 1usize..400,
    ) {
        let chunks = capture::chunk_text(&text, max_chars).unwrap();
        for chunk in &chunks {
            prop_assert!(chunk.chars().count() <= max_chars,
                "chunk of {} chars exceeds cap {max_chars}: {chunk:?}",
                chunk.chars().count());
        }
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let original: String = squash(&text);
        let rejoined: String = chunks.iter().map(|c| squash(c)).collect();
        prop_assert_eq!(original, rejoined, "non-whitespace content must survive");
        if text.trim().is_empty() {
            prop_assert!(chunks.is_empty(), "empty input must chunk to nothing");
        }
        let again = capture::chunk_text(&text, max_chars).unwrap();
        prop_assert_eq!(chunks, again, "chunking must be deterministic");
    }

    /// Scorer (AR-007): the total is exactly the weighted sum of the rendered
    /// components, each component stays in its documented bound, future
    /// timestamps clamp to recency 1.0, and scoring is deterministic.
    #[test]
    fn scorer_total_is_the_weighted_sum_of_components(
        w_bm25 in 0.0f64..1e6,
        w_vector in 0.0f64..1e6,
        w_recency in 0.0f64..1e6,
        pinned_boost in 0.0f64..1e6,
        bm25_raw in -10.0f64..10.0,
        cosine in -1.0f64..1.0,
        age_days in -100.0f64..(365.0 * 10.0),
        pinned in proptest::bool::ANY,
        decay_off in proptest::bool::ANY,
    ) {
        let scorer = HybridScorer {
            weights: Weights { bm25: w_bm25, vector: w_vector, recency: w_recency },
            tau_days: if decay_off { None } else { Some(30.0) },
            pinned_boost,
        };
        let now = 1_786_000_000i64;
        let created_at = now - (age_days * 86_400.0).round() as i64;
        let raw = RawScore { bm25_normalized: bm25_raw, cosine, created_at, pinned };
        let b = scorer.score(raw, now);
        // Component bounds.
        prop_assert!((0.0..=1.0).contains(&b.bm25), "bm25 must clamp to [0,1]: {}", b.bm25);
        prop_assert!((-1.0..=1.0).contains(&b.vector));
        prop_assert!((0.0..=1.0).contains(&b.recency), "recency must be in (0,1] or 0");
        prop_assert_eq!(b.pinned_boost, if pinned { pinned_boost } else { 0.0 });
        // The documented identity (same formula shape as the implementation).
        let expected = w_bm25
            .mul_add(b.bm25, w_vector * b.vector)
            + w_recency * b.recency
            + b.pinned_boost;
        prop_assert!((total_of(&b) - expected).abs() <= 1e-9 * expected.abs().max(1.0),
            "total must equal the weighted component sum");
        // Future timestamps clamp: recency is exactly 1.0 when decay is on
        // (no decay yet) and 0.0 when decay is disabled.
        let future = scorer.score(RawScore { created_at: now + 86_400, ..raw }, now);
        prop_assert_eq!(future.recency, if decay_off { 0.0 } else { 1.0 });
        // Deterministic (bit-identical).
        prop_assert_eq!(scorer.score(raw, now), b);
    }
}

fn total_of(b: &recall_core::ScoreBreakdown) -> f64 {
    b.total
}
