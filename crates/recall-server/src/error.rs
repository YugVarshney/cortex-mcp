//! Mapping of core errors onto HTTP responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use recall_core::RecallError;

/// A core error paired with its HTTP mapping.
#[derive(Debug)]
pub struct HttpError(pub RecallError);

impl From<RecallError> for HttpError {
    fn from(e: RecallError) -> Self {
        HttpError(e)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            RecallError::NamespaceNotFound(_) => (StatusCode::NOT_FOUND, self.0.to_string()),
            RecallError::MemoryNotFound(_) => (StatusCode::NOT_FOUND, self.0.to_string()),
            RecallError::DuplicateNamespace(_) => (StatusCode::CONFLICT, self.0.to_string()),
            RecallError::InvalidInput(_) => (StatusCode::BAD_REQUEST, self.0.to_string()),
            // Everything else is a server fault; details go to logs, not clients.
            other => {
                tracing::error!(error = %other, "internal error handling request");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Rejection used by the auth middleware (structured like HttpError bodies).
pub fn unauthorized(detail: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Bearer")],
        Json(serde_json::json!({ "error": detail })),
    )
        .into_response()
}

/// Rejection used by the rate-limit middleware, with the standard
/// `Retry-After` hint in whole seconds.
pub fn too_many_requests(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("Retry-After", retry_after_secs.max(1).to_string())],
        Json(serde_json::json!({ "error": "rate limit exceeded" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn maps_errors_to_statuses() {
        let cases = [
            (
                RecallError::NamespaceNotFound("w".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                RecallError::MemoryNotFound("m".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                RecallError::DuplicateNamespace("w".into()),
                StatusCode::CONFLICT,
            ),
            (
                RecallError::InvalidInput("x".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                RecallError::Io(std::io::Error::other("db down")),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (err, expected) in cases {
            let res = HttpError(err).into_response();
            assert_eq!(res.status(), expected);
        }
        let res = unauthorized("nope").into_response();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
