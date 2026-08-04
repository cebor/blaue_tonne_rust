use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use bytes::Bytes;
use chrono::NaiveDate;
use http_body_util::BodyExt;
use tower::ServiceExt;

use blaue_tonne_rust::config::Plan;
use blaue_tonne_rust::pdf_parser::normalize_district;
use blaue_tonne_rust::{AppState, build_router};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_dates(district: &str) -> Option<Vec<NaiveDate>> {
    match district {
        "Kolbermoor" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 15).unwrap(),
        ]),
        "Bad Aibling" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
        ]),
        "Prien a. Chiemsee" => Some(vec![
            NaiveDate::from_ymd_opt(2026, 1, 25).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 25).unwrap(),
        ]),
        "Aschau" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 10).unwrap()]),
        "Bruckmühl 1" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 11).unwrap()]),
        "Feldkirchen 2" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 12).unwrap()]),
        "Raubling 3" => Some(vec![NaiveDate::from_ymd_opt(2026, 1, 13).unwrap()]),
        _ => None,
    }
}

fn state_with_cached_dates(district: &str, dates: Vec<NaiveDate>) -> AppState {
    let state = AppState::new(vec![]);
    // The handler keys the cache on the normalized name, so seeding has to use
    // the same form.
    state
        .dates_cache
        .insert(normalize_district(district), dates);
    state
}

fn fixture_pdf_bytes() -> Bytes {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lk_rosenheim_2026.pdf");
    Bytes::from(std::fs::read(&path).expect("fixture PDF not found"))
}

fn state_with_fixture_pdf() -> AppState {
    let pdf_bytes = fixture_pdf_bytes();

    let plan = Plan {
        url: "https://fake.test/schedule.pdf".to_string(),
        pages: "1,2".to_string(),
    };
    let state = AppState::new(vec![plan]);
    state
        .pdf_cache
        .insert("https://fake.test/schedule.pdf".to_string(), pdf_bytes);
    state
}

async fn body_to_json(response: Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(state: AppState, path: &str) -> Response {
    let app = build_router(state, vec![]);
    app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_check() {
    let response = get(AppState::new(vec![]), "/health").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert_eq!(body["status"], "healthy");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – valid district (pre-cached)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_valid_district_from_cache() {
    let dates = fake_dates("Kolbermoor").unwrap();
    let state = state_with_cached_dates("Kolbermoor", dates);

    let response = get(state, "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr[0].as_str().unwrap().starts_with("2026-01-15"));
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – invalid district returns 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_invalid_district_returns_404() {
    let state = state_with_fixture_pdf();

    let response = get(state, "/lk_rosenheim?district=NonExistentDistrict").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "District not found");
}

// ---------------------------------------------------------------------------
// GET /lk_rosenheim – missing query param returns 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_missing_district_parameter_returns_400() {
    let response = get(AppState::new(vec![]), "/lk_rosenheim").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // The 400 must carry the same `ErrorDetail` shape as every other error —
    // axum's own QueryRejection would be plain text, which is what the handler
    // maps away. Regression guard for the documented response schema.
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
}

// ---------------------------------------------------------------------------
// Cache: second request re-uses cached result (no PDF re-parse)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cache_prevents_repeated_pdf_parsing() {
    let dates = fake_dates("Bad Aibling").unwrap();
    let state = state_with_cached_dates("Bad Aibling", dates.clone());
    let app = build_router(state.clone(), vec![]);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Bad+Aibling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let d1 = body_to_json(r1).await;

    // Manually confirm cache has the entry (under the normalized key)
    assert!(state.dates_cache.contains_key("BadAibling"));

    let r2 = app
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Bad+Aibling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let d2 = body_to_json(r2).await;

    assert_eq!(d1, d2);
}

// ---------------------------------------------------------------------------
// Cache: two different districts have separate entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_districts_separate_cache_entries() {
    let state = AppState::new(vec![]);
    state.dates_cache.insert(
        normalize_district("Kolbermoor"),
        fake_dates("Kolbermoor").unwrap(),
    );
    state.dates_cache.insert(
        normalize_district("Prien a. Chiemsee"),
        fake_dates("Prien a. Chiemsee").unwrap(),
    );
    let app = build_router(state, vec![]);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Kolbermoor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let r2 = app
        .oneshot(
            Request::builder()
                .uri("/lk_rosenheim?district=Prien+a.+Chiemsee")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(r2.status(), StatusCode::OK);

    let d1 = body_to_json(r1).await;
    let d2 = body_to_json(r2).await;
    assert_ne!(d1, d2);
}

// ---------------------------------------------------------------------------
// Cache key: whitespace variants of a district name share one entry.
// Matching strips whitespace, so without a normalized key every spelling of
// the same district would allocate its own entry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_whitespace_variants_share_one_cache_entry() {
    let state = state_with_fixture_pdf();
    let app = build_router(state.clone(), vec![]);

    for spelling in [
        "Bad+Aibling",
        "BadAibling",
        "B+a+d++Aibling",
        "%20Bad%20Aibling%20",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/lk_rosenheim?district={spelling}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "spelling {spelling:?}");
    }

    assert_eq!(
        state.dates_cache.len(),
        1,
        "expected a single cache entry, got: {:?}",
        state
            .dates_cache
            .iter()
            .map(|e| e.key().clone())
            .collect::<Vec<_>>()
    );
    assert!(state.dates_cache.contains_key("BadAibling"));
}

// ---------------------------------------------------------------------------
// No plans configured → 503, not 404. A service with nothing to search cannot
// have established that the district is missing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_no_plans_returns_503() {
    let response = get(AppState::new(vec![]), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// An empty or whitespace-only district is rejected before any work happens.
//
// Both normalize to "", which is not a name: it would match any row whose cells
// are whitespace only, and finding that out would cost a download and a parse of
// every plan. The plan below points at a host that never resolves, so if the
// guard is ever removed these turn into a network error instead of a 400.
// ---------------------------------------------------------------------------

fn state_with_unreachable_plan() -> AppState {
    AppState::new(vec![Plan {
        url: "http://nonexistent.invalid/schedule.pdf".to_string(),
        pages: "1".to_string(),
    }])
}

#[tokio::test]
async fn test_empty_district_returns_400() {
    let response = get(state_with_unreachable_plan(), "/lk_rosenheim?district=").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["detail"], "Invalid or missing query parameter");
}

#[tokio::test]
async fn test_whitespace_only_district_returns_400() {
    let response = get(
        state_with_unreachable_plan(),
        "/lk_rosenheim?district=%20%20%09",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Bad plan URL → 503: the URL comes from plans.yaml, so this is a server-side
// fault and must not be reported as a client error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalid_pdf_url_returns_503() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/not-a-pdf")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html><body>Not a PDF</body></html>")
        .create_async()
        .await;

    let upstream_url = format!("{}/not-a-pdf", mock_server.url());
    let plan = Plan {
        url: upstream_url.clone(),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    let detail = body["detail"].as_str().unwrap();
    // Only the leak matters here; pinning the exact wording as well would make
    // this assertion unreachable and the guard decorative.
    assert!(
        !detail.contains(&upstream_url),
        "response leaked the upstream URL: {detail}"
    );
    assert!(
        !detail.to_lowercase().contains("pdf"),
        "response disclosed the data format: {detail}"
    );
}

// ---------------------------------------------------------------------------
// Parametrized: districts with numbers in names (via fixture PDF)
// ---------------------------------------------------------------------------

macro_rules! api_district_test {
    ($name:ident, $district:expr) => {
        #[tokio::test]
        async fn $name() {
            let dates = fake_dates($district)
                .unwrap_or_else(|| vec![chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()]);
            let state = state_with_cached_dates($district, dates);
            let encoded = urlencoding::encode($district);
            let response = get(state, &format!("/lk_rosenheim?district={}", encoded)).await;

            assert_eq!(
                response.status(),
                StatusCode::OK,
                "district '{}' failed",
                $district
            );
            let body = body_to_json(response).await;
            assert!(
                !body.as_array().unwrap().is_empty(),
                "no dates for district '{}'",
                $district
            );
        }
    };
}

api_district_test!(test_api_aschau, "Aschau");
api_district_test!(test_api_bruckmuhl_1, "Bruckmühl 1");
api_district_test!(test_api_feldkirchen_2, "Feldkirchen 2");
api_district_test!(test_api_raubling_3, "Raubling 3");

// ---------------------------------------------------------------------------
// Multi-plan: district missing in the first plan must not 404 — the handler
// has to continue with the remaining plans
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_district_only_in_later_plan_is_found() {
    let pdf = fixture_pdf_bytes();
    // Premise: "Vogtareuth" is not on page 1 of the fixture, so plan 1 yields
    // DistrictNotFound and only plan 2 (pages 1,2) can find it.
    assert!(matches!(
        blaue_tonne_rust::pdf_parser::get_dates(&pdf, "1", "Vogtareuth"),
        Err(blaue_tonne_rust::errors::AppError::DistrictNotFound)
    ));

    let url = "https://fake.test/schedule.pdf".to_string();
    let plans = vec![
        Plan {
            url: url.clone(),
            pages: "1".to_string(),
        },
        Plan {
            url: url.clone(),
            pages: "1,2".to_string(),
        },
    ];
    let state = AppState::new(plans);
    state.pdf_cache.insert(url, pdf);

    let response = get(state, "/lk_rosenheim?district=Vogtareuth").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// download_pdf: full real fetch path via mockito (serves the fixture PDF)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_download_pdf_full_fetch_success() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/schedule.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/schedule.pdf", mock_server.url()),
        pages: "1,2".to_string(),
    };
    let url = plan.url.clone();
    let state = AppState::new(vec![plan]);

    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());

    // The fetched PDF bytes were cached under the URL.
    assert!(state.pdf_cache.contains_key(&url));
}

// ---------------------------------------------------------------------------
// get_dates success path against the pre-cached fixture PDF (no network)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_dates_from_fixture_caches_result() {
    let state = state_with_fixture_pdf();
    assert!(!state.dates_cache.contains_key("Kolbermoor"));

    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());

    // Handler stored the parsed dates in the cache.
    assert!(state.dates_cache.contains_key("Kolbermoor"));
}

// ---------------------------------------------------------------------------
// No plan could be read → 503, never 404.
//
// "District not found" asserts that we looked and it was not there. If the only
// plan was unreachable we never looked at all, so claiming absence would be a
// lie the caller cannot distinguish from a genuine miss.
//
// The fault here is deliberately a 503 and not a 404: an upstream 404 means the
// plan is *gone*, which is expected and permanent — see the test below.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unreadable_only_plan_returns_503_not_404() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/flaky.pdf")
        .with_status(503)
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/flaky.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an unread plan must not be reported as a missing district"
    );
}

// ---------------------------------------------------------------------------
// A retired plan (upstream 404) does not mask the 404 — as long as some *other*
// plan was actually read.
//
// Last year's PDF goes offline while it is still listed in plans.yaml, and stays
// offline until someone prunes the config. Counting that as "we did not look"
// would make "District not found" unreachable for weeks — every typo would get a
// 503 telling the caller to retry something that will never start working.
//
// But when the retired plan was the *only* one, nothing was read at all, and a
// 404 would be a claim about data nobody looked at. That case is a 503: a fully
// stale plans.yaml has to be visible in the status code and the 5xx rate, not
// answer "District not found" for every district with nothing above DEBUG in
// the log.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_only_plan_retired_returns_503() {
    let mut mock_server = mockito::Server::new_async().await;
    let _gone = mock_server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/last-year.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=NonExistentDistrict",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "no plan was readable, so nothing supports a 'not found'"
    );
    assert_eq!(
        body_to_json(response).await["detail"],
        "Service temporarily unavailable, please try again later"
    );
}

#[tokio::test]
async fn test_retired_plan_404_does_not_mask_a_genuine_404() {
    let mut mock_server = mockito::Server::new_async().await;
    let _gone = mock_server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;
    let _current = mock_server
        .mock("GET", "/this-year.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/last-year.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/this-year.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];

    let response = get(
        AppState::new(plans),
        "/lk_rosenheim?district=NonExistentDistrict",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a retired plan must not turn a genuine miss into a 503"
    );
    assert_eq!(body_to_json(response).await["detail"], "District not found");
}

// ---------------------------------------------------------------------------
// Log-capturing helpers, used by the tests that assert on the internal detail
// `AppError` deliberately keeps out of the response body.
// ---------------------------------------------------------------------------

/// One recorded log event, flattened to `level` plus the concatenated text of
/// every field. Several tests here assert on the *internal* detail that
/// `AppError` keeps out of the response body but writes to the log, so the
/// `error` field matters as much as `message`.
#[derive(Clone, Debug)]
struct LoggedEvent {
    level: tracing::Level,
    text: String,
}

#[derive(Clone, Default)]
struct EventRecorder(Arc<Mutex<Vec<LoggedEvent>>>);

static INIT: std::sync::Once = std::sync::Once::new();

/// Installs a permissive global subscriber, once per test binary.
///
/// Required for the thread-local recorders below to see anything at all.
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
    /// what the request does on the calling thread. Fine for the paths below,
    /// which all log before reaching `spawn_blocking`.
    fn install() -> (Self, tracing::subscriber::DefaultGuard) {
        use tracing_subscriber::layer::SubscriberExt;
        init_global_tracing();
        let recorder = Self::default();
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(recorder.clone()));
        (recorder, guard)
    }

    fn at(&self, level: tracing::Level) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// A plan that is online but momentarily unreadable must not take down requests
// another plan can answer. Only a 404 used to be survivable here — a 503, a
// reset connection or a timeout killed the whole request even when the district
// sat in an already-cached plan.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_transiently_broken_plan_does_not_break_remaining_plan() {
    let mut mock_server = mockito::Server::new_async().await;
    // Not a 404: this plan exists, the server is just having a bad moment.
    let _flaky = mock_server
        .mock("GET", "/flaky.pdf")
        .with_status(503)
        .create_async()
        .await;
    let _good = mock_server
        .mock("GET", "/good.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/flaky.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/good.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];

    let (recorder, _guard) = EventRecorder::install();
    let response = get(AppState::new(plans), "/lk_rosenheim?district=Kolbermoor").await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a transiently broken plan must be skipped, not fail the request"
    );
    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());

    // The skip is silent to the caller but must be visible to the operator.
    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("skipping unreadable plan")),
        "the skipped plan was not logged, got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// ...but the answer assembled while a plan was skipped must not be cached.
//
// `dates_cache` has no expiry. Caching a result that is missing the skipped
// plan's dates would freeze a momentary upstream blip in for the lifetime of
// the process: the district would keep answering with half its dates long after
// the plan recovered, and nothing would ever re-read it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_partial_result_from_a_skipped_plan_is_not_cached() {
    let mut mock_server = mockito::Server::new_async().await;
    let _flaky = mock_server
        .mock("GET", "/flaky.pdf")
        .with_status(503)
        .create_async()
        .await;
    let _good = mock_server
        .mock("GET", "/good.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/flaky.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/good.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];
    let state = AppState::new(plans);

    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!body_to_json(response).await.as_array().unwrap().is_empty());

    assert!(
        state.dates_cache.is_empty(),
        "an incomplete answer was cached: {:?}",
        state
            .dates_cache
            .iter()
            .map(|e| e.key().clone())
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Same for a plan that downloads fine but cannot be parsed. The bytes land in
// `pdf_cache`, so hard-failing here would make the 500 permanent for the
// process — every district, not just this one, for as long as it runs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unparseable_plan_does_not_break_remaining_plan() {
    let mut mock_server = mockito::Server::new_async().await;
    // Served as a PDF, but the bytes are not one — download succeeds, get_dates
    // fails with PdfError.
    let _corrupt = mock_server
        .mock("GET", "/corrupt.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("this is not a PDF at all")
        .create_async()
        .await;
    let _good = mock_server
        .mock("GET", "/good.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/corrupt.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/good.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];

    let (recorder, _guard) = EventRecorder::install();
    let response = get(AppState::new(plans), "/lk_rosenheim?district=Kolbermoor").await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an unparseable plan must be skipped, not fail the request"
    );
    assert!(!body_to_json(response).await.as_array().unwrap().is_empty());

    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("skipping unparseable plan")),
        "the skipped plan was not logged, got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// Same for a plan whose host does not resolve at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unreachable_plan_does_not_break_remaining_plan() {
    let mut mock_server = mockito::Server::new_async().await;
    let _good = mock_server
        .mock("GET", "/good.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        // ".invalid" never resolves (RFC 2606) → send error, not a timeout.
        Plan {
            url: "https://nonexistent.invalid/plan.pdf".to_string(),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/good.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];

    let response = get(AppState::new(plans), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!body_to_json(response).await.as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// download_pdf: upstream non-2xx (500) → 503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_server_error_returns_503() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/broken.pdf")
        .with_status(500)
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/broken.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// download_pdf: configured URL not ending in .pdf → 503 (no network)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_non_pdf_url_returns_503() {
    let plan = Plan {
        url: "http://example.test/not-a-pdf-file".to_string(),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    assert_eq!(
        body["detail"],
        "Service temporarily unavailable, please try again later"
    );
}

// ---------------------------------------------------------------------------
// download_pdf: .pdf URL but wrong content-type → 503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_url_wrong_content_type_returns_503() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/fake.pdf")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html>not a pdf</html>")
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/fake.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_to_json(response).await;
    assert_eq!(
        body["detail"],
        "Service temporarily unavailable, please try again later"
    );
}

// ---------------------------------------------------------------------------
// download_pdf: connection error (unresolvable host) → 503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pdf_connection_error_returns_503() {
    // ".invalid" never resolves (RFC 2606) → immediate DNS/send error, not a
    // timeout. (A closed port is not reliable here: some environments drop
    // instead of refusing, which turns the test into a 30 s timeout / 504.)
    let plan = Plan {
        url: "http://nonexistent.invalid/schedule.pdf".to_string(),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// Turn of the year: plans are published yearly, so for a few weeks plans.yaml
// lists both the old and the new PDF, and at some point the old one 404s. That
// must not fail the request — the surviving plan still answers it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retired_plan_404_does_not_break_remaining_plan() {
    let mut mock_server = mockito::Server::new_async().await;
    let _gone = mock_server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;
    let _current = mock_server
        .mock("GET", "/this-year.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/last-year.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
        Plan {
            url: format!("{}/this-year.pdf", mock_server.url()),
            pages: "1,2".to_string(),
        },
    ];

    let response = get(AppState::new(plans), "/lk_rosenheim?district=Kolbermoor").await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a retired plan must be skipped, not fail the request"
    );
    let body = body_to_json(response).await;
    assert!(!body.as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// download_pdf: oversized body → 503, and nothing is cached.
//
// The cap in download.rs has two independent guards, and they need separate
// tests: a Content-Length pre-check, and an accumulating check inside the read
// loop. A single test with a large fixed body only ever reaches the first one —
// mockito sets Content-Length for it, so the loop never runs.
//
// Both guards produce the same 503 and the same client message, so the status
// code alone cannot tell them apart: each test would still pass with its own
// guard removed. The internal detail that `AppError` writes to the log is what
// distinguishes them, so that is what these assert on.
// ---------------------------------------------------------------------------

/// Mirrors `MAX_PDF_BYTES` in `src/download.rs`. Kept as a literal rather than
/// exported: the constant is internal, and widening the crate's public API for
/// a test would undo the module-visibility narrowing.
const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;

#[tokio::test]
async fn test_oversized_pdf_content_length_returns_503() {
    // A fixed body gets an honest Content-Length from the mock server, so the
    // pre-check sees the real size and rejects before reading anything. (Faking
    // a mismatched header is not an option — hyper panics on a payload that
    // disagrees with its declared length.)
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/huge.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(vec![b'x'; MAX_PDF_BYTES + 1])
        .create_async()
        .await;

    let url = format!("{}/huge.pdf", mock_server.url());
    let plan = Plan {
        url: url.clone(),
        pages: "1".to_string(),
    };
    let state = AppState::new(vec![plan]);

    let (recorder, _guard) = EventRecorder::install();
    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    // The rejected body must not end up in the cache.
    assert!(!state.pdf_cache.contains_key(&url));

    // Pin the pre-check specifically. Without this the test would still pass
    // with the pre-check removed — the in-loop cap would catch the same body
    // and return the same 503. The log detail is what tells them apart.
    let errors = recorder.at(tracing::Level::ERROR);
    assert!(
        errors.iter().any(|e| e.contains("advertises")),
        "expected the content-length pre-check to reject, got: {errors:?}"
    );
}

#[tokio::test]
async fn test_oversized_chunked_pdf_returns_503() {
    // Chunked transfer → no Content-Length, so the pre-check cannot fire and
    // the in-loop cap is the only thing standing between a hostile upstream and
    // unbounded memory growth. This is the path the pre-check test cannot reach.
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/huge.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_chunked_body(|w| {
            let chunk = vec![b'x'; 1024 * 1024];
            // One MiB past the cap, so the limit is crossed mid-stream.
            for _ in 0..(MAX_PDF_BYTES / chunk.len()) + 1 {
                w.write_all(&chunk)?;
            }
            Ok(())
        })
        .create_async()
        .await;

    let url = format!("{}/huge.pdf", mock_server.url());
    let plan = Plan {
        url: url.clone(),
        pages: "1".to_string(),
    };
    let state = AppState::new(vec![plan]);

    let (recorder, _guard) = EventRecorder::install();
    let response = get(state.clone(), "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!state.pdf_cache.contains_key(&url));

    let errors = recorder.at(tracing::Level::ERROR);
    assert!(
        errors.iter().any(|e| e.contains("exceeds the")),
        "expected the in-loop cap to reject, got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// get_dates: valid download but unparseable PDF bytes → 500 (our own parse
// failed, so this one stays a server error rather than a gateway error)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_corrupt_pdf_returns_500() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/corrupt.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("not actually a pdf")
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/corrupt.pdf", mock_server.url()),
        pages: "1".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// When several plans fail, the fault that reaches the caller is the one that
// says the most — not whichever happened first.
//
// A `PdfError` is our own bug and has to stay a 500. Remembering only the first
// fault would let a flaky upstream on an earlier plan downgrade it to a 503,
// i.e. silently turn "we are broken" into "come back later".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_parse_error_outranks_an_earlier_upstream_fault() {
    let mut mock_server = mockito::Server::new_async().await;
    // First plan: online but 5xx-ing → Upstream (503).
    let _flaky = mock_server
        .mock("GET", "/flaky.pdf")
        .with_status(503)
        .create_async()
        .await;
    // Second plan: downloads fine, but the bytes are not a PDF → PdfError (500).
    let _corrupt = mock_server
        .mock("GET", "/corrupt.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("not actually a pdf")
        .create_async()
        .await;

    let plans = vec![
        Plan {
            url: format!("{}/flaky.pdf", mock_server.url()),
            pages: "1".to_string(),
        },
        Plan {
            url: format!("{}/corrupt.pdf", mock_server.url()),
            pages: "1".to_string(),
        },
    ];
    let response = get(
        AppState::new(plans),
        "/lk_rosenheim?district=NonExistentDistrict",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "the parse failure must not be masked by the earlier upstream fault"
    );
}

// ---------------------------------------------------------------------------
// A plan URL may carry a query string. The `.pdf` check looks at the path, so
// a cache-busting `?v=…` on an otherwise valid link is not a config error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_plan_url_with_query_string_is_fetched() {
    let mut mock_server = mockito::Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/schedule.pdf?v=2")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .create_async()
        .await;

    let plan = Plan {
        url: format!("{}/schedule.pdf?v=2", mock_server.url()),
        pages: "1,2".to_string(),
    };
    let response = get(
        AppState::new(vec![plan]),
        "/lk_rosenheim?district=Kolbermoor",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!body_to_json(response).await.as_array().unwrap().is_empty());
}
