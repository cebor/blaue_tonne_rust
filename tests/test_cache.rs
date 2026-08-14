//! The on-disk plan cache: that a second start makes no request, and the four
//! `build_index` decisions around expired, corrupt and retired entries.

use std::time::Duration;

use blaue_tonne_rust::cache::PdfCache;
use blaue_tonne_rust::errors::PlanError;
use blaue_tonne_rust::index::build_index;

mod common;
use common::{EventRecorder, mock_fixture, temp_dir};

/// A TTL long enough that nothing in a test run can outlive it.
const FRESH: Duration = Duration::from_secs(3600);

/// Removes the directory a test created. Called explicitly rather than from a
/// `Drop` guard, so a failing assertion leaves the directory behind to look at.
fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("cache dir must exist")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- The reason the cache exists ---

#[tokio::test]
async fn test_a_second_start_reads_the_plan_from_disk_and_makes_no_request() {
    let dir = temp_dir("cache_hit");
    let mut server = mockito::Server::new_async().await;
    // One request across two full startups; `.expect(1)` is what pins it.
    let mock = mock_fixture(&mut server, "/schedule.pdf").await.expect(1);

    let plans = [format!("{}/schedule.pdf", server.url())];
    let cache = PdfCache::new(dir.clone(), FRESH);

    let (recorder, _guard) = EventRecorder::install();
    let first = build_index(&plans, &cache)
        .await
        .expect("the first start downloads the plan");
    let second = build_index(&plans, &cache)
        .await
        .expect("the second start must not need the network");

    mock.assert_async().await;
    assert_eq!(first.len(), second.len());
    assert!(second.lookup("Kolbermoor").is_some());
    assert_eq!(files_in(&dir).len(), 1, "one plan, one cache file");

    // The `source` field is filtered on, so its values are part of the
    // interface rather than incidental log text.
    let indexed: Vec<String> = recorder
        .at(tracing::Level::INFO)
        .into_iter()
        .filter(|line| line.contains("indexed plan"))
        .collect();
    assert_eq!(indexed.len(), 2, "one line per plan per start: {indexed:?}");
    assert!(indexed[0].contains("\"url\""), "{:?}", indexed[0]);
    assert!(indexed[1].contains("\"cache\""), "{:?}", indexed[1]);

    cleanup(&dir);
}

#[tokio::test]
async fn test_the_cache_file_is_named_after_the_plan_it_holds() {
    let dir = temp_dir("cache_name");
    let mut server = mockito::Server::new_async().await;
    let _mock = mock_fixture(&mut server, "/Abfuhrplan_2026.pdf").await;

    let cache = PdfCache::new(dir.clone(), FRESH);
    build_index(&[format!("{}/Abfuhrplan_2026.pdf", server.url())], &cache)
        .await
        .expect("must index");

    let files = files_in(&dir);
    assert!(
        files[0].ends_with("-Abfuhrplan_2026.pdf"),
        "the file name has to say which plan it holds: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.contains("tmp-")),
        "the temp file used for the atomic write must not survive: {files:?}"
    );

    cleanup(&dir);
}

// --- Expiry ---

#[tokio::test]
async fn test_an_expired_entry_is_downloaded_again() {
    let dir = temp_dir("cache_expired");
    let mut server = mockito::Server::new_async().await;
    // Two starts, two requests: an expired file is not used while the source
    // answers.
    let mock = mock_fixture(&mut server, "/schedule.pdf").await.expect(2);

    let plans = [format!("{}/schedule.pdf", server.url())];
    // A zero TTL means "already expired" without backdating an mtime: nothing
    // is ever fresh, but entries are still written.
    let cache = PdfCache::new(dir.clone(), Duration::ZERO);

    build_index(&plans, &cache).await.expect("first start");
    let second = build_index(&plans, &cache).await.expect("second start");

    mock.assert_async().await;
    assert!(second.lookup("Kolbermoor").is_some());

    cleanup(&dir);
}

// --- The stale fallback ---

#[tokio::test]
async fn test_an_unreachable_source_falls_back_to_the_expired_copy() {
    let dir = temp_dir("cache_stale");
    let mut server = mockito::Server::new_async().await;

    // First start: the source answers, the plan lands in the cache.
    let good = mock_fixture(&mut server, "/schedule.pdf").await;
    let plans = [format!("{}/schedule.pdf", server.url())];
    let cache = PdfCache::new(dir.clone(), Duration::ZERO);
    build_index(&plans, &cache).await.expect("first start");
    good.remove_async().await;

    // Second start: the source is broken and the copy has expired.
    let _broken = server
        .mock("GET", "/schedule.pdf")
        .with_status(503)
        .create_async()
        .await;

    let (recorder, _guard) = EventRecorder::install();
    let index = build_index(&plans, &cache)
        .await
        .expect("an outage with a cached copy must not keep the service down");

    assert!(index.lookup("Kolbermoor").is_some());
    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings.iter().any(|w| w.contains("stale cached copy")),
        "serving expired data must produce a WARN: {warnings:?}"
    );

    cleanup(&dir);
}

#[tokio::test]
async fn test_an_unreachable_source_without_a_cached_copy_is_still_fatal() {
    let dir = temp_dir("cache_stale_none");
    let mut server = mockito::Server::new_async().await;
    let _broken = server
        .mock("GET", "/schedule.pdf")
        .with_status(503)
        .create_async()
        .await;

    let cache = PdfCache::new(dir.clone(), FRESH);
    let result = build_index(&[format!("{}/schedule.pdf", server.url())], &cache).await;

    assert!(
        matches!(result, Err(PlanError::Failed(_))),
        "with nothing to fall back to this stays fatal: {result:?}"
    );

    cleanup(&dir);
}

// --- A retired plan (404) is gone, cache or no cache ---

#[tokio::test]
async fn test_a_retired_plan_is_skipped_even_though_it_is_cached() {
    let dir = temp_dir("cache_retired");
    let mut server = mockito::Server::new_async().await;

    // Cache both plans while the source still serves them.
    let last_year = mock_fixture(&mut server, "/last-year.pdf").await;
    let _this_year = mock_fixture(&mut server, "/this-year.pdf").await;
    let plans = [
        format!("{}/last-year.pdf", server.url()),
        format!("{}/this-year.pdf", server.url()),
    ];
    let cache = PdfCache::new(dir.clone(), Duration::ZERO);
    build_index(&plans, &cache).await.expect("first start");
    last_year.remove_async().await;

    // Last year's plan is withdrawn: the cached copy must not resurrect it.
    let _gone = server
        .mock("GET", "/last-year.pdf")
        .with_status(404)
        .create_async()
        .await;

    let (recorder, _guard) = EventRecorder::install();
    build_index(&plans, &cache)
        .await
        .expect("the remaining plan still indexes");

    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings.iter().any(|w| w.contains("gone upstream")),
        "expected the retired-plan warning, got: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.contains("stale cached copy")),
        "a 404 must not go through the stale fallback: {warnings:?}"
    );

    cleanup(&dir);
}

// --- A bad cache file must not be able to brick startup ---

#[tokio::test]
async fn test_a_corrupt_cache_file_is_replaced_rather_than_fatal() {
    let dir = temp_dir("cache_corrupt");
    let mut server = mockito::Server::new_async().await;
    let _mock = mock_fixture(&mut server, "/schedule.pdf").await;

    let plans = [format!("{}/schedule.pdf", server.url())];
    let cache = PdfCache::new(dir.clone(), FRESH);
    build_index(&plans, &cache).await.expect("first start");

    // Overwrite the entry with something that is not a PDF.
    let entry = dir.join(&files_in(&dir)[0]);
    std::fs::write(&entry, b"not a pdf at all").expect("overwrite cache entry");

    let (recorder, _guard) = EventRecorder::install();
    let index = build_index(&plans, &cache)
        .await
        .expect("a bad cache entry must be recoverable");

    assert!(index.lookup("Kolbermoor").is_some());
    let warnings = recorder.at(tracing::Level::WARN);
    assert!(
        warnings.iter().any(|w| w.contains("unusable")),
        "expected a refetch warning, got: {warnings:?}"
    );

    cleanup(&dir);
}

#[tokio::test]
async fn test_bytes_that_will_not_parse_are_never_cached() {
    let dir = temp_dir("cache_unparseable");
    let mut server = mockito::Server::new_async().await;
    // 200 and the right content-type, but bytes the parser cannot read: the
    // download succeeds, so `put` is reachable.
    let _mock = server
        .mock("GET", "/schedule.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body("not a pdf at all")
        .create_async()
        .await;

    let plans = [format!("{}/schedule.pdf", server.url())];
    let cache = PdfCache::new(dir.clone(), FRESH);

    let result = build_index(&plans, &cache).await;
    assert!(
        matches!(result, Err(PlanError::Failed(_))),
        "a plan that will not parse is still fatal: {result:?}"
    );
    // `put` runs after the parse, so nothing was written.
    assert_eq!(
        files_in(&dir),
        Vec::<String>::new(),
        "bytes that did not parse must not reach the cache"
    );

    cleanup(&dir);
}

// --- Switched off ---

#[tokio::test]
async fn test_a_disabled_cache_neither_reads_nor_writes() {
    let dir = temp_dir("cache_disabled");
    let mut server = mockito::Server::new_async().await;

    // Seeded through an enabled cache first, so "wrote nothing" and "read
    // nothing" can be told apart.
    let mock = mock_fixture(&mut server, "/schedule.pdf").await.expect(2);
    let plans = [format!("{}/schedule.pdf", server.url())];
    build_index(&plans, &PdfCache::new(dir.clone(), FRESH))
        .await
        .expect("seed the cache");
    let seeded = files_in(&dir);
    assert_eq!(seeded.len(), 1);

    build_index(&plans, &PdfCache::disabled())
        .await
        .expect("must index");

    mock.assert_async().await;
    assert_eq!(
        files_in(&dir),
        seeded,
        "a disabled cache must leave the directory exactly as it found it"
    );

    cleanup(&dir);
}

// --- PdfCache::from_env ---
//
// One `#[test]` for both cases: they mutate process-wide environment variables,
// which is not safe while other tests run in parallel. The pure resolution logic
// is tested without `set_var` in `test_config.rs`.

#[test]
fn test_from_env_covers_the_off_switch_and_an_unusable_directory() {
    let previous = std::env::var("PDF_CACHE_DIR").ok();

    // Set but empty: the cache is off. Unset would mean the default location.
    unsafe { std::env::set_var("PDF_CACHE_DIR", "") };
    assert!(
        !PdfCache::from_env().is_enabled(),
        "an empty PDF_CACHE_DIR is the documented off switch"
    );

    // A path that cannot become a directory, because it is a file.
    let dir = temp_dir("cache_env");
    let blocker = dir.join("not-a-directory");
    std::fs::write(&blocker, b"").expect("create blocking file");
    unsafe { std::env::set_var("PDF_CACHE_DIR", &blocker) };
    assert!(
        !PdfCache::from_env().is_enabled(),
        "an uncreatable directory degrades to no cache"
    );

    unsafe {
        match previous {
            Some(v) => std::env::set_var("PDF_CACHE_DIR", v),
            None => std::env::remove_var("PDF_CACHE_DIR"),
        }
    }
    cleanup(&dir);
}
