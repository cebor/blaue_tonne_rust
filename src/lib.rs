pub mod config;
pub mod errors;
pub mod handlers;
pub mod middleware;
pub mod pdf_parser;
pub mod state;

pub use state::{AppState, ResolvedClientIp};

use std::sync::Arc;

use axum::{routing::get, Router};
use ipnet::IpNet;
use tower_http::trace::{DefaultOnRequest, TraceLayer};
use tracing::Level;
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

    // Resolve the real client IP (ConnectInfo / X-Forwarded-For) before tracing.
    let ip_middleware =
        axum::middleware::from_fn_with_state(allow_ips, middleware::resolve_client_ip);

    // TraceLayer: creates a span per request containing method, URI and client IP.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(middleware::make_request_span)
        .on_request(DefaultOnRequest::new().level(Level::TRACE))
        .on_response(middleware::log_response);

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
