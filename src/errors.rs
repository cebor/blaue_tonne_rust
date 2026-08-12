//! [`AppError`] is what a request can be answered with; [`PlanError`] is what
//! reading a plan can fail with (startup only, never serialized).

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

/// Error response body returned on 4xx/5xx
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    pub detail: String,
}

/// Everything a request can be answered with other than success.
///
/// Each variant's `Display` is the internal detail and is only ever logged;
/// clients receive [`AppError::client_message`] instead. Nothing a client can
/// observe may reveal that this service fetches and parses PDFs from a third
/// party — `test_no_variant_discloses_the_data_source` asserts it.
#[derive(Debug, Error)]
pub enum AppError {
    /// Missing or undeserializable query parameter, or a `district` that is
    /// empty after normalization. Carries axum's rejection text.
    #[error("{0}")]
    BadRequest(String),
    /// The district is in no plan.
    #[error("District not found")]
    DistrictNotFound,
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::DistrictNotFound => StatusCode::NOT_FOUND,
        }
    }

    /// Client-facing message. Never contains library error text, and never
    /// mentions PDFs, plans or a third-party source.
    fn client_message(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "Invalid or missing query parameter",
            AppError::DistrictNotFound => "District not found",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // No variant currently produces a 5xx; the branch keeps a future one off
        // the DEBUG level, where the default filter would hide it.
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), error = %self, "request failed");
        } else if status.is_client_error() {
            // DEBUG, not INFO: a 4xx is caller noise, not an operator signal.
            tracing::debug!(status = status.as_u16(), error = %self, "request rejected");
        }
        let body = ErrorDetail {
            detail: self.client_message().to_string(),
        };
        (status, Json(body)).into_response()
    }
}

/// A plan that could not be read, at startup. Never becomes a response: `main`
/// logs it and exits 1.
///
/// The `Display` text is the internal detail and carries plan URLs and raw
/// library error strings.
#[derive(Debug, Error)]
pub enum PlanError {
    /// The plan's PDF is gone upstream (HTTP 404). `build_index` matches on
    /// this to skip the plan with a WARN rather than refuse to start.
    #[error("plan PDF is gone upstream: {0}")]
    Retired(String),

    /// Anything else: unreachable source, non-2xx, not a PDF, timed out, over
    /// the size cap, or bytes that would not parse. All fatal at startup and
    /// handled identically — the message is what distinguishes them.
    #[error("{0}")]
    Failed(String),
}

impl PlanError {
    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed(detail.into())
    }
}
