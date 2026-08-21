//! Router builder (public for integration tests).

use std::sync::Arc;

use axum::{Router, routing::get};
use ipnet::IpNet;
use tower_http::trace::{DefaultOnRequest, TraceLayer};
use tracing::Level;
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::openapi::ApiDoc;
use crate::state::AppState;
use crate::{handlers, middleware};

pub fn build_router(state: AppState, forwarded_allow_ips: Vec<IpNet>) -> Router {
    let api_doc_url = "/docs/openapi.json";
    let api_doc_config = Config::new([api_doc_url]).use_base_layout();
    let allow_ips = Arc::new(forwarded_allow_ips);

    let ip_middleware =
        axum::middleware::from_fn_with_state(allow_ips, middleware::resolve_client_ip);

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(middleware::make_request_span)
        .on_request(DefaultOnRequest::new().level(Level::TRACE))
        .on_response(middleware::log_response);

    // `Router::layer` only affects routes registered before it, so /health —
    // added to the outer router below — carries neither layer and is not traced
    // at any level. Keeps container health checks out of the logs.
    let traced = Router::new()
        .merge(
            SwaggerUi::new("/docs")
                .url(api_doc_url, ApiDoc::with_districts(state.index.names()))
                .config(api_doc_config),
        )
        .route("/", get(handlers::redirect_to_docs))
        .route("/lk_rosenheim", get(handlers::lk_rosenheim_handler))
        // Last `.layer()` = outermost = runs first. ip_middleware has to run
        // before trace_layer so the span already has client_ip.
        .layer(trace_layer)
        .layer(ip_middleware);

    Router::new()
        .route("/health", get(handlers::health_check))
        .merge(traced)
        .with_state(state)
}
