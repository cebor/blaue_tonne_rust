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
async fn test_service_unavailable_response() {
    let response =
        AppError::ServiceUnavailable("https://secret.internal/plan.pdf timed out".to_string())
            .into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    assert_eq!(
        body["detail"],
        "Service temporarily unavailable, please try again later"
    );
    // The URL is what makes the *log* useful; it must still not reach the client.
    assert!(!body["detail"].as_str().unwrap().contains("secret.internal"));
}

#[tokio::test]
async fn test_upstream_response() {
    let response =
        AppError::Upstream("https://secret.internal/plan.pdf exploded".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    assert_eq!(
        body["detail"],
        "Service temporarily unavailable, please try again later"
    );
    // The internal detail (here: an upstream URL) must not reach the client.
    assert!(!body["detail"].as_str().unwrap().contains("secret.internal"));
}

#[tokio::test]
async fn test_pdf_not_found_response() {
    let response =
        AppError::PdfNotFound("https://secret.internal/plan.pdf".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    assert!(!body["detail"].as_str().unwrap().contains("secret.internal"));
}

#[tokio::test]
async fn test_pdf_error_response() {
    let response = AppError::PdfError("boom at offset 0x41".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Internal server error");
    assert!(!body["detail"].as_str().unwrap().contains("boom"));
}

// ---------------------------------------------------------------------------
// No response may disclose that this service fetches and parses PDFs from a
// third party. Asserted over every variant at once, so a message added later
// is covered without anyone remembering to extend this file. A brand-new
// *variant* is a different matter — `assert_every_variant_is_covered` below
// turns that into a compile error rather than a silent gap.
// ---------------------------------------------------------------------------

/// Makes the list below exhaustive by construction: adding a variant to
/// `AppError` stops this from compiling, which is the reminder to add it to
/// `variants` in the test — the list itself cannot enforce that on its own.
///
/// Never called; it exists purely for the compile-time check.
#[allow(dead_code)]
fn assert_every_variant_is_covered(e: &AppError) {
    match e {
        AppError::BadRequest(_) => {}
        AppError::DistrictNotFound => {}
        AppError::ServiceUnavailable(_) => {}
        AppError::Upstream(_) => {}
        AppError::PdfNotFound(_) => {}
        AppError::PdfError(_) => {}
    }
}

#[tokio::test]
async fn test_no_variant_discloses_the_data_source() {
    let variants = [
        AppError::BadRequest("missing field `district`".to_string()),
        AppError::DistrictNotFound,
        AppError::ServiceUnavailable("https://chiemgau-recycling.test/plan.pdf".to_string()),
        AppError::Upstream("https://chiemgau-recycling.test/plan.pdf: 500".to_string()),
        AppError::PdfNotFound("https://chiemgau-recycling.test/plan.pdf".to_string()),
        AppError::PdfError("xref table malformed".to_string()),
    ];

    // Substrings that would each betray the architecture: the file format, the
    // fact that a plan is fetched, or that something sits behind us.
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
