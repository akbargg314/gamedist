//! RFC 7807 errors.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use gamedist_core::api::Problem;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub title: String,
    pub detail: String,
}

impl ApiError {
    pub fn new(status: StatusCode, title: impl Into<String>, detail: impl Into<String>) -> Self {
        ApiError {
            status,
            title: title.into(),
            detail: detail.into(),
        }
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad request", detail)
    }

    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized", detail)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", detail)
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not found", what)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", detail)
    }

    pub fn too_many_requests() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests",
            "slow down and retry in a minute",
        )
    }

    pub fn taken_down(reason: &str) -> Self {
        Self::new(
            StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS,
            "Taken down",
            format!("this game was taken down by the server operator: {reason}"),
        )
    }

    pub fn internal(context: &str, err: impl std::fmt::Display) -> Self {
        // Details go to the log, not to the client.
        tracing::error!("{context}: {err}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
            "",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let problem = Problem {
            kind: "about:blank".into(),
            title: self.title,
            status: self.status.as_u16(),
            detail: self.detail,
        };
        let body = serde_json::to_vec(&problem).unwrap_or_default();
        let mut response = (
            self.status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            body,
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::internal("database", e)
    }
}

impl From<gamedist_core::Error> for ApiError {
    fn from(e: gamedist_core::Error) -> Self {
        use gamedist_core::Error as E;
        match &e {
            E::Invalid { .. } | E::UnsafePath { .. } | E::Schema(_) | E::Json(_) => {
                ApiError::bad_request(e.to_string())
            }
            E::BadSignature => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Bad signature",
                "the signature does not verify against the developer key",
            ),
            E::ChunkMismatch { .. } => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Chunk hash mismatch",
                e.to_string(),
            ),
            E::MissingChunk(id) => ApiError::not_found(format!("chunk {id}")),
            _ => ApiError::internal("core", e),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
