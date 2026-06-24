use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use bytes::Bytes;
use chrono::NaiveDate;
use http_body_util::BodyExt;
use tower::ServiceExt;

use blaue_tonne_rust::config::Plan;
use blaue_tonne_rust::{build_router, AppState};

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

fn state_with_cached_dates(district: &str, dates: Vec<NaiveDate>) -> AppState {
    let state = AppState::new(vec![]);
    state.dates_cache.insert(district.to_string(), dates);
    state
}

fn fixture_pdf_bytes() -> Bytes {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lk_rosenheim_2026.pdf");
    Bytes::from(std::fs::read(&path).expect("fixture PDF not found"))
}

fn state_with_fixture_pdf() -> AppState {
    let pdf_bytes = fixture_pdf_bytes();

    let plan = Plan {
        url: "https://fake.test/schedule.pdf".to_string(),
        pages: "1,2".to_string(),
    };
    let state = AppState::new(vec![plan]);
    state
        .pdf_cache
        .insert("https://fake.test/schedule.pdf".to_string(), pdf_bytes);
    state
}

async fn body_to_json(response: Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(state: AppState, path: &str) -> Response {
    let app = build_router(state, vec![]);
    app.oneshot(
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_check() {
    let response = get(AppState::new(vec![]), "/health").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert_eq!(body["status"], "healthy");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – valid district (pre-cached)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_valid_district_from_cache() {
    let dates = fake_dates("Kolbermoor").unwrap();
    let state = state_with_cached_dates("Kolbermoor", dates);

    let response = get(state, "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr[0].as_str().unwrap().starts_with("2026-01-15"));
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – invalid district returns 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_invalid_district_returns_404() {
    let state = state_with_fixture_pdf();

    let response = get(state, "/lk_rosenheim?district=NonExistentDistrict").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "District not found");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – missing query param returns 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_missing_district_parameter_returns_422() {
    let response = get(AppState::new(vec![]), "/lk_rosenheim").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Cache: second request re-uses cached result (no PDF re-parse)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cache_prevents_repeated_pdf_parsing() {
    let dates = fake_dates("Bad Aibling").unwrap();
    let state = state_with_cached_dates("Bad Aibling", dates.clone());
    let app = build_router(state.clone(), vec![]);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Bad+Aibling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let d1 = body_to_json(r1).await;

    // Manually confirm cache has the entry
    assert!(state.dates_cache.contains_key("Bad Aibling"));

    let r2 = app
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Bad+Aibling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let d2 = body_to_json(r2).await;

    assert_eq!(d1, d2);
}

// ---------------------------------------------------------------------------
// Cache: two different districts have separate entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_districts_separate_cache_entries() {
    let state = AppState::new(vec![]);
    state
        .dates_cache
        .insert("Kolbermoor".to_string(), fake_dates("Kolbermoor").unwrap());
    state.dates_cache.insert(
        "Prien a. Chiemsee".to_string(),
        fake_dates("Prien a. Chiemsee").unwrap(),
    );
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
// No plans → DistrictNotFound (no PDF to scan)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_no_plans_returns_404() {
    let response = get(AppState::new(vec![]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Invalid URL in plan → 400  (mocked: server returns HTML instead of PDF)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalid_pdf_url_returns_400() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/not-a-pdf")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html><body>Not a PDF</body></html>")
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/not-a-pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("PDF"),
        "expected PDF error detail, got: {}",
        body["detail"]
    );
}

// ---------------------------------------------------------------------------
// Parametrized: districts with numbers in names (via fixture PDF)
// ---------------------------------------------------------------------------

macro_rules! api_district_test {
    ($name:ident, $district:expr) => {
        #[tokio::test]
        async fn $name() {
            let dates = fake_dates($district).unwrap_or_else(|| {
                vec![chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()]
            });
            let state = state_with_cached_dates($district, dates);
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

// ---------------------------------------------------------------------------
// download_pdf: full real fetch path via mockito (serves the fixture PDF)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_download_pdf_full_fetch_success() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/schedule.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/schedule.pdf", mock_server.url()),
        pages: "1,2".to_string(),
    };
    let url = plan.url.clone();
    let state = AppState::new(vec![plan]);

    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());

    // The fetched PDF bytes were cached under the URL.
    assert!(state.pdf_cache.contains_key(&url));
}

// ---------------------------------------------------------------------------
// get_dates success path against the pre-cached fixture PDF (no network)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_from_fixture_caches_result() {
    let state = state_with_fixture_pdf();
    assert!(!state.dates_cache.contains_key("Kolbermoor"));

    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());

    // Handler stored the parsed dates in the cache.
    assert!(state.dates_cache.contains_key("Kolbermoor"));
}

// ---------------------------------------------------------------------------
// download_pdf: 404 → soft skip → eventually 404 DistrictNotFound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_404_is_soft_skipped() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/missing.pdf")
        .with_status(404)
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/missing.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// download_pdf: non-2xx (500) → 500 propagated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_server_error_returns_500() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/broken.pdf")
        .with_status(500)
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/broken.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// download_pdf: URL not ending in .pdf → 400 InvalidUrl (no network)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_non_pdf_url_returns_400() {
    let plan = Plan {
        url: "http://example.test/not-a-pdf-file".to_string(),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert!(body["detail"].as_str().unwrap().contains("PDF"));
}

// ---------------------------------------------------------------------------
// download_pdf: .pdf URL but wrong content-type → 400 InvalidUrl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_url_wrong_content_type_returns_400() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/fake.pdf")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html>not a pdf</html>")
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/fake.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert!(body["detail"].as_str().unwrap().contains("valid PDF"));
}

// ---------------------------------------------------------------------------
// download_pdf: connection error (no server) → 500 PdfError
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_connection_error_returns_500() {
    // Port 1 is reserved/unused → connection refused (not a timeout).
    let plan = Plan {
        url: "http://127.0.0.1:1/schedule.pdf".to_string(),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// get_dates: valid download but unparseable PDF bytes → 500 PdfError
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_corrupt_pdf_returns_500() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/corrupt.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("not actually a pdf")
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/corrupt.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(AppState::new(vec![plan]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
