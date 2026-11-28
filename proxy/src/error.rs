//! Unified error type for the proxy, convertible into HTTP responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Application-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("upstream provider error: {0}")]
    Upstream(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Storage(_) | AppError::Memory(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let type_str = format!("{:?}", self)
            .split('(')
            .next()
            .unwrap_or("error")
            .to_lowercase();
        // Never leak internal detail to the client for SERVER-side errors: an
        // upstream reqwest error string carries the upstream host/URL, and
        // storage/internal messages carry implementation detail. Log the real
        // cause, return a generic message. Client-facing 4xx keep their message
        // (they are intentional caller guidance).
        let message = match &self {
            AppError::Upstream(detail) => {
                tracing::warn!(error = %detail, "upstream request failed");
                "Upstream request failed.".to_string()
            }
            AppError::Storage(detail) | AppError::Memory(detail) | AppError::Internal(detail) => {
                tracing::error!(error = %detail, "internal error");
                "Internal error.".to_string()
            }
            other => other.to_string(),
        };
        let body = Json(json!({ "error": { "type": type_str, "message": message } }));
        (status, body).into_response()
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Upstream(e.to_string())
    }
}

/// Convenience result alias.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(
            AppError::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::Unauthorized("x".into()).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AppError::Upstream("x".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            AppError::Storage("x".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppError::Memory("x".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppError::Internal("x".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn into_response_carries_status_and_type() {
        let resp = AppError::BadRequest("nope".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn server_errors_do_not_leak_internal_detail() {
        use axum::body::to_bytes;
        // An upstream error string carries the upstream host/URL — must NOT reach
        // the client body.
        let resp = AppError::Upstream(
            "error sending request for url (https://secret-upstream.internal:8443/v1/messages)"
                .into(),
        )
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(resp.into_body(), 65536).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("secret-upstream.internal"),
            "upstream host must not leak: {text}"
        );
        assert!(
            text.contains("Upstream request failed"),
            "generic message returned"
        );

        let resp =
            AppError::Internal("panic in module::secret_fn at line 42".into()).into_response();
        let body = to_bytes(resp.into_body(), 65536).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("secret_fn"),
            "internal detail must not leak: {text}"
        );

        // But a 4xx keeps its caller-facing message.
        let resp = AppError::BadRequest("missing field: model".into()).into_response();
        let body = to_bytes(resp.into_body(), 65536).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("missing field: model"));
    }

    #[test]
    fn display_messages() {
        assert_eq!(AppError::Memory("m".into()).to_string(), "memory error: m");
        assert_eq!(
            AppError::Storage("s".into()).to_string(),
            "storage error: s"
        );
    }
}
