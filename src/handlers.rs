use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::download::download_pdf;
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
    Json(HealthResponse {
        status: "healthy".to_string(),
    })
}

fn dates_to_iso(dates: &[NaiveDate]) -> Vec<String> {
    dates
        .iter()
        .map(|d| {
            let dt = d.and_time(NaiveTime::MIN);
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
        let pdf_bytes = match download_pdf(&state.http_client, &state.pdf_cache, &plan.url).await {
            Ok(b) => b,
            Err(AppError::PdfNotFound(_)) => continue,
            Err(e) => return Err(e),
        };

        match get_dates(&pdf_bytes, &plan.pages, district) {
            Ok(dates) => all_dates.extend(dates),
            // Not in this plan's PDF — try the remaining plans; the
            // final is_empty check turns "in none of them" into a 404.
            Err(AppError::DistrictNotFound) => continue,
            Err(e) => return Err(e),
        }
    }

    if all_dates.is_empty() {
        return Err(AppError::DistrictNotFound);
    }

    state
        .dates_cache
        .insert(district.clone(), all_dates.clone());
    Ok(Json(dates_to_iso(&all_dates)))
}
