use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("District not found")]
    DistrictNotFound,
    #[error("Service temporarily unavailable")]
    ServiceUnavailable,
    #[error("{0}")]
    InvalidUrl(String),
    /// Upstream returned HTTP 404 for a plan's PDF. Soft-skipped in
    /// `lk_rosenheim_handler` (the next plan is tried instead).
    #[error("PDF not found at {0}")]
    PdfNotFound(String),
    #[error("{0}")]
    PdfError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::DistrictNotFound => StatusCode::NOT_FOUND,
            AppError::ServiceUnavailable => StatusCode::GATEWAY_TIMEOUT,
            AppError::InvalidUrl(_) => StatusCode::BAD_REQUEST,
            AppError::PdfNotFound(_) | AppError::PdfError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "detail": self.to_string() }))).into_response()
    }
}
