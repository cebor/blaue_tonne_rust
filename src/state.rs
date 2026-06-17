use std::net::IpAddr;
use std::sync::Arc;

use bytes::Bytes;
use chrono::NaiveDate;
use dashmap::DashMap;
use reqwest::Client;

use crate::config::Plan;

// ---------------------------------------------------------------------------
// Extension type: resolved client IP (set by IP-resolution middleware)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct ResolvedClientIp(pub IpAddr);

// ---------------------------------------------------------------------------
// App state (public so integration tests can build it)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub plans: Arc<Vec<Plan>>,
    pub dates_cache: Arc<DashMap<String, Vec<NaiveDate>>>,
    pub pdf_cache: Arc<DashMap<String, Bytes>>,
    pub http_client: Client,
}

impl AppState {
    pub fn new(plans: Vec<Plan>) -> Self {
        Self {
            plans: Arc::new(plans),
            dates_cache: Arc::new(DashMap::new()),
            pdf_cache: Arc::new(DashMap::new()),
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}
