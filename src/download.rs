//! HTTP download of plan PDFs, with URL/content-type validation and caching.

use axum::http::StatusCode;
use bytes::Bytes;
use dashmap::DashMap;
use reqwest::Client;

use crate::errors::AppError;

/// Maps a transport-level failure to the right variant. The client timeout
/// covers the whole exchange, so a timeout can surface either from `send` or
/// from reading the body — both have to map to the timeout variant.
fn transport_error(url: &str, e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::ServiceUnavailable(format!("{url}: {e}"))
    } else {
        AppError::Upstream(format!("{url}: {e}"))
    }
}

pub async fn download_pdf(
    client: &Client,
    pdf_cache: &DashMap<String, Bytes>,
    url: &str,
) -> Result<Bytes, AppError> {
    if let Some(cached) = pdf_cache.get(url) {
        return Ok(cached.clone());
    }

    if !url.to_lowercase().ends_with(".pdf") {
        return Err(AppError::Upstream(format!(
            "configured plan URL does not end in .pdf: {url}"
        )));
    }

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| transport_error(url, e))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        // Expected at the turn of the year, when last year's plan goes offline
        // while it is still listed in plans.yaml. The handler soft-skips this.
        tracing::debug!(url, "plan PDF returned 404, skipping this plan");
        return Err(AppError::PdfNotFound(url.to_string()));
    }
    if !status.is_success() {
        return Err(AppError::Upstream(format!("HTTP {status} fetching {url}")));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !content_type.starts_with("application/pdf") {
        return Err(AppError::Upstream(format!(
            "{url} responded with content-type {content_type:?}, expected application/pdf"
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| transport_error(url, e))?;

    pdf_cache.insert(url.to_string(), bytes.clone());
    Ok(bytes)
}
