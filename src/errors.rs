//! The two error types, split by whether anyone outside the process can see
//! them.
//!
//! [`AppError`] is what a request can be answered with. [`PlanError`] is what
//! reading a plan can fail with — startup only, never serialized. They live in
//! one file because the split between them is the point, and it is easier to
//! keep honest when both are in view: a new variant belongs here only if a
//! client can actually receive it.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

// ---------------------------------------------------------------------------
// What a request can be answered with
// ---------------------------------------------------------------------------

// This is the body `into_response` serializes *and* the schema `/docs`
// advertises, so the two cannot drift apart. It belongs next to `AppError`,
// which is the only thing that produces one. `detail` always carries
// `client_message`, never the internal `Display` text.
//
// The doc comment below is served at `/docs`, so it says what the body is and
// nothing about how it is produced.

/// Error response body returned on 4xx/5xx
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    pub detail: String,
}

/// Everything a request can be answered with other than success.
///
/// Both variants are the caller's own doing, which is the whole list: every
/// fault that involves the plan source happened at startup and is a
/// [`PlanError`], logged by `main` before it exits. The route is normalize,
/// reject `""`, look up — there is nothing else left to fail.
///
/// The `Display` text of each variant is the *internal* detail: `BadRequest`
/// carries axum's rejection text, which is only ever logged, never serialized.
/// Clients receive `client_message` instead, which is private for that reason.
///
/// Nothing a client can observe — neither the status code nor the message — may
/// reveal that this service fetches and parses PDFs from a third party. No
/// variant here carries a plan URL or library text, so this holds by
/// construction. `test_no_variant_discloses_the_data_source` asserts it over
/// every variant anyway: the invariant is about what may be *added*, and a
/// variant carrying upstream text is the first thing someone would reach for if
/// request-time fetching were introduced.
#[derive(Debug, Error)]
pub enum AppError {
    /// The request itself was malformed — a missing or undeserializable query
    /// parameter, or a `district` that is empty after normalization.
    #[error("{0}")]
    BadRequest(String),
    /// The district is in no plan. An observation rather than a fault: every
    /// plan was read at startup, so nothing was left unlooked-at.
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

    /// Client-facing message. Safe to serve: never contains library error text,
    /// and never mentions PDFs, plans or a third-party source.
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
        // Only 4xx can occur: the route has no 5xx path. The branch exists so
        // that a variant added with a 5xx status cannot be logged at DEBUG and
        // vanish under the default filter.
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), error = %self, "request failed");
        } else if status.is_client_error() {
            // The internal detail of a 4xx (e.g. axum's query-rejection text)
            // would otherwise be collected and then dropped unseen — it is not
            // serialized either. DEBUG keeps it off the default filter: a 4xx is
            // caller noise, not an operator signal.
            tracing::debug!(status = status.as_u16(), error = %self, "request rejected");
        }
        let body = ErrorDetail {
            detail: self.client_message().to_string(),
        };
        (status, Json(body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// What reading a plan can fail with
// ---------------------------------------------------------------------------

/// A plan that could not be read — at startup, and only there.
///
/// Separate from [`AppError`] because these never become a response.
/// `build_index` runs before the listener binds; a fault here is logged by
/// `main`, which then exits 1. Keeping them out of `AppError` is what lets
/// [`IntoResponse`] cover only the two outcomes a client can actually receive,
/// instead of carrying status codes and client messages for cases that cannot
/// reach a client.
///
/// Two variants, because the startup path asks exactly two questions: *is this
/// plan retired?* — which `build_index` answers by skipping it — and *what do I
/// print before giving up?* A finer split between the upstream faults
/// (unreachable, non-2xx, wrong content-type, timed out, oversized,
/// unparseable) would buy nothing: they are all handled identically, and what
/// tells them apart is the message, not the variant. That is also why their
/// tests assert on substrings rather than matching a variant.
///
/// The `Display` text is the **internal** detail — it carries plan URLs and raw
/// library error strings, and is only ever logged. Nothing here is serialized,
/// which is precisely why it may say "PDF" and name the source at all.
#[derive(Debug, Error)]
pub enum PlanError {
    /// The plan's PDF is gone upstream (HTTP 404).
    ///
    /// Expected at the turn of the year, when last year's plan goes offline
    /// while it is still listed in `plans.yaml`, and permanent until someone
    /// prunes the config. `build_index` matches on this to skip the plan with a
    /// WARN rather than refuse to start — the one variant that exists for
    /// control flow rather than for its message.
    #[error("plan PDF is gone upstream: {0}")]
    Retired(String),

    /// Anything else: unreachable source, non-2xx, not a PDF, timed out, over
    /// the size cap, or bytes that would not parse. All fatal at startup, all
    /// handled identically — the message is what distinguishes them.
    #[error("{0}")]
    Failed(String),
}

impl PlanError {
    /// Convenience for the many `format!`-and-wrap call sites.
    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed(detail.into())
    }
}
