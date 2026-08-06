//! Helpers shared by the integration test binaries.
//!
//! Each binary uses a different subset — the request-path tests never touch the
//! log recorder, the index tests never build a router from a seeded index — so
//! dead code is expected here rather than a sign of something left behind.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::{body::Body, http::Request, response::Response};
use bytes::Bytes;
use http_body_util::BodyExt;
use tower::ServiceExt;

use blaue_tonne_rust::index::DistrictIndex;
use blaue_tonne_rust::pdf_parser::index_districts;
use blaue_tonne_rust::{AppState, build_router};

/// Pages of the fixture that carry the district tables — the same value the
/// production `plans.yaml` uses.
pub const FIXTURE_PAGES: &str = "1,2";

pub fn fixture_pdf_bytes() -> Bytes {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lk_rosenheim_2026.pdf");
    Bytes::from(std::fs::read(&path).expect("fixture PDF not found"))
}

/// The index the real startup path would produce for the fixture plan, without
/// going through the network.
pub fn fixture_index() -> DistrictIndex {
    DistrictIndex::from_pairs(
        index_districts(&fixture_pdf_bytes(), FIXTURE_PAGES).expect("fixture must parse"),
    )
}

pub fn state_from_fixture() -> AppState {
    AppState::from_index(fixture_index())
}

pub async fn body_to_json(response: Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

pub async fn get(state: AppState, path: &str) -> Response {
    let app = build_router(state, vec![]);
    app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Log capture, for the tests that assert on the internal detail `AppError`
// deliberately keeps out of any response.
// ---------------------------------------------------------------------------

/// One recorded log event, flattened to `level` plus the concatenated text of
/// every field — the `error` field matters as much as `message` here.
#[derive(Clone, Debug)]
struct LoggedEvent {
    level: tracing::Level,
    text: String,
}

#[derive(Clone, Default)]
pub struct EventRecorder(Arc<Mutex<Vec<LoggedEvent>>>);

static INIT: std::sync::Once = std::sync::Once::new();

/// Installs a permissive global subscriber, once per test binary.
///
/// Required for the thread-local recorder below to see anything at all.
/// `tracing` caches each callsite's `Interest` globally, and with no global
/// subscriber that interest is computed against `NoSubscriber` — the first
/// thread to reach a `warn!` marks the callsite "never", and every later
/// thread-local recorder is skipped before the dispatcher is even consulted.
/// Since tests run in parallel, that made these assertions pass or fail
/// depending on which test happened to touch the callsite first.
fn init_global_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_test_writer()
            .init();
    });
}

impl EventRecorder {
    /// Installs the recorder for the current thread. The returned guard must be
    /// held for as long as events should be captured.
    ///
    /// Thread-local, and `#[tokio::test]` is current-thread, so this only sees
    /// what happens on the calling thread. Fine for the paths below: the index
    /// build logs every skip and every fault before handing bytes to
    /// `spawn_blocking`.
    pub fn install() -> (Self, tracing::subscriber::DefaultGuard) {
        use tracing_subscriber::layer::SubscriberExt;
        init_global_tracing();
        let recorder = Self::default();
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(recorder.clone()));
        (recorder, guard)
    }

    pub fn at(&self, level: tracing::Level) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.level == level)
            .map(|e| e.text.clone())
            .collect()
    }
}

#[derive(Default)]
struct FieldCollector(String);

impl tracing::field::Visit for FieldCollector {
    fn record_debug(&mut self, _field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{value:?} "));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventRecorder {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        self.0.lock().unwrap().push(LoggedEvent {
            level: *event.metadata().level(),
            text: fields.0,
        });
    }
}
