//! HTTP download of plan PDFs, with content-type and size validation.
//!
//! The URL is not re-checked here: every plan has already been through
//! `config::validate_plan_url` while `plans.yaml` was read.

use axum::http::StatusCode;
use bytes::{Bytes, BytesMut};
use reqwest::Client;

use crate::errors::PlanError;

/// Upper bound on a plan PDF; the real files are a few hundred KB.
const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;

/// Maps a transport-level failure to a plan fault. Reached from both `send` and
/// the body reads; `e` says which, and whether it was a timeout.
fn transport_error(url: &str, e: reqwest::Error) -> PlanError {
    PlanError::failed(format!("{url}: {e}"))
}

pub async fn download_pdf(client: &Client, url: &str) -> Result<Bytes, PlanError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| transport_error(url, e))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        tracing::debug!(url, "plan PDF returned 404, skipping this plan");
        return Err(PlanError::Retired(url.to_string()));
    }
    if !status.is_success() {
        return Err(PlanError::failed(format!("HTTP {status} fetching {url}")));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !content_type.starts_with("application/pdf") {
        return Err(PlanError::failed(format!(
            "{url} responded with content-type {content_type:?}, expected application/pdf"
        )));
    }

    // Pre-check for servers that advertise the length.
    if let Some(len) = response.content_length()
        && len > MAX_PDF_BYTES as u64
    {
        return Err(PlanError::failed(format!(
            "{url} advertises {len} bytes, limit is {MAX_PDF_BYTES}"
        )));
    }

    // Content-Length can lie or be absent (chunked), so the cap is enforced
    // again while reading. Not pre-sized from it, for the same reason.
    let mut buf = BytesMut::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| transport_error(url, e))?
    {
        if buf.len() + chunk.len() > MAX_PDF_BYTES {
            return Err(PlanError::failed(format!(
                "{url} exceeds the {MAX_PDF_BYTES} byte limit"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}
