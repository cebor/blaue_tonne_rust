use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;

use blaue_tonne_rust::errors::AppError;

async fn body_to_json(response: Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_district_not_found_response() {
    let response = AppError::DistrictNotFound.into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "District not found");
}

#[tokio::test]
async fn test_bad_request_response() {
    let response = AppError::BadRequest("missing field `district`".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
}

#[tokio::test]
async fn test_the_internal_detail_never_reaches_the_client() {
    // `BadRequest` is the only variant carrying free text: axum's rejection
    // string, which is logged and never serialized.
    let response = AppError::BadRequest("Failed to deserialize query string: xyzzy".to_string())
        .into_response();
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
    assert!(!body["detail"].as_str().unwrap().contains("xyzzy"));
}

// --- No response may disclose that this service fetches and parses PDFs ---

/// Never called. Adding a variant to `AppError` stops this from compiling,
/// which is the reminder to extend `variants` in the test below — a list of
/// values cannot enforce exhaustiveness on its own.
#[allow(dead_code)]
fn assert_every_variant_is_covered(e: &AppError) {
    match e {
        AppError::BadRequest(_) => {}
        AppError::DistrictNotFound => {}
    }
}

#[tokio::test]
async fn test_no_variant_discloses_the_data_source() {
    // The free text carries everything that must not come out the other side.
    let variants = [
        AppError::BadRequest("https://chiemgau-recycling.test/plan.pdf: 500".to_string()),
        AppError::DistrictNotFound,
    ];

    // Each of these would betray the file format, the fetch, or the source.
    const DISCLOSING: &[&str] = &[
        "pdf",
        "plan",
        "upstream",
        "gateway",
        "download",
        "parse",
        "http://",
        "https://",
        "url",
        "recycling",
        "chiemgau",
    ];

    for variant in variants {
        let internal = variant.to_string();
        let body = body_to_json(variant.into_response()).await;
        let detail = body["detail"].as_str().unwrap().to_lowercase();

        for needle in DISCLOSING {
            assert!(
                !detail.contains(needle),
                "response for {internal:?} leaks {needle:?}: {detail:?}"
            );
        }
    }
}
