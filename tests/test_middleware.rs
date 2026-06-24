use std::net::SocketAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::Request,
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use http_body_util::BodyExt;
use ipnet::IpNet;
use tower::ServiceExt;
use tracing::Level;

use blaue_tonne_rust::middleware::{log_response, make_request_span, resolve_client_ip};
use blaue_tonne_rust::ResolvedClientIp;

// ---------------------------------------------------------------------------
// resolve_client_ip — exercised via a mini-router that echoes the resolved IP
// ---------------------------------------------------------------------------

async fn echo_ip(Extension(ip): Extension<ResolvedClientIp>) -> String {
    ip.0.to_string()
}

fn router(allow: Vec<IpNet>) -> Router {
    let allow = Arc::new(allow);
    Router::new()
        .route("/", get(echo_ip))
        .layer(from_fn_with_state(allow, resolve_client_ip))
}

async fn body_to_string(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn ip(s: &str) -> IpNet {
    s.parse::<std::net::IpAddr>().unwrap().into()
}

#[tokio::test]
async fn test_resolve_ip_no_connect_info_falls_back_to_localhost() {
    let response = router(vec![])
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "127.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_proxy_uses_leftmost_xff() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let response = router(vec![ip("10.0.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "1.2.3.4");
}

#[tokio::test]
async fn test_resolve_ip_untrusted_proxy_uses_peer() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    // allowlist does NOT contain the peer → XFF is ignored.
    let response = router(vec![ip("192.168.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "1.2.3.4")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "10.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_proxy_broken_xff_falls_back_to_peer() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let response = router(vec![ip("10.0.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "garbage-not-an-ip")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "10.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_via_cidr() {
    let peer: SocketAddr = "10.5.6.7:5000".parse().unwrap();
    let response = router(vec!["10.0.0.0/8".parse().unwrap()])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "9.9.9.9")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "9.9.9.9");
}

// ---------------------------------------------------------------------------
// make_request_span / log_response — tracing helpers
// ---------------------------------------------------------------------------

static INIT: Once = Once::new();

fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(Level::TRACE)
            .with_test_writer()
            .init();
    });
}

#[test]
fn test_make_request_span_health_is_trace() {
    init_tracing();
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let span = make_request_span(&req);
    assert_eq!(span.metadata().unwrap().level(), &Level::TRACE);
}

#[test]
fn test_make_request_span_other_is_info() {
    init_tracing();
    let req = Request::builder()
        .uri("/lk_rosenheim")
        .body(Body::empty())
        .unwrap();
    let span = make_request_span(&req);
    assert_eq!(span.metadata().unwrap().level(), &Level::INFO);
}

#[test]
fn test_log_response_both_branches() {
    init_tracing();
    let res = ().into_response();

    // INFO span → info branch
    let info_span = tracing::info_span!("request");
    log_response(&res, Duration::from_millis(3), &info_span);

    // TRACE span → trace branch
    let trace_span = tracing::trace_span!("request");
    log_response(&res, Duration::from_millis(3), &trace_span);
}
