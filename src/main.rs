use std::net::SocketAddr;
use std::path::PathBuf;

use blaue_tonne_rust::build_router;
use blaue_tonne_rust::config::{load_plans, parse_forwarded_allow_ips};
use blaue_tonne_rust::AppState;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // `RUST_LOG` fully controls filtering when set (so e.g.
    // `RUST_LOG=blaue_tonne_rust=trace` surfaces /health request logs); only
    // when it is absent do we fall back to a sensible default.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("blaue_tonne_rust=info")),
        )
        .init();

    let plans_path = PathBuf::from(
        std::env::var("PLANS_PATH").unwrap_or_else(|_| "plans.yaml".to_string()),
    );

    // Comma-separated list of IPs/CIDRs whose X-Forwarded-For headers are trusted.
    // Use "*" to trust all proxies. Default: empty (X-Forwarded-For not trusted).
    let forwarded_allow_ips =
        parse_forwarded_allow_ips(&std::env::var("FORWARDED_ALLOW_IPS").unwrap_or_default());

    if forwarded_allow_ips.is_empty() {
        info!("FORWARDED_ALLOW_IPS: none — X-Forwarded-For headers will not be trusted");
    } else {
        info!(
            "FORWARDED_ALLOW_IPS: {}",
            forwarded_allow_ips
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let plans = load_plans(&plans_path).expect("failed to load plans.yaml");
    info!("Loaded {} plan(s)", plans.len());

    let state = AppState::new(plans);
    let app = build_router(state, forwarded_allow_ips);

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    info!("Listening on {bind_addr}");
    info!("API docs available at http://{bind_addr}/docs");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
