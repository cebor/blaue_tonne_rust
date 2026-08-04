use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::download::download_pdf;
use crate::errors::AppError;
use crate::pdf_parser::{get_dates, normalize_district};
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
    // These descriptions are served to clients at /docs, so they describe the
    // outcome only — never the data source behind it.
    responses(
        (status = 200, description = "Collection dates in RFC 3339 UTC format", body = Vec<String>),
        (status = 400, description = "Missing or invalid `district` query parameter", body = ErrorDetail),
        (status = 404, description = "District not found", body = ErrorDetail),
        (status = 500, description = "Internal server error", body = ErrorDetail),
        (status = 503, description = "Temporarily unable to answer, retry later", body = ErrorDetail),
    ),
    tag = "dates"
)]
pub async fn lk_rosenheim_handler(
    State(state): State<AppState>,
    // Taken as a `Result` rather than a bare `Query` so the rejection becomes an
    // `AppError` too. Axum's own rejection is a plain-text body, which would be
    // the one response that does not match the documented `ErrorDetail` shape.
    params: Result<Query<DistrictQuery>, QueryRejection>,
) -> Result<Json<Vec<String>>, AppError> {
    let Query(params) = params.map_err(|e| AppError::BadRequest(e.body_text()))?;

    // Matching is whitespace-insensitive, so the cache has to be keyed on the
    // normalized name — otherwise "Bad Aibling", "BadAibling" and " Bad Aibling"
    // all resolve to the same row but each allocate their own cache entry,
    // letting a caller grow the map without bound.
    let district = normalize_district(&params.district);

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

        // Parsing a PDF is CPU-bound and takes long enough to stall a runtime
        // worker, so it goes to the blocking pool. Note such a task is not
        // cancelled when the client disconnects — it runs to completion.
        // `district` is already normalized and normalization is idempotent, so
        // get_dates can take it as-is.
        let pages = plan.pages.clone();
        let key = district.clone();
        let parsed = tokio::task::spawn_blocking(move || get_dates(&pdf_bytes, &pages, &key))
            .await
            .map_err(|e| AppError::PdfError(format!("PDF parse task failed: {e}")))?;

        match parsed {
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

    state.dates_cache.insert(district, all_dates.clone());
    Ok(Json(dates_to_iso(&all_dates)))
}
