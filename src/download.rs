//! HTTP download of plan PDFs, with URL/content-type validation and caching.

use axum::http::StatusCode;
use bytes::Bytes;
use dashmap::DashMap;
use reqwest::Client;

use crate::errors::AppError;

pub async fn download_pdf(
    client: &Client,
    pdf_cache: &DashMap<String, Bytes>,
    url: &str,
) -> Result<Bytes, AppError> {
    if let Some(cached) = pdf_cache.get(url) {
        return Ok(cached.clone());
    }

    if !url.to_lowercase().ends_with(".pdf") {
        return Err(AppError::InvalidUrl(
            "URL must point to a PDF file".to_string(),
        ));
    }

    let response = client.get(url).send().await.map_err(|e| {
        if e.is_timeout() {
            AppError::ServiceUnavailable
        } else {
            AppError::PdfError(e.to_string())
        }
    })?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::PdfNotFound(url.to_string()));
    }
    if !status.is_success() {
        return Err(AppError::PdfError(format!("HTTP {status} fetching PDF")));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !content_type.starts_with("application/pdf") {
        return Err(AppError::InvalidUrl(
            "URL does not point to a valid PDF file".to_string(),
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::PdfError(e.to_string()))?;

    pdf_cache.insert(url.to_string(), bytes.clone());
    Ok(bytes)
}
