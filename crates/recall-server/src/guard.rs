//! Host/Origin request guard (adversarial review AR-001).
//!
//! The default install binds `127.0.0.1` with auth off. Without this guard any
//! web page the user visits could script `fetch("http://127.0.0.1:8787/...")`
//! from the browser: with a permissive CORS policy the response bodies were
//! fully readable cross-origin (every stored memory, namespace and stat), and
//! DNS rebinding could reach the server even without CORS. The guard enforces
//! a same-origin default:
//!
//! - **Host check** (DNS-rebinding defense): the `Host` header must be a
//!   loopback host (`localhost`, `127.0.0.1`, `::1`) or an explicitly allowed
//!   host (`--allow-host`). A rebound hostname is rejected with `421`.
//! - **Origin check** (cross-origin browser defense): when an `Origin` header
//!   is present — every browser cross-origin request carries one — its
//!   authority must match the request authority (same-origin) or an entry of
//!   `--allow-origin`. Anything else is rejected with `403`. `Origin: null`
//!   (sandboxed frames, some redirects) is rejected.
//!
//! Non-browser clients (curl, MCP clients, monitors) send no `Origin` and
//! pass; requests without a `Host` header (HTTP/2-style or test clients) pass
//! the Host check — DNS rebinding always requires a forged `Host`, so the
//! defense is unaffected. Cross-origin agent access stays available as an
//! explicit opt-in: `--allow-origin https://agent.example`.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Hosts the Host-header check always accepts (loopback, case-insensitive).
/// Ports are ignored: the check is about the hostname, not the bound port.
pub const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Hostname of a Host/Origin authority: lowercased, port stripped, IPv6
/// brackets removed (`[::1]:8787` and `::1` both become `::1`).
pub fn hostname(authority: &str) -> &str {
    let a = authority.trim();
    if let Some(rest) = a.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    // Bare IPv6 addresses carry several colons and no port; anything with a
    // single colon is host:port.
    if a.matches(':').count() > 1 {
        return a;
    }
    match a.rsplit_once(':') {
        Some((host, _port)) => host,
        None => a,
    }
}

/// Authority of an `Origin` header value (`scheme://authority`), lowercased.
/// Opaque origins (`null`) return `None` and are always rejected.
fn origin_authority(origin: &str) -> Option<&str> {
    let (_, authority) = origin.trim().split_once("://")?;
    Some(authority)
}

/// Same-origin or explicitly allowed: an `Origin` passes when its authority
/// equals the request authority (browser same-origin policy) or one of the
/// configured opt-in origins (case-insensitive).
fn origin_allowed(origin: &str, headers: &HeaderMap, allowed: &[String]) -> bool {
    let Some(authority) = origin_authority(origin) else {
        return false;
    };
    let authority = authority.to_ascii_lowercase();
    if let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        && authority == host.trim().to_ascii_lowercase()
    {
        return true;
    }
    allowed
        .iter()
        .any(|o| origin_authority(o).is_some_and(|a| a.to_ascii_lowercase() == authority))
}

/// Axum middleware: reject requests from foreign hosts (421) and cross-origin
/// browser requests (403) before any handler runs. Mounted outermost so
/// rejected requests never reach auth, rate limiting, or CORS.
pub async fn enforce_same_origin(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let headers = request.headers();
    if let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        let host = hostname(host);
        let loopback = LOOPBACK_HOSTS.iter().any(|h| host.eq_ignore_ascii_case(h));
        let listed = state
            .allowed_hosts
            .iter()
            .any(|h| hostname(h).eq_ignore_ascii_case(host));
        if !loopback && !listed {
            tracing::warn!(%host, "rejected request with foreign Host header");
            return misdirected_host();
        }
    }
    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        && !origin_allowed(origin, headers, &state.allowed_origins)
    {
        tracing::warn!(%origin, "rejected cross-origin request (same-origin default)");
        return forbidden_origin();
    }
    next.run(request).await
}

/// `421 Misdirected Request`: the Host header does not name this server
/// (DNS-rebinding signature).
fn misdirected_host() -> Response {
    (
        StatusCode::MISDIRECTED_REQUEST,
        Json(serde_json::json!({
            "error": "request Host is not allowed: this server only serves loopback hosts (opt out with --allow-host)"
        })),
    )
        .into_response()
}

/// `403 Forbidden`: a browser cross-origin request outside the same-origin
/// default and the `--allow-origin` opt-in list.
fn forbidden_origin() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "cross-origin request rejected: same-origin only by default (opt in with --allow-origin)"
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_strips_ports_and_brackets() {
        assert_eq!(hostname("127.0.0.1:8787"), "127.0.0.1");
        assert_eq!(hostname("localhost"), "localhost");
        assert_eq!(hostname("LOCALHOST:80"), "LOCALHOST"); // callers compare case-insensitively
        assert_eq!(hostname("[::1]:8787"), "::1");
        assert_eq!(hostname("[::1]"), "::1");
        assert_eq!(hostname("::1"), "::1");
        assert_eq!(hostname("evil.example:443"), "evil.example");
    }

    #[test]
    fn origin_authority_parses_and_rejects_opaque_origins() {
        assert_eq!(
            origin_authority("http://127.0.0.1:8787"),
            Some("127.0.0.1:8787")
        );
        assert_eq!(
            origin_authority("https://Agent.Example"),
            Some("Agent.Example")
        );
        assert_eq!(origin_authority("null"), None);
        assert_eq!(origin_authority(""), None);
    }

    #[test]
    fn origin_allowed_matches_same_host_then_list() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "127.0.0.1:8787".parse().unwrap());
        let allowed: Vec<String> = vec!["https://agent.example".into()];
        // Same-origin authority passes without any opt-in.
        assert!(origin_allowed("http://127.0.0.1:8787", &headers, &allowed));
        // Different port on the same host is cross-origin and not listed.
        assert!(!origin_allowed("http://127.0.0.1:9999", &headers, &allowed));
        // Listed origin passes.
        assert!(origin_allowed("https://agent.example", &headers, &allowed));
        assert!(origin_allowed("HTTPS://AGENT.EXAMPLE", &headers, &allowed));
        // Unlisted origin fails; opaque `null` always fails.
        assert!(!origin_allowed("https://evil.example", &headers, &allowed));
        assert!(!origin_allowed("null", &headers, &allowed));
    }
}
