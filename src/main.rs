use std::net::SocketAddr;
use std::path::PathBuf;

use blaue_tonne_rust::build_router;
use blaue_tonne_rust::config::{load_plans, parse_forwarded_allow_ips};
use blaue_tonne_rust::AppState;
use tracing::info;
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";

fn bind_addr() -> String {
    std::env::var("BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
}

/// `blaue_tonne_rust healthcheck` performs a GET on /health and exits with
/// code 0 (healthy) or 1. Used by the Docker HEALTHCHECK: the distroless
/// runtime image has neither a shell nor curl.
async fn run_healthcheck() -> ! {
    let url = format!("http://{}/health", bind_addr().replace("0.0.0.0", "127.0.0.1"));
    let ok = reqwest::get(&url)
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    std::process::exit(if ok { 0 } else { 1 });
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        run_healthcheck().await;
    }

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

    let bind_addr = bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    info!("Listening on {bind_addr}");
    info!("API docs available at http://{bind_addr}/docs");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server error");
}

/// Resolves on SIGINT (ctrl+c) or SIGTERM (`docker stop` / Kubernetes), letting
/// `axum::serve` shut down gracefully. Installing these handlers explicitly is
/// also what makes the process respond to signals when it runs as PID 1 in the
/// container (no `tini`): an unhandled SIGINT/SIGTERM is ignored by PID 1.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, stopping server");
}
