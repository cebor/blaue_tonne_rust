//! Building the index at startup.
//!
//! This is where every fault involving the plan source lands. The rule they all
//! follow: the process either serves a complete index or refuses to start.
//! There is no second attempt at request time, so a half-built index would
//! quietly serve a district short of its dates for as long as the process runs
//! — and nobody would see it.

use axum::http::StatusCode;

use blaue_tonne_rust::AppState;
use blaue_tonne_rust::cache::PdfCache;
use blaue_tonne_rust::errors::PlanError;
use blaue_tonne_rust::index::build_index;
use blaue_tonne_rust::pdf_parser::index_districts;

mod common;
use common::{
    EventRecorder, FIXTURE_PAGES, body_to_json, fixture_pdf_bytes, get, mock_fixture, plan,
};

// ---------------------------------------------------------------------------
// The happy path: one plan, every district in it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_index_is_built_from_the_fetched_plan() {
    let mut server = mockito::Server::new_async().await;
    let _mock = mock_fixture(&mut server, "/schedule.pdf").await;

    let index = build_index(
        &[plan(
            format!("{}/schedule.pdf", server.url()),
            FIXTURE_PAGES,
        )],
        &PdfCache::disabled(),
    )
    .await
    .expect("the fixture plan must index");

    assert!(index.lookup("Kolbermoor").is_some_and(|d| !d.is_empty()));
    assert!(index.lookup("BadAibling").is_some_and(|d| !d.is_empty()));
    assert!(index.lookup("NonExistentDistrict").is_none());

    // The count `main` logs on startup has to mean something: all 50 districts
    // of the fixture (`DISTRICTS` in test_pdf_parser.rs) are in there.
    assert!(!index.is_empty());
    assert!(
        index.len() >= 50,
        "expected at least the 50 known districts, got {}",
        index.len()
    );
}

// ---------------------------------------------------------------------------
// The point of the whole design: the source is read once, and an unknown
// district costs nothing afterwards.
//
// This is what keeps a caller from driving work at the source: with a fetch on
// the request path, a loop of random names — none of them in any plan — would
// re-download and re-parse every PDF, at a cost bounded by nothing the service
// controls. Here it is five requests, four misses, and one fetch.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_source_is_read_once_and_never_again() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/schedule.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .expect(1)
        .create_async()
        .await;

    let state = AppState::build(
        &[plan(
            format!("{}/schedule.pdf", server.url()),
            FIXTURE_PAGES,
        )],
        &PdfCache::disabled(),
    )
    .await
    .expect("the fixture plan must index");

    for name in ["Nirgendwo", "xyzzy", "Kolbermoor", "42", "Nirgendwo"] {
        let response = get(state.clone(), &format!("/lk_rosenheim?district={name}")).await;
        assert!(
            response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND,
            "district {name:?} answered {}",
            response.status()
        );
    }

    // Exactly one fetch, for five requests of which four miss.
    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Faults that refuse the start.
//
// Each asserts on the variant *and* on the internal detail: the variant decides
// what an operator sees in the log, and the detail is the only thing that says
// which guard fired.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_upstream_server_error_refuses_to_start() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/broken.pdf")
        .with_status(500)
        .create_async()
        .await;

    let result = build_index(
        &[plan(format!("{}/broken.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("500")),
        "expected an upstream fault, got: {result:?}"
    );
}

#[tokio::test]
async fn test_wrong_content_type_refuses_to_start() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/fake.pdf")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html>not a pdf</html>")
        .create_async()
        .await;

    let result = build_index(
        &[plan(format!("{}/fake.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("text/html")),
        "expected the content-type guard to reject, got: {result:?}"
    );
}

// A plan URL that does not point at a .pdf path is rejected while `plans.yaml`
// is read, not while it is fetched, so it is not a fault this file covers —
// `test_load_plans_rejects_non_pdf_url` in `test_config.rs` owns it.

#[tokio::test]
async fn test_unreachable_host_refuses_to_start() {
    // ".invalid" never resolves (RFC 2606) → an immediate send error rather than
    // a 30 s timeout.
    let result = build_index(
        &[plan(
            "http://nonexistent.invalid/schedule.pdf".to_string(),
            "1",
        )],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(_))),
        "expected a transport fault, got: {result:?}"
    );
}

#[tokio::test]
async fn test_corrupt_pdf_refuses_to_start() {
    // Downloads fine, but the bytes are not a PDF. `PlanError` has no variant
    // separating "we could not fetch it" from "we could not read what we
    // fetched" — every startup fault is handled identically — so the assertion
    // is on the message, which is the only thing that tells the two apart in a
    // log.
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/corrupt.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("not actually a pdf")
        .create_async()
        .await;

    let result = build_index(
        &[plan(format!("{}/corrupt.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("cross-reference")),
        "expected the parse to fail, not the fetch, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// The download cap has two independent guards — a Content-Length pre-check and
// an accumulating check inside the read loop — and they need separate tests: a
// single large fixed body only ever reaches the first, because mockito sets
// Content-Length for it and the loop never runs.
//
// Both produce the same `PlanError::Failed` variant — as does every other
// startup fault — so the variant alone cannot tell them apart: each test would
// still pass with its own guard removed. The message is what distinguishes
// them, so that is what these assert on.
// ---------------------------------------------------------------------------

/// Mirrors `MAX_PDF_BYTES` in `src/download.rs`. Kept as a literal rather than
/// exported: the constant is internal, and widening the crate's public API for
/// a test would undo the module-visibility narrowing.
const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;

#[tokio::test]
async fn test_oversized_pdf_content_length_refuses_to_start() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/huge.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(vec![b'x'; MAX_PDF_BYTES + 1])
        .create_async()
        .await;

    let result = build_index(
        &[plan(format!("{}/huge.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("advertises")),
        "expected the content-length pre-check to reject, got: {result:?}"
    );
}

#[tokio::test]
async fn test_oversized_chunked_pdf_refuses_to_start() {
    // Chunked transfer → no Content-Length, so the pre-check cannot fire and the
    // in-loop cap is the only thing standing between a hostile upstream and
    // unbounded memory growth.
    let mut server = mockito::Server::new_async().await;
    let _mock = server
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

    let result = build_index(
        &[plan(format!("{}/huge.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("exceeds the")),
        "expected the in-loop cap to reject, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Turn of the year: plans are published yearly, so for a few weeks plans.yaml
// lists both the old and the new PDF, and at some point the old one 404s. That
// is expected and permanent until someone prunes the config, so it must not
// keep the service down — it is skipped, and said so once in the log.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retired_plan_is_skipped_with_a_warning() {
    let mut server = mockito::Server::new_async().await;
    let _gone = server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;
    let _current = mock_fixture(&mut server, "/this-year.pdf").await;

    let (recorder, _guard) = EventRecorder::install();
    let index = build_index(
        &[
            plan(format!("{}/last-year.pdf", server.url()), FIXTURE_PAGES),
            plan(format!("{}/this-year.pdf", server.url()), FIXTURE_PAGES),
        ],
        &PdfCache::disabled(),
    )
    .await
    .expect("a retired plan must not keep the service from starting");

    assert!(index.lookup("Kolbermoor").is_some());

    // Silent to a client, but it has to be visible to whoever maintains
    // plans.yaml — it will stay broken until they act.
    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings.iter().any(|w| w.contains("gone upstream")),
        "the skipped plan was not logged, got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// ...but when the retired plan was the only one, nothing was read at all.
//
// Serving an empty index would answer "District not found" for every name in
// the county — an assertion about data nobody looked at, with nothing in the
// log above DEBUG and nothing in the 5xx rate to show for it. A fully stale
// plans.yaml has to be loud, and refusing to start is as loud as it gets.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_only_plan_retired_refuses_to_start() {
    let mut server = mockito::Server::new_async().await;
    let _gone = server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;

    let result = build_index(
        &[plan(format!("{}/last-year.pdf", server.url()), "1")],
        &PdfCache::disabled(),
    )
    .await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("none of the 1")),
        "a retired-only config must not start, got: {result:?}"
    );
}

#[tokio::test]
async fn test_no_plans_refuses_to_start() {
    let result = build_index(&[], &PdfCache::disabled()).await;
    assert!(
        matches!(result, Err(PlanError::Failed(ref d)) if d.contains("none of the 0")),
        "an empty plans.yaml must not start, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// A district that only a later plan covers still ends up in the index — the
// build merges every plan rather than stopping at the first one that parses.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_district_only_in_a_later_plan_is_indexed() {
    // Premise: "Vogtareuth" is not on page 1 of the fixture, so a plan limited
    // to that page cannot contribute it.
    let page_one = index_districts(&fixture_pdf_bytes(), "1").expect("fixture must parse");
    assert!(!page_one.contains_key("Vogtareuth"));

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/schedule.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(fixture_pdf_bytes())
        .expect(2)
        .create_async()
        .await;

    let url = format!("{}/schedule.pdf", server.url());
    let index = build_index(
        &[plan(url.clone(), "1"), plan(url, FIXTURE_PAGES)],
        &PdfCache::disabled(),
    )
    .await
    .expect("both plans must index");

    assert!(index.lookup("Vogtareuth").is_some_and(|d| !d.is_empty()));
}

// ---------------------------------------------------------------------------
// A plan URL may carry a query string. The `.pdf` check looks at the path, so
// a cache-busting `?v=…` on an otherwise valid link is not a config error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_plan_url_with_query_string_is_fetched() {
    let mut server = mockito::Server::new_async().await;
    let _mock = mock_fixture(&mut server, "/schedule.pdf?v=2").await;

    let state = AppState::build(
        &[plan(
            format!("{}/schedule.pdf?v=2", server.url()),
            FIXTURE_PAGES,
        )],
        &PdfCache::disabled(),
    )
    .await
    .expect("a query string must not disqualify a plan URL");

    let response = get(state, "/lk_rosenheim?district=Kolbermoor").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!body_to_json(response).await.as_array().unwrap().is_empty());
}
