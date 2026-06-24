//! Request middleware and tracing helpers used by `build_router`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use ipnet::IpNet;
use tracing::Span;

use crate::state::ResolvedClientIp;

/// Resolve the real client IP and insert it as a [`ResolvedClientIp`] extension.
///
/// If the connecting peer is listed in `allow_ips` (the trusted-proxy
/// allowlist), the leftmost `X-Forwarded-For` entry is used; otherwise the
/// socket peer IP is used. Falls back to `127.0.0.1` when no `ConnectInfo` is
/// present (e.g. unit tests using `oneshot`).
///
/// Wired up via `axum::middleware::from_fn_with_state` with the allowlist as
/// state.
pub async fn resolve_client_ip(
    State(allow_ips): State<Arc<Vec<IpNet>>>,
    mut req: Request,
    next: Next,
) -> Response {
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
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };

    req.extensions_mut().insert(ResolvedClientIp(client_ip));
    next.run(req).await
}

/// Span factory for `TraceLayer`: one span per request with method, URI and the
/// already-resolved client IP.
pub fn make_request_span(req: &Request) -> Span {
    let client_ip = req
        .extensions()
        .get::<ResolvedClientIp>()
        .map(|r| r.0.to_string())
        .unwrap_or_else(|| "-".to_string());
    tracing::info_span!(
        "request",
        method = %req.method(),
        uri = %req.uri(),
        client_ip = %client_ip,
    )
}

/// Response logger for `TraceLayer`: logs status and latency at INFO.
pub fn log_response(res: &Response, latency: Duration, _span: &Span) {
    tracing::info!(
        status = res.status().as_u16(),
        latency_ms = latency.as_millis(),
        "response sent"
    );
}
