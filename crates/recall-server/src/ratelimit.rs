//! Token-bucket rate limiting for the HTTP transports.
//!
//! One bucket per presented API key (or one shared `anonymous` bucket when
//! auth is off / no key was presented). The limiter is layered outside the
//! auth middleware (AR-012), so wrong/missing-key requests are throttled by
//! the anonymous bucket — brute-force key guessing costs requests even with
//! auth enabled. Zero dependencies: the bucket math is a few lines and
//! windows-gnu-safe, and burst/refill are configured explicitly in
//! `ServerConfig`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Refill rate and capacity, validated at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimit {
    /// Sustained tokens added per second.
    pub rate: f64,
    /// Bucket capacity (maximum instant burst).
    pub burst: f64,
}

impl RateLimit {
    /// `rate` must be finite and > 0; `burst` finite and >= 1.
    pub fn validate(&self) -> Result<(), String> {
        if !self.rate.is_finite() || self.rate <= 0.0 {
            return Err(format!("rate must be finite and > 0, got {}", self.rate));
        }
        if !self.burst.is_finite() || self.burst < 1.0 {
            return Err(format!("burst must be finite and >= 1, got {}", self.burst));
        }
        if self.burst < self.rate {
            return Err(format!(
                "burst ({}) should not be smaller than the per-second rate ({})",
                self.burst, self.rate
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    limits: RateLimit,
    buckets: Mutex<HashMap<String, Bucket>>,
}

/// Upper bound on tracked keys; stale buckets are swept beyond this.
const MAX_BUCKETS: usize = 10_000;
/// Idle time after which a bucket becomes sweepable.
const BUCKET_IDLE: Duration = Duration::from_secs(3600);

/// Result of offering one request to the limiter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Admission {
    Admitted,
    Limited { retry_after_secs: u64 },
}

/// Axum middleware: per-key (or shared anonymous) token bucket in front of
/// every protected route. Requests rejected here never reach the handlers.
pub async fn enforce(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Some(limiter) = state.rate_limiter.clone() else {
        return next.run(request).await;
    };
    // Bucket by presented key; unauthenticated traffic shares one bucket so
    // key-guessing floods cannot rotate buckets.
    let key = crate::auth::presented_key(request.headers()).unwrap_or_else(|| "anonymous".into());
    match limiter.check(&key) {
        Admission::Admitted => next.run(request).await,
        Admission::Limited { retry_after_secs } => {
            state.metrics.observe_rate_limited();
            crate::error::too_many_requests(retry_after_secs)
        }
    }
}

impl RateLimiter {
    pub fn new(limits: RateLimit) -> Self {
        Self {
            limits,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Offer one request for `key`: consumes a token or reports how long the
    /// client should wait before retrying.
    pub fn check(&self, key: &str) -> Admission {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if buckets.len() > MAX_BUCKETS {
            buckets.retain(|_, b| now.duration_since(b.last_refill) < BUCKET_IDLE);
        }
        let limits = self.limits;
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: limits.burst,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * limits.rate).min(limits.burst);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Admission::Admitted
        } else {
            let wait = (1.0 - bucket.tokens) / limits.rate;
            Admission::Limited {
                retry_after_secs: wait.ceil().max(1.0) as u64,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(rate: f64, burst: f64) -> RateLimiter {
        RateLimiter::new(RateLimit { rate, burst })
    }

    #[test]
    fn validates_rate_and_burst() {
        assert!(
            RateLimit {
                rate: 50.0,
                burst: 100.0
            }
            .validate()
            .is_ok()
        );
        for (rate, burst) in [(0.0, 10.0), (-1.0, 10.0), (f64::NAN, 10.0)] {
            assert!(RateLimit { rate, burst }.validate().is_err(), "rate {rate}");
        }
        for (rate, burst) in [(10.0, 0.5), (10.0, f64::NAN), (10.0, 5.0)] {
            assert!(
                RateLimit { rate, burst }.validate().is_err(),
                "burst {burst}"
            );
        }
    }

    #[test]
    fn admits_burst_then_limits_with_retry_hint() {
        let l = limiter(0.0 + 1.0, 3.0); // 1 token/sec, burst 3
        for _ in 0..3 {
            assert_eq!(l.check("k"), Admission::Admitted);
        }
        let Admission::Limited { retry_after_secs } = l.check("k") else {
            panic!("4th request within the burst window must be limited");
        };
        assert!(retry_after_secs >= 1);
        // A different key still has its own full bucket.
        assert_eq!(l.check("other"), Admission::Admitted);
    }

    #[test]
    fn tokens_refill_over_time() {
        let l = limiter(1_000.0, 2.0);
        assert_eq!(l.check("k"), Admission::Admitted);
        assert_eq!(l.check("k"), Admission::Admitted);
        assert_ne!(l.check("k"), Admission::Admitted);
        std::thread::sleep(Duration::from_millis(12)); // ~12 tokens at 1000/s
        assert_eq!(l.check("k"), Admission::Admitted, "refill grants tokens");
    }

    #[test]
    fn refill_never_exceeds_burst() {
        let l = limiter(1.0, 2.0);
        assert_eq!(l.check("k"), Admission::Admitted);
        assert_eq!(l.check("k"), Admission::Admitted);
        assert_ne!(l.check("k"), Admission::Admitted);
        std::thread::sleep(Duration::from_millis(5));
        assert_ne!(
            l.check("k"),
            Admission::Admitted,
            "0.005 tokens is not enough"
        );
    }
}
