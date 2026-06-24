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
async fn test_service_unavailable_response() {
    let response = AppError::ServiceUnavailable.into_response();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Service temporarily unavailable");
}

#[tokio::test]
async fn test_invalid_url_response() {
    let response = AppError::InvalidUrl("bad url here".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "bad url here");
}

#[tokio::test]
async fn test_pdf_error_response() {
    let response = AppError::PdfError("boom".to_string()).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "boom");
}
