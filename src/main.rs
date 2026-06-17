use std::net::SocketAddr;
use std::path::PathBuf;

use blaue_tonne_rust::build_router;
use blaue_tonne_rust::config::load_plans;
use blaue_tonne_rust::AppState;
use ipnet::IpNet;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(
            "blaue_tonne_rust=info".parse().unwrap(),
        ))
        .init();

    let plans_path = PathBuf::from(
        std::env::var("PLANS_PATH").unwrap_or_else(|_| "plans.yaml".to_string()),
    );

    // Comma-separated list of IPs/CIDRs whose X-Forwarded-For headers are trusted.
    // Use "*" to trust all proxies. Default: empty (X-Forwarded-For not trusted).
    let forwarded_allow_ips: Vec<IpNet> = std::env::var("FORWARDED_ALLOW_IPS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // Accept both plain IPs ("127.0.0.1") and CIDR notation ("10.0.0.0/8").
            s.parse::<IpNet>()
                .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                .map_err(|e| tracing::warn!("Ignoring invalid FORWARDED_ALLOW_IPS entry {s:?}: {e}"))
                .ok()
        })
        .collect();

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
