//! API-key authentication middleware for the HTTP transports.
//!
//! The 2026 MCP ecosystem has an auth gap (only a small minority of servers
//! implement any auth); Recall-MCP closes it for local deployments: set
//! `RECALL_MCP_API_KEY` and every `/v1/*` + `/mcp` request must present the key
//! as `Authorization: Bearer <key>` or `X-API-Key: <key>`.

use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::error::unauthorized;
use crate::state::AppState;

/// Extract the presented key from `Authorization: Bearer` or `X-API-Key`.
pub fn presented_key(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok())?;
    let (scheme, key) = auth.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Constant-time byte comparison (subtle `ConstantTimeEq`), so a forged key
/// cannot be probed byte-by-byte through timing. Differing lengths fail —
/// the leak of "wrong length" is negligible next to the local threat model.
fn keys_match(presented: &str, expected: &str) -> bool {
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

pub async fn require_api_key(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(expected) = &state.api_key else {
        return next.run(request).await;
    };
    match presented_key(request.headers()) {
        Some(presented) if keys_match(&presented, expected) => next.run(request).await,
        Some(_) => unauthorized("invalid API key"),
        None => unauthorized("missing API key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bearer_and_x_api_key() {
        let mut h = axum::http::HeaderMap::new();
        assert_eq!(presented_key(&h), None);
        h.insert("authorization", "Bearer abc".parse().unwrap());
        assert_eq!(presented_key(&h).as_deref(), Some("abc"));
        h.remove("authorization");
        h.insert("x-api-key", " xyz ".parse().unwrap());
        assert_eq!(presented_key(&h).as_deref(), Some("xyz"));
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(
            presented_key(&h).as_deref(),
            Some("xyz"),
            "x-api-key wins over non-bearer"
        );
        h.remove("x-api-key");
        assert_eq!(presented_key(&h), None, "non-bearer scheme is ignored");
        h.insert("authorization", "Bearer  ".parse().unwrap());
        assert_eq!(presented_key(&h), None, "blank bearer is ignored");
    }

    #[test]
    fn keys_match_is_exact_and_length_sensitive() {
        assert!(keys_match("secret", "secret"));
        assert!(!keys_match("secreT", "secret"), "case must matter");
        assert!(
            !keys_match("secret ", "secret"),
            "no trimming after extraction"
        );
        assert!(!keys_match("secr", "secret"), "shorter must fail");
        assert!(!keys_match("secret-longer", "secret"), "longer must fail");
        assert!(!keys_match("", "secret"), "empty must fail");
    }
}
