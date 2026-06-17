use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use bytes::Bytes;
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use dashmap::DashMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::pdf_parser::get_dates;
use crate::state::AppState;

/// Successful response from the health endpoint
#[derive(Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

/// Error response body returned on 4xx/5xx
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    pub detail: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DistrictQuery {
    /// Name of the district (Gemeinde), e.g. "Bad Aibling"
    pub district: String,
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    ),
    tag = "health"
)]
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse { status: "healthy".to_string() })
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
        return Err(AppError::InvalidUrl(
            "URL must point to a PDF file".to_string(),
        ));
    }

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AppError::ServiceUnavailable
            } else {
                AppError::PdfError(e.to_string())
            }
        })?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::PdfError(format!("PDF not found at {url}")));
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

fn dates_to_iso(dates: &[NaiveDate]) -> Vec<String> {
    dates
        .iter()
        .map(|d| {
            let dt: NaiveDateTime = d.and_hms_opt(0, 0, 0).unwrap();
            Utc.from_utc_datetime(&dt).to_rfc3339()
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/lk_rosenheim",
    params(
        ("district" = String, Query, description = "Name of the district (Gemeinde), e.g. \"Bad Aibling\"")
    ),
    responses(
        (status = 200, description = "Collection dates in RFC 3339 UTC format", body = Vec<String>),
        (status = 400, description = "Bad request (invalid URL or parameter)", body = ErrorDetail),
        (status = 404, description = "District not found", body = ErrorDetail),
        (status = 504, description = "PDF service unavailable (timeout)", body = ErrorDetail),
    ),
    tag = "dates"
)]
pub async fn lk_rosenheim_handler(
    State(state): State<AppState>,
    Query(params): Query<DistrictQuery>,
) -> Result<Json<Vec<String>>, AppError> {
    let district = &params.district;

    if let Some(cached) = state.dates_cache.get(district.as_str()) {
        return Ok(Json(dates_to_iso(&cached)));
    }

    let mut all_dates: Vec<NaiveDate> = Vec::new();

    for plan in state.plans.iter() {
        let pdf_bytes =
            match download_pdf(&state.http_client, &state.pdf_cache, &plan.url).await {
                Ok(b) => b,
                Err(AppError::PdfError(msg)) if msg.contains("not found") => continue,
                Err(e) => return Err(e),
            };

        match get_dates(&pdf_bytes, &plan.pages, district) {
            Ok(dates) => all_dates.extend(dates),
            Err(AppError::DistrictNotFound) => return Err(AppError::DistrictNotFound),
            Err(e) => return Err(e),
        }
    }

    if all_dates.is_empty() {
        return Err(AppError::DistrictNotFound);
    }

    state.dates_cache.insert(district.clone(), all_dates.clone());
    Ok(Json(dates_to_iso(&all_dates)))
}
