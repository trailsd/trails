//! Error types for trailsd.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum TrailsError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("app not found: {0}")]
    AppNotFound(uuid::Uuid),

    #[error("invalid state transition: {from} → {to}")]
    InvalidTransition { from: String, to: String },

    #[error("registration failed: {0}")]
    RegistrationFailed(String),

    #[error("protocol error: {0}")]
    Protocol(String),
}

impl IntoResponse for TrailsError {
    fn into_response(self) -> Response {
        let status = match &self {
            TrailsError::AppNotFound(_) => StatusCode::NOT_FOUND,
            TrailsError::InvalidTransition { .. } => StatusCode::CONFLICT,
            TrailsError::RegistrationFailed(_) => StatusCode::BAD_REQUEST,
            TrailsError::Protocol(_) => StatusCode::BAD_REQUEST,
            TrailsError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}
