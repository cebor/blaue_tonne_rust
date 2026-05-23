use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    DistrictNotFound,
    ServiceUnavailable,
    InvalidUrl(String),
    PdfError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::DistrictNotFound => (StatusCode::NOT_FOUND, "District not found".to_string()),
            AppError::ServiceUnavailable => (
                StatusCode::GATEWAY_TIMEOUT,
                "Service temporarily unavailable".to_string(),
            ),
            AppError::InvalidUrl(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::PdfError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "detail": message }))).into_response()
    }
}
