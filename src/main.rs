use std::path::PathBuf;

use blaue_tonne_rust::build_router;
use blaue_tonne_rust::config::load_plans;
use blaue_tonne_rust::AppState;
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

    let plans = load_plans(&plans_path).expect("failed to load plans.yaml");
    info!("Loaded {} plan(s)", plans.len());

    let state = AppState::new(plans);
    let app = build_router(state);

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    info!("Listening on {bind_addr}");
    axum::serve(listener, app).await.expect("server error");
}
