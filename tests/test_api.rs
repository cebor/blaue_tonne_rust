//! The request path: what a client can observe — a hit, a miss, and a bad
//! parameter. Faults while reading the plans belong to `test_index.rs`.

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

// --- Helpers ---

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

// --- Health check ---

#[tokio::test]
async fn test_health_check() {
    let response = get(state_from_fixture(), "/health").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert_eq!(body["status"], "healthy");
}

// --- GET /lk_rosenheim ---

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

#[tokio::test]
async fn test_missing_district_parameter_returns_400() {
    // 400, not 422: axum 0.8 changed the status for a missing query param.
    let response = get(state_from_fixture(), "/lk_rosenheim").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // The 400 carries the documented `ErrorDetail` shape rather than axum's own
    // plain-text QueryRejection.
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

// --- Normalization ---

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

// --- District names with numbers or non-ASCII characters ---

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

// --- The documented district parameter ---

/// The `district` parameter's `enum` in the served spec.
async fn documented_districts(state: AppState) -> Vec<String> {
    let spec = body_to_json(get(state, "/docs/openapi.json").await).await;
    let parameter = spec["paths"]["/lk_rosenheim"]["get"]["parameters"]
        .as_array()
        .expect("the operation must document its parameters")
        .iter()
        .find(|p| p["name"] == "district")
        .expect("district must be documented");

    parameter["schema"]["enum"]
        .as_array()
        .expect("district must be documented as an enum")
        .iter()
        .map(|v| v.as_str().expect("enum values are strings").to_string())
        .collect()
}

#[tokio::test]
async fn test_the_documented_districts_are_the_indexed_ones() {
    // The dropdown /docs offers comes from the index, not from a constant, so
    // it cannot name a district the service would answer 404 for.
    let districts = documented_districts(state_from_fixture()).await;

    assert_eq!(districts.len(), 50);
    assert!(districts.contains(&"Bad Aibling".to_string()));
    assert!(districts.contains(&"Nußdorf am Inn".to_string()));
    assert!(districts.contains(&"Großkarolinenfeld 1".to_string()));
    // Sorted: a dropdown in HashMap order would reshuffle on every start.
    let mut sorted = districts.clone();
    sorted.sort();
    assert_eq!(districts, sorted);
}

#[tokio::test]
async fn test_a_documented_district_is_one_the_service_answers() {
    // The printed name, not the normalized key: what the dropdown offers has to
    // work when "Try it out" sends it back verbatim.
    for district in documented_districts(state_from_fixture()).await {
        let response = get(
            state_from_fixture(),
            &format!("/lk_rosenheim?district={}", urlencoding::encode(&district)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "district {district:?}");
    }
}
