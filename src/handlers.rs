use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::pdf_parser::normalize_district;
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
    // outcome only — never the data source behind it. The list is short because
    // the route is: everything that could fail has already failed at startup.
    responses(
        (status = 200, description = "Collection dates in RFC 3339 UTC format", body = Vec<String>),
        (status = 400, description = "Missing or invalid `district` query parameter", body = ErrorDetail),
        (status = 404, description = "District not found", body = ErrorDetail),
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

    // The index is keyed on the normalized name, so "Bad Aibling", "BadAibling"
    // and " Bad Aibling" all resolve to the one entry they name.
    let district = normalize_district(&params.district);

    // An all-whitespace name normalizes to "", which is not a district — telling
    // the caller their parameter is unusable is more accurate than reporting a
    // district they never named as missing.
    if district.is_empty() {
        return Err(AppError::BadRequest(
            "district must not be empty or whitespace-only".to_string(),
        ));
    }

    // Every plan was read at startup, so an absent key is an observation, not a
    // gap in what we looked at: this is the whole request path.
    let dates = state
        .index
        .lookup(&district)
        .ok_or(AppError::DistrictNotFound)?;

    Ok(Json(dates_to_iso(dates)))
}
