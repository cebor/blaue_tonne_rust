use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use blaue_tonne_rust::AppState;
use blaue_tonne_rust::build_router;
use blaue_tonne_rust::cache::PdfCache;
use blaue_tonne_rust::config::{healthcheck_url, load_plans, parse_forwarded_allow_ips};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";

fn bind_addr() -> String {
    std::env::var("BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
}

/// Shorter than the Dockerfile's `HEALTHCHECK --timeout`, so a probe that hangs
/// ends as this process's exit code rather than as a kill.
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// `blaue_tonne_rust healthcheck` performs a GET on /health and exits with
/// code 0 (healthy) or 1. Used by the Docker HEALTHCHECK, because the distroless
/// runtime image has neither a shell nor curl.
async fn run_healthcheck() -> ! {
    let url = healthcheck_url(&bind_addr());

    let ok = match reqwest::Client::builder()
        .timeout(HEALTHCHECK_TIMEOUT)
        .build()
    {
        Ok(client) => client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        Err(_) => false,
    };

    std::process::exit(if ok { 0 } else { 1 });
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        run_healthcheck().await;
    }

    // `RUST_LOG` takes full control when set; the fallback applies only when it
    // is absent.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("blaue_tonne_rust=info")),
        )
        .init();

    let plans_path =
        PathBuf::from(std::env::var("PLANS_PATH").unwrap_or_else(|_| "plans.yaml".to_string()));

    // Comma-separated IPs/CIDRs whose X-Forwarded-For headers are trusted.
    // Default: empty, i.e. X-Forwarded-For is not trusted.
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

    // Unreadable file, invalid YAML, or a plan URL `download_pdf` could never
    // fetch. Logged and exited rather than `expect`ed, so the detail reaches an
    // operator through tracing like every other startup fault.
    let plans = match load_plans(&plans_path) {
        Ok(plans) => plans,
        Err(e) => {
            error!(path = %plans_path.display(), error = %e, "failed to load the plans config, refusing to start");
            std::process::exit(1);
        }
    };
    info!("Loaded {} plan(s)", plans.len());

    // Logs the resolved directory, or that the cache is off.
    let cache = PdfCache::from_env();

    // Every plan is read here, once; the service does no network I/O after this
    // point. A plan that cannot be read is fatal.
    let state = match AppState::build(&plans, &cache).await {
        Ok(state) => state,
        Err(e) => {
            error!(error = %e, "failed to build the district index, refusing to start");
            std::process::exit(1);
        }
    };
    info!("Indexed {} district(s)", state.index.len());

    let app = build_router(state, forwarded_allow_ips);

    let bind_addr = bind_addr();
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            error!(addr = %bind_addr, error = %e, "failed to bind, refusing to start");
            std::process::exit(1);
        }
    };

    info!("Listening on {bind_addr}");
    info!("API docs available at http://{bind_addr}/docs");

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    {
        error!(error = %e, "server error");
        std::process::exit(1);
    }
}

/// Resolves on SIGINT (ctrl+c) or SIGTERM (`docker stop` / Kubernetes), letting
/// `axum::serve` shut down gracefully.
///
/// The handlers are installed explicitly because PID 1 — which this process is
/// in the container — ignores signals it has no handler for.
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
