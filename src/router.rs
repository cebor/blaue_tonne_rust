//! Router builder (public for integration tests).

use std::sync::Arc;

use axum::{Router, routing::get};
use ipnet::IpNet;
use tower_http::trace::{DefaultOnRequest, TraceLayer};
use tracing::Level;
use utoipa::OpenApi;
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::openapi::ApiDoc;
use crate::state::AppState;
use crate::{handlers, middleware};

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
        .merge(
            SwaggerUi::new("/docs")
                .url(api_doc_url, ApiDoc::openapi())
                .config(api_doc_config),
        )
        .route("/health", get(handlers::health_check))
        .route("/lk_rosenheim", get(handlers::lk_rosenheim_handler))
        // Layer order with Router::layer: last `.layer()` call = outermost (runs first).
        // ip_middleware must run before trace_layer so the span already has client_ip.
        .layer(trace_layer)
        .layer(ip_middleware)
        .with_state(state)
}
