pub mod config;
pub mod errors;
pub mod pdf_parser;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::ConnectInfo;
use axum::{middleware, routing::get, Router};
use bytes::Bytes;
use chrono::NaiveDate;
use dashmap::DashMap;
use ipnet::IpNet;
use reqwest::Client;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use config::Plan;

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
        .merge(SwaggerUi::new("/docs").url("/api-doc/openapi.json", ApiDoc::openapi()))
        .route("/health", get(handlers::health_check))
        .route("/lk_rosenheim", get(handlers::lk_rosenheim_handler))
        // Layer order with Router::layer: last `.layer()` call = outermost (runs first).
        // ip_middleware must run before trace_layer so the span already has client_ip.
        .layer(trace_layer)
        .layer(ip_middleware)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers (pub(crate) so they are accessible via build_router)
// ---------------------------------------------------------------------------

pub mod handlers {
    use axum::{
        extract::{Query, State},
        http::StatusCode,
        Json,
    };
    use bytes::Bytes;
    use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
    use dashmap::DashMap;
    use reqwest::Client;
    use serde::{Deserialize, Serialize};

    use super::AppState;
    use crate::errors::AppError;
    use crate::pdf_parser::get_dates;

    /// Successful response from the health endpoint
    #[derive(Serialize, utoipa::ToSchema)]
    pub struct HealthResponse {
        pub status: String,
    }

    /// Error response body returned on 4xx/5xx
    #[derive(Serialize, utoipa::ToSchema)]
    pub struct ErrorDetail {
        pub detail: String,
    }

    #[utoipa::path(
        get,
        path = "/health",
        responses(
            (status = 200, description = "Service is healthy", body = HealthResponse)
        ),
        tag = "health"
    )]
    pub async fn health_check() -> Json<HealthResponse> {
        Json(HealthResponse { status: "healthy".to_string() })
    }

    #[derive(Deserialize, utoipa::ToSchema)]
    pub struct DistrictQuery {
        /// Name of the district (Gemeinde), e.g. "Bad Aibling"
        pub district: String,
    }

    pub async fn download_pdf(
        client: &Client,
        pdf_cache: &DashMap<String, Bytes>,
        url: &str,
    ) -> Result<Bytes, AppError> {
        if let Some(cached) = pdf_cache.get(url) {
            return Ok(cached.clone());
        }

        if !url.to_lowercase().ends_with(".pdf") {
            return Err(AppError::InvalidUrl(
                "URL must point to a PDF file".to_string(),
            ));
        }

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::ServiceUnavailable
                } else {
                    AppError::PdfError(e.to_string())
                }
            })?;

        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(AppError::PdfError(format!("PDF not found at {url}")));
        }
        if !status.is_success() {
            return Err(AppError::PdfError(format!("HTTP {status} fetching PDF")));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        if !content_type.starts_with("application/pdf") {
            return Err(AppError::InvalidUrl(
                "URL does not point to a valid PDF file".to_string(),
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| AppError::PdfError(e.to_string()))?;

        pdf_cache.insert(url.to_string(), bytes.clone());
        Ok(bytes)
    }

    fn dates_to_iso(dates: &[NaiveDate]) -> Vec<String> {
        dates
            .iter()
            .map(|d| {
                let dt: NaiveDateTime = d.and_hms_opt(0, 0, 0).unwrap();
                Utc.from_utc_datetime(&dt).to_rfc3339()
            })
            .collect()
    }

    #[utoipa::path(
        get,
        path = "/lk_rosenheim",
        params(
            ("district" = String, Query, description = "Name of the district (Gemeinde), e.g. \"Bad Aibling\"")
        ),
        responses(
            (status = 200, description = "Collection dates in RFC 3339 UTC format", body = Vec<String>),
            (status = 400, description = "Bad request (invalid URL or parameter)", body = ErrorDetail),
            (status = 404, description = "District not found", body = ErrorDetail),
            (status = 504, description = "PDF service unavailable (timeout)", body = ErrorDetail),
        ),
        tag = "dates"
    )]
    pub async fn lk_rosenheim_handler(
        State(state): State<AppState>,
        Query(params): Query<DistrictQuery>,
    ) -> Result<Json<Vec<String>>, AppError> {
        let district = &params.district;

        if let Some(cached) = state.dates_cache.get(district.as_str()) {
            return Ok(Json(dates_to_iso(&cached)));
        }

        let mut all_dates: Vec<NaiveDate> = Vec::new();

        for plan in state.plans.iter() {
            let pdf_bytes =
                match download_pdf(&state.http_client, &state.pdf_cache, &plan.url).await {
                    Ok(b) => b,
                    Err(AppError::PdfError(msg)) if msg.contains("not found") => continue,
                    Err(e) => return Err(e),
                };

            match get_dates(&pdf_bytes, &plan.pages, district) {
                Ok(dates) => all_dates.extend(dates),
                Err(AppError::DistrictNotFound) => return Err(AppError::DistrictNotFound),
                Err(e) => return Err(e),
            }
        }

        if all_dates.is_empty() {
            return Err(AppError::DistrictNotFound);
        }

        state.dates_cache.insert(district.clone(), all_dates.clone());
        Ok(Json(dates_to_iso(&all_dates)))
    }
}
