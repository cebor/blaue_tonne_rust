pub mod config;
pub mod errors;
pub mod handlers;
pub mod pdf_parser;
pub mod state;

pub use state::{AppState, ResolvedClientIp};

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::ConnectInfo;
use axum::{middleware, routing::get, Router};
use ipnet::IpNet;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::{Config, SwaggerUi};

// ---------------------------------------------------------------------------
// OpenAPI spec
// ---------------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::health_check,
        handlers::lk_rosenheim_handler,
    ),
    components(
        schemas(
            handlers::HealthResponse,
            handlers::ErrorDetail,
            handlers::DistrictQuery,
        )
    ),
    info(
        title = "Blaue Tonne API",
        version = "0.1.0",
        description = "Altpapier (Blaue Tonne) collection dates for Landkreis Rosenheim",
        contact(
            name = "Source Code",
            url = "https://gitlab.stkn.org/felix/blaue_tonne_rust"
        ),
        license(
            name = "MIT",
            identifier = "MIT"
        )
    )
)]
pub struct ApiDoc;

// ---------------------------------------------------------------------------
// Router builder (public for integration tests)
// ---------------------------------------------------------------------------

pub fn build_router(state: AppState, forwarded_allow_ips: Vec<IpNet>) -> Router {
    let api_doc_url = "/docs/openapi.json";
    let api_doc_config = Config::new([api_doc_url]).use_base_layout();
    let allow_ips = Arc::new(forwarded_allow_ips);

    // Middleware: resolve the real client IP from ConnectInfo or X-Forwarded-For.
    // Trusted proxy check: only forward headers from IPs listed in allow_ips.
    let ip_middleware = {
        let allow_ips = Arc::clone(&allow_ips);
        middleware::from_fn(
            move |mut req: axum::extract::Request, next: middleware::Next| {
                let allow_ips = Arc::clone(&allow_ips);
                async move {
                    let peer_ip: Option<IpAddr> = req
                        .extensions()
                        .get::<ConnectInfo<SocketAddr>>()
                        .map(|ci| ci.0.ip());

                    let client_ip = if let Some(peer) = peer_ip {
                        if allow_ips.iter().any(|net| net.contains(&peer)) {
                            // Proxy is trusted: use leftmost entry of X-Forwarded-For
                            req.headers()
                                .get("x-forwarded-for")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|s| s.split(',').next())
                                .and_then(|s| s.trim().parse::<IpAddr>().ok())
                                .unwrap_or(peer)
                        } else {
                            peer
                        }
                    } else {
                        // No ConnectInfo available (e.g. unit tests with oneshot)
                        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                    };

                    req.extensions_mut().insert(ResolvedClientIp(client_ip));
                    next.run(req).await
                }
            },
        )
    };

    // TraceLayer: creates a span per request containing method, URI and client IP.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|req: &axum::extract::Request| {
            let client_ip = req
                .extensions()
                .get::<ResolvedClientIp>()
                .map(|r| r.0.to_string())
                .unwrap_or_else(|| "-".to_string());
            tracing::info_span!(
                "request",
                method = %req.method(),
                uri    = %req.uri(),
                client_ip = %client_ip,
            )
        })
        .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::TRACE))
        .on_response(
            |res: &axum::response::Response,
             latency: std::time::Duration,
             _span: &tracing::Span| {
                tracing::info!(
                    status     = res.status().as_u16(),
                    latency_ms = latency.as_millis(),
                    "response sent"
                );
            },
        );

    Router::new()
        .merge(SwaggerUi::new("/docs").url(api_doc_url, ApiDoc::openapi()).config(api_doc_config))
        .route("/health", get(handlers::health_check))
        .route("/lk_rosenheim", get(handlers::lk_rosenheim_handler))
        // Layer order with Router::layer: last `.layer()` call = outermost (runs first).
        // ip_middleware must run before trace_layer so the span already has client_ip.
        .layer(trace_layer)
        .layer(ip_middleware)
        .with_state(state)
}
