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
    // `BadRequest` is the only variant carrying free text — axum's rejection
    // string. It is logged, never serialized, and this is what pins that.
    let response = AppError::BadRequest("Failed to deserialize query string: xyzzy".to_string())
        .into_response();
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
    assert!(!body["detail"].as_str().unwrap().contains("xyzzy"));
}

// ---------------------------------------------------------------------------
// No response may disclose that this service fetches and parses PDFs from a
// third party. Asserted over every variant at once, so a message added later
// is covered without anyone remembering to extend this file. A brand-new
// *variant* is a different matter — `assert_every_variant_is_covered` below
// turns that into a compile error rather than a silent gap.
//
// Since the startup faults moved to `PlanError` this is mostly true by
// construction: no variant left even has a plan URL to leak. The test stays
// because the invariant is about what may be *added*, not about what is here —
// and a variant carrying upstream text is exactly what someone would add first
// if request-time fetching ever came back.
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
    }
}

#[tokio::test]
async fn test_no_variant_discloses_the_data_source() {
    // The free text is deliberately full of things that must not come out the
    // other side, so the test would fail loudly if a variant ever started
    // echoing its internal detail.
    let variants = [
        AppError::BadRequest("https://chiemgau-recycling.test/plan.pdf: 500".to_string()),
        AppError::DistrictNotFound,
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
