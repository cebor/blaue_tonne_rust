use std::net::SocketAddr;
use std::sync::{Arc, Once};

use axum::{
    Extension, Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    response::Response,
    routing::get,
};
use http_body_util::BodyExt;
use ipnet::IpNet;
use tower::ServiceExt;
use tracing::Level;

use blaue_tonne_rust::ResolvedClientIp;
use blaue_tonne_rust::index::DistrictIndex;
use blaue_tonne_rust::middleware::{make_request_span, resolve_client_ip};
use blaue_tonne_rust::{AppState, build_router};

// ---------------------------------------------------------------------------
// resolve_client_ip — exercised via a mini-router that echoes the resolved IP
// ---------------------------------------------------------------------------

async fn echo_ip(Extension(ip): Extension<ResolvedClientIp>) -> String {
    ip.0.to_string()
}

fn router(allow: Vec<IpNet>) -> Router {
    let allow = Arc::new(allow);
    Router::new()
        .route("/", get(echo_ip))
        .layer(from_fn_with_state(allow, resolve_client_ip))
}

async fn body_to_string(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn ip(s: &str) -> IpNet {
    s.parse::<std::net::IpAddr>().unwrap().into()
}

#[tokio::test]
async fn test_resolve_ip_no_connect_info_falls_back_to_localhost() {
    let response = router(vec![])
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "127.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_proxy_uses_leftmost_xff() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let response = router(vec![ip("10.0.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "1.2.3.4");
}

#[tokio::test]
async fn test_resolve_ip_untrusted_proxy_uses_peer() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    // allowlist does NOT contain the peer → XFF is ignored.
    let response = router(vec![ip("192.168.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "1.2.3.4")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "10.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_proxy_broken_xff_falls_back_to_peer() {
    let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let response = router(vec![ip("10.0.0.1")])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "garbage-not-an-ip")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "10.0.0.1");
}

#[tokio::test]
async fn test_resolve_ip_trusted_via_cidr() {
    let peer: SocketAddr = "10.5.6.7:5000".parse().unwrap();
    let response = router(vec!["10.0.0.0/8".parse().unwrap()])
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "9.9.9.9")
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_to_string(response).await, "9.9.9.9");
}

// ---------------------------------------------------------------------------
// make_request_span — records the resolved client IP on the span
// ---------------------------------------------------------------------------

static INIT: Once = Once::new();

/// Installs a permissive global subscriber, once per test binary.
///
/// Not optional for `record_trace`: `tracing` caches callsite `Interest`
/// globally, and without a global subscriber it is computed against
/// `NoSubscriber` and cached as "never" — the thread-local recorder would then
/// be skipped before the dispatcher is consulted, non-deterministically,
/// depending on which parallel test reached the callsite first.
fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(Level::TRACE)
            .with_test_writer()
            .init();
    });
}

#[test]
fn test_make_request_span_is_info() {
    init_tracing();
    let req = Request::builder()
        .uri("/lk_rosenheim")
        .body(Body::empty())
        .unwrap();
    let span = make_request_span(&req);
    assert_eq!(span.metadata().unwrap().level(), &Level::INFO);
}

// ---------------------------------------------------------------------------
// /health is registered outside the traced router, so it must not produce a
// request span — this is what keeps high-frequency health checks out of the
// logs, and it is the property that would silently regress if someone moved
// the route back inside the layered block in build_router.
// ---------------------------------------------------------------------------

/// One recorded event: the target it was emitted under and its message.
#[derive(Clone, Debug, PartialEq)]
struct RecordedEvent {
    target: String,
    message: String,
}

/// Records "request" spans and every event emitted while this is the active
/// subscriber.
#[derive(Clone, Default)]
struct TraceRecorder {
    spans: Arc<std::sync::atomic::AtomicUsize>,
    events: Arc<std::sync::Mutex<Vec<RecordedEvent>>>,
}

impl TraceRecorder {
    fn span_count(&self) -> usize {
        self.spans.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn events(&self) -> Vec<RecordedEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// Pulls the `message` field out of an event.
#[derive(Default)]
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TraceRecorder {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if attrs.metadata().name() == "request" {
            self.spans
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(RecordedEvent {
            target: event.metadata().target().to_string(),
            message: visitor.0,
        });
    }
}

/// Drives one request with `recorder` installed as the thread-local subscriber.
///
/// Relies on `#[tokio::test]`'s current-thread runtime: `set_default` is
/// thread-local, so anything the request does on another thread (a
/// `multi_thread` flavour, say) would not be recorded. Keep the URIs here to
/// routes that stay on the calling thread.
async fn record_trace(uri: &str) -> TraceRecorder {
    use tracing_subscriber::layer::SubscriberExt;

    // Called here explicitly rather than relying on another test in this binary
    // reaching it first: tests run in parallel, so that ordering is not a
    // guarantee, and without a global subscriber the callsite's `Interest` is
    // cached as "never" and the thread-local recorder never sees it.
    init_tracing();

    let recorder = TraceRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let app = build_router(AppState::from_index(DistrictIndex::default()), vec![]);
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{uri} did not return 200"
    );

    recorder
}

#[tokio::test]
async fn test_health_produces_no_request_span() {
    assert_eq!(record_trace("/health").await.span_count(), 0);
}

#[tokio::test]
async fn test_traced_route_produces_a_request_span() {
    // Control for the test above: without this, the assertion would also hold
    // if tracing were broken everywhere.
    assert_eq!(record_trace("/docs/openapi.json").await.span_count(), 1);
}

#[tokio::test]
async fn test_response_is_logged_under_this_crates_target() {
    // Regression guard: tower-http's DefaultOnResponse emits under the
    // `tower_http` target, which the default RUST_LOG fallback in main.rs
    // (`blaue_tonne_rust=info`) filters out. Swapping middleware::log_response
    // for it makes request logging vanish in production while every other test
    // still passes.
    let events = record_trace("/docs/openapi.json").await.events();

    // Match on the response event specifically, not just "some event from this
    // crate" — otherwise any unrelated log line added later would satisfy this.
    assert!(
        events
            .iter()
            .any(|e| e.target.starts_with("blaue_tonne_rust") && e.message == "response sent"),
        "no \"response sent\" event under this crate's target, got: {events:?}"
    );
}
