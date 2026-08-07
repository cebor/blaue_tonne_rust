//! HTTP download of plan PDFs, with content-type and size validation.
//!
//! The URL itself is not re-checked here: `config::validate_plan_url` rejects
//! anything unfetchable while `plans.yaml` is being read, which is the only way
//! a plan reaches this function.

use axum::http::StatusCode;
use bytes::{Bytes, BytesMut};
use reqwest::Client;

use crate::errors::PlanError;

/// Upper bound on a plan PDF. The real files are a few hundred KB; this only
/// exists so a misbehaving upstream cannot stream unbounded bytes into memory.
const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;

/// Maps a transport-level failure to a plan fault. The client timeout covers
/// the whole exchange, so this is reached from both `send` and the body reads;
/// `e` already says which of the two it was, and whether it was a timeout.
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
        // Expected at the turn of the year, when last year's plan goes offline
        // while it is still listed in plans.yaml. `build_index` matches on this
        // variant to skip the plan with a WARN instead of refusing to start.
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

    // Cheap pre-check: honest servers advertise the length.
    if let Some(len) = response.content_length()
        && len > MAX_PDF_BYTES as u64
    {
        return Err(PlanError::failed(format!(
            "{url} advertises {len} bytes, limit is {MAX_PDF_BYTES}"
        )));
    }

    // Content-Length can lie or be absent (chunked), so enforce while reading.
    // Deliberately not pre-sized from content_length: a bogus header would
    // otherwise drive a large allocation for a tiny body.
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
