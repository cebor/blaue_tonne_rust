//! The request path: a lookup in the index built at startup, and nothing else.
//!
//! Everything that can fail while *reading* the plans fails before the server
//! accepts connections and is covered by `test_index.rs`. What is left here is
//! what a client can still observe: a hit, a miss, and a bad parameter.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::NaiveDate;
use tower::ServiceExt;

use blaue_tonne_rust::index::DistrictIndex;
use blaue_tonne_rust::{AppState, build_router};

mod common;
use common::{body_to_json, get, state_from_fixture};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_dates(district: &str) -> Option<Vec<NaiveDate>> {
    match district {
        "Kolbermoor" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 15).unwrap(),
        ]),
        "Bad Aibling" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
        ]),
        "Prien a. Chiemsee" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 25).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 25).unwrap(),
        ]),
        "Aschau" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 10).unwrap()]),
        "Bruckmühl 1" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 11).unwrap()]),
        "Feldkirchen 2" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 12).unwrap()]),
        "Raubling 3" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 13).unwrap()]),
        _ => None,
    }
}

/// A state holding exactly the given districts, without touching a PDF.
fn state_with_dates<const N: usize>(entries: [(&str, Vec<NaiveDate>); N]) -> AppState {
    AppState::from_index(DistrictIndex::from_pairs(
        entries.map(|(district, dates)| (district.to_string(), dates)),
    ))
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_check() {
    let response = get(state_from_fixture(), "/health").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert_eq!(body["status"], "healthy");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – a district in the index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_valid_district() {
    let state = state_with_dates([("Kolbermoor", fake_dates("Kolbermoor").unwrap())]);

    let response = get(state, "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr[0].as_str().unwrap().starts_with("2026-01-15"));
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – a district in no plan returns 404.
//
// Every plan was read at startup, so an absent key is an observation about the
// data and not a gap in what was looked at — which is what makes this a 404 and
// not a "try again later".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_district_returns_404() {
    let response = get(
        state_from_fixture(),
        "/lk_rosenheim?district=NonExistentDistrict",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "District not found");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – missing query param returns 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_missing_district_parameter_returns_400() {
    let response = get(state_from_fixture(), "/lk_rosenheim").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // The 400 must carry the same `ErrorDetail` shape as every other error —
    // axum's own QueryRejection would be plain text, which is what the handler
    // maps away. Regression guard for the documented response schema.
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
}

// ---------------------------------------------------------------------------
// An empty or whitespace-only district is rejected as a bad parameter.
//
// Both normalize to "", which is not a name. Without the guard they would fall
// through to a plain index miss and answer 404 — reporting a district the
// caller never named as missing, instead of the unusable parameter they sent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_district_returns_400() {
    let response = get(state_from_fixture(), "/lk_rosenheim?district=").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
}

#[tokio::test]
async fn test_whitespace_only_district_returns_400() {
    let response = get(state_from_fixture(), "/lk_rosenheim?district=%20%20%09").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Every whitespace spelling of a district resolves to the same entry.
//
// The PDF stores names as character fragments, so matching strips whitespace on
// both sides; the index is keyed on that form and the handler has to normalize
// before looking up, or the spellings below would miss the row they all name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_whitespace_variants_resolve_to_the_same_district() {
    let app = build_router(state_from_fixture(), vec![]);

    let mut bodies = Vec::new();
    for spelling in [
        "Bad+Aibling",
        "BadAibling",
        "B+a+d++Aibling",
        "%20Bad%20Aibling%20",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/lk_rosenheim?district={spelling}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "spelling {spelling:?}");
        bodies.push(body_to_json(response).await);
    }

    assert!(
        bodies.windows(2).all(|w| w[0] == w[1]),
        "spellings of one district returned different dates: {bodies:?}"
    );
}

// ---------------------------------------------------------------------------
// Two districts are answered independently
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_districts_are_answered_independently() {
    let state = state_with_dates([
        ("Kolbermoor", fake_dates("Kolbermoor").unwrap()),
        (
            "Prien a. Chiemsee",
            fake_dates("Prien a. Chiemsee").unwrap(),
        ),
    ]);
    let app = build_router(state, vec![]);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Kolbermoor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let r2 = app
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Prien+a.+Chiemsee")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(r2.status(), StatusCode::OK);

    let d1 = body_to_json(r1).await;
    let d2 = body_to_json(r2).await;
    assert_ne!(d1, d2);
}

// ---------------------------------------------------------------------------
// Parametrized: districts whose names carry numbers or non-ASCII characters
// ---------------------------------------------------------------------------

macro_rules! api_district_test {
    ($name:ident, $district:expr) => {
        #[tokio::test]
        async fn $name() {
            let dates = fake_dates($district)
                .unwrap_or_else(|| vec![chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()]);
            let state = state_with_dates([($district, dates)]);
            let encoded = urlencoding::encode($district);
            let response = get(state, &format!("/lk_rosenheim?district={}", encoded)).await;

            assert_eq!(
                response.status(),
                StatusCode::OK,
                "district '{}' failed",
                $district
            );
            let body = body_to_json(response).await;
            assert!(
                !body.as_array().unwrap().is_empty(),
                "no dates for district '{}'",
                $district
            );
        }
    };
}

api_district_test!(test_api_aschau, "Aschau");
api_district_test!(test_api_bruckmuhl_1, "Bruckmühl 1");
api_district_test!(test_api_feldkirchen_2, "Feldkirchen 2");
api_district_test!(test_api_raubling_3, "Raubling 3");
