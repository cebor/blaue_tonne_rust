use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Application errors.
///
/// The `Display` text of each variant is the *internal* detail: it may contain
/// upstream URLs and raw library error strings, so it is only ever logged, never
/// serialized into a response. Clients receive [`AppError::client_message`].
///
/// Nothing a client can observe — neither the status code nor the message —
/// may reveal that this service fetches and parses PDFs from a third party.
/// That is why the upstream variants map to 503 rather than 502/504: a gateway
/// status is itself a statement about the architecture. Every variant below
/// therefore collapses into one of four outcomes the caller can act on
/// (bad request / not found / try later / we broke), and the distinctions that
/// matter operationally live in the log instead.
#[derive(Debug, Error)]
pub enum AppError {
    /// The request itself was malformed — a missing or undeserializable query
    /// parameter. The only variant that is the caller's fault.
    #[error("{0}")]
    BadRequest(String),
    #[error("District not found")]
    DistrictNotFound,
    /// Upstream did not answer within the HTTP client's timeout. Carries the
    /// URL so the log says *which* upstream hung.
    #[error("{0}")]
    ServiceUnavailable(String),
    /// Upstream or configuration fault: the configured plan URL is not a PDF,
    /// the upstream was unreachable, answered non-2xx, or sent something that
    /// is not a PDF. The plan URL comes from `plans.yaml`, never from the
    /// client, so this is never the caller's fault.
    #[error("{0}")]
    Upstream(String),
    /// Upstream returned HTTP 404 for a plan's PDF. Skipped per plan in
    /// `lk_rosenheim_handler` (the next plan is tried instead): plans are
    /// published yearly and an old plan going offline is expected, not a fault.
    #[error("PDF not found at {0}")]
    PdfNotFound(String),
    /// We received bytes, but parsing them as a PDF failed — our own problem.
    #[error("{0}")]
    PdfError(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::DistrictNotFound => StatusCode::NOT_FOUND,
            // Anything caused by the plan source — unreachable, non-2xx, not a
            // PDF, timed out — is "come back later" from the caller's side.
            AppError::ServiceUnavailable(_) | AppError::Upstream(_) | AppError::PdfNotFound(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            AppError::PdfError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Client-facing message. Safe to serve: never contains upstream URLs or
    /// library error text, and never mentions PDFs, plans or a third-party
    /// source — those stay in the log.
    fn client_message(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "Invalid or missing query parameter",
            AppError::DistrictNotFound => "District not found",
            AppError::ServiceUnavailable(_) | AppError::Upstream(_) | AppError::PdfNotFound(_) => {
                "Service temporarily unavailable, please try again later"
            }
            AppError::PdfError(_) => "Internal server error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), error = %self, "request failed");
        } else if status.is_client_error() {
            // The internal detail of a 4xx (e.g. axum's query-rejection text)
            // would otherwise be collected and then dropped unseen — it is not
            // serialized either. DEBUG keeps it off the default filter: a 4xx is
            // caller noise, not an operator signal.
            tracing::debug!(status = status.as_u16(), error = %self, "request rejected");
        }
        (status, Json(json!({ "detail": self.client_message() }))).into_response()
    }
}
