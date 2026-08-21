use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    response::Redirect,
};
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::errors::{AppError, ErrorDetail};
use crate::pdf_parser::normalize_district;
use crate::state::AppState;

/// Successful response from the health endpoint
#[derive(Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
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

/// Sends the service root to the Swagger UI, the only human-facing page here.
pub async fn redirect_to_docs() -> Redirect {
    Redirect::temporary("/docs")
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
    // Served to clients at /docs: outcome only, never the data source behind it.
    responses(
        (status = 200, description = "Collection dates in RFC 3339 UTC format", body = Vec<String>),
        (status = 400, description = "Missing or invalid `district` query parameter", body = ErrorDetail),
        (status = 404, description = "District not found", body = ErrorDetail),
    ),
    tag = "dates"
)]
pub async fn lk_rosenheim_handler(
    State(state): State<AppState>,
    // A `Result` rather than a bare `Query`, so the rejection becomes an
    // `AppError`: axum's own rejection is a plain-text body, which would not
    // match the documented `ErrorDetail` shape.
    params: Result<Query<DistrictQuery>, QueryRejection>,
) -> Result<Json<Vec<String>>, AppError> {
    let Query(params) = params.map_err(|e| AppError::BadRequest(e.body_text()))?;

    // The index is keyed on the normalized name, so "Bad Aibling", "BadAibling"
    // and " Bad Aibling" all resolve to the one entry they name.
    let district = normalize_district(&params.district);

    // Empty and whitespace-only both normalize to "", which is not a district.
    // Without this they would fall through to a plain index miss and answer 404.
    if district.is_empty() {
        return Err(AppError::BadRequest(
            "district must not be empty or whitespace-only".to_string(),
        ));
    }

    let dates = state
        .index
        .lookup(&district)
        .ok_or(AppError::DistrictNotFound)?;

    Ok(Json(dates_to_iso(dates)))
}
