//! The explainable hybrid scorer.
//!
//! Formula (ARCHITECTURE.md):
//! `total = w_k*norm_bm25 + w_v*cosine + w_r*exp(-delta_days/tau) + pinned_boost`

use crate::models::{ScoreBreakdown, Weights};

/// Raw per-candidate inputs the store collects before scoring.
#[derive(Debug, Clone, Copy)]
pub struct RawScore {
    /// BM25 already normalized to `[0,1]` against the candidate set (0 if no keyword match).
    pub bm25_normalized: f64,
    /// Cosine similarity between the query and memory embeddings.
    pub cosine: f64,
    pub created_at: i64,
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct HybridScorer {
    pub weights: Weights,
    /// `tau` in days for `exp(-days/tau)`; `None` disables the recency term entirely.
    pub tau_days: Option<f64>,
    pub pinned_boost: f64,
}

impl Default for HybridScorer {
    fn default() -> Self {
        Self {
            weights: Weights::default(),
            tau_days: Some(30.0),
            pinned_boost: 0.2,
        }
    }
}

impl HybridScorer {
    /// Compute the full breakdown for one candidate at time `now` (unix seconds).
    /// Deterministic: same inputs always produce bit-identical outputs.
    pub fn score(&self, raw: RawScore, now: i64) -> ScoreBreakdown {
        let bm25 = raw.bm25_normalized.clamp(0.0, 1.0);
        let vector = raw.cosine;
        let recency = match self.tau_days {
            Some(tau) if tau > 0.0 => {
                let delta_days = ((now - raw.created_at).max(0)) as f64 / 86_400.0;
                (-delta_days / tau).exp()
            }
            _ => 0.0,
        };
        let pinned_boost = if raw.pinned { self.pinned_boost } else { 0.0 };
        let total = self
            .weights
            .bm25
            .mul_add(bm25, self.weights.vector * vector)
            + self.weights.recency * recency
            + pinned_boost;
        ScoreBreakdown {
            bm25,
            vector,
            recency,
            pinned_boost,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scorer() -> HybridScorer {
        HybridScorer::default()
    }

    const NOW: i64 = 1_786_000_000; // arbitrary fixed "today"
    const DAY: i64 = 86_400;

    #[test]
    fn perfect_fresh_pinned_candidate() {
        let b = scorer().score(
            RawScore {
                bm25_normalized: 1.0,
                cosine: 1.0,
                created_at: NOW,
                pinned: true,
            },
            NOW,
        );
        assert!((b.bm25 - 1.0).abs() < 1e-9);
        assert!((b.vector - 1.0).abs() < 1e-9);
        assert!((b.recency - 1.0).abs() < 1e-9);
        assert!((b.pinned_boost - 0.2).abs() < 1e-9);
        let expected = 0.45 + 0.45 + 0.10 + 0.2;
        assert!((b.total - expected).abs() < 1e-9);
    }

    #[test]
    fn no_signal_candidate_scores_zero() {
        let mut s = scorer();
        // Disable decay so the only inputs are keyword/vector/pinned — all zero here.
        s.tau_days = None;
        let b = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert_eq!(b.total, 0.0);
        assert_eq!(b.pinned_boost, 0.0);
        assert_eq!(b.recency, 0.0);
    }

    #[test]
    fn recency_decays_with_age_and_clamps_future_timestamps() {
        let s = scorer();
        let fresh = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert!((fresh.recency - 1.0).abs() < 1e-9);

        let tau = 30.0f64;
        let aged = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW - DAY,
                pinned: false,
            },
            NOW,
        );
        assert!((aged.recency - (-1.0 / tau).exp()).abs() < 1e-9);

        let ten_days = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW - 10 * DAY,
                pinned: false,
            },
            NOW,
        );
        assert!((ten_days.recency - (-10.0 / tau).exp()).abs() < 1e-9);
        assert!(aged.recency > ten_days.recency);

        // Clock skew (created in the future) is clamped to age 0.
        let future = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW + DAY,
                pinned: false,
            },
            NOW,
        );
        assert!((future.recency - 1.0).abs() < 1e-9);
    }

    #[test]
    fn decay_disabled_and_bad_tau_zero_the_recency_term() {
        let mut s = scorer();
        s.tau_days = None;
        let b = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW - 365 * DAY,
                pinned: false,
            },
            NOW,
        );
        assert_eq!(b.recency, 0.0);
        assert_eq!(b.total, 0.0);

        // A validated-elsewhere tau <= 0 never multiplies the term.
        s.tau_days = Some(0.0);
        let b = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.0,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert_eq!(b.recency, 0.0);
    }

    #[test]
    fn pinned_boost_only_applies_to_pinned() {
        let s = scorer();
        let base = || RawScore {
            bm25_normalized: 1.0,
            cosine: 0.0,
            created_at: NOW,
            pinned: false,
        };
        let pinned = RawScore {
            pinned: true,
            ..base()
        };
        let unpinned_total = s.score(base(), NOW).total;
        let pinned_total = s.score(pinned, NOW).total;
        assert!((pinned_total - unpinned_total - 0.2).abs() < 1e-9);
    }

    #[test]
    fn custom_weights_are_honored() {
        let s = HybridScorer {
            weights: Weights {
                bm25: 1.0,
                vector: 0.0,
                recency: 0.0,
            },
            tau_days: Some(30.0),
            pinned_boost: 0.0,
        };
        let b = s.score(
            RawScore {
                bm25_normalized: 0.5,
                cosine: 0.9,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert!((b.total - 0.5).abs() < 1e-9);
    }

    #[test]
    fn bm25_input_is_clamped_into_unit_range() {
        let s = scorer();
        let over = s.score(
            RawScore {
                bm25_normalized: 5.0,
                cosine: 0.0,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert!((over.bm25 - 1.0).abs() < 1e-9);
        let under = s.score(
            RawScore {
                bm25_normalized: -2.0,
                cosine: 0.0,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert!((under.bm25 - 0.0).abs() < 1e-9);
    }

    #[test]
    fn negative_cosine_is_kept_and_lowers_total() {
        let s = scorer();
        let pos = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: 0.5,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        let neg = s.score(
            RawScore {
                bm25_normalized: 0.0,
                cosine: -0.5,
                created_at: NOW,
                pinned: false,
            },
            NOW,
        );
        assert!(neg.total < pos.total);
        assert!((neg.vector + 0.5).abs() < 1e-9);
    }

    #[test]
    fn deterministic_bitwise() {
        let raw = RawScore {
            bm25_normalized: 0.7,
            cosine: 0.3,
            created_at: 1_700_000_000,
            pinned: true,
        };
        let b1 = scorer().score(raw, NOW);
        let b2 = scorer().score(raw, NOW);
        assert_eq!(b1, b2);
    }
}
