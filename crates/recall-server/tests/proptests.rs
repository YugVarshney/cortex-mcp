//! Property-based tests for the token-bucket rate limiter (AR-007). Time
//! passes between real `Instant`s, so the properties are stated to be robust
//! to small refills rather than assuming a frozen clock.

use proptest::prelude::*;
use recall_server::ratelimit::{Admission, RateLimit, RateLimiter};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn token_bucket_partition_and_burst_bound(
        rate in 0.01f64..1.0,
        burst in 1.0f64..50.0,
        draws in 1usize..60,
    ) {
        let limits = RateLimit { rate, burst };
        limits.validate().expect("generated limits are valid");
        let limiter = RateLimiter::new(limits);
        let mut admitted = 0usize;
        for _ in 0..draws {
            match limiter.check("key-a") {
                Admission::Admitted => admitted += 1,
                Admission::Limited { retry_after_secs } => {
                    prop_assert!(retry_after_secs >= 1, "Retry-After must be a positive hint");
                }
            }
        }
        // The bucket starts full (burst tokens); refills during the tiny test
        // window add well under one token at rate <= 1/s, so the admitted
        // count can only slightly exceed the integer burst capacity.
        prop_assert!(admitted <= burst as usize + 1,
            "admitted {admitted} exceeds burst capacity {burst}");
        // Buckets are independent: an untouched key still admits.
        prop_assert!(matches!(limiter.check("key-b"), Admission::Admitted));
    }
}
