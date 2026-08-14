# Test Coverage

`cargo llvm-cov` line coverage is ~83 % — ≈96 % excluding the `main.rs` server-bootstrap entrypoint, which is 90 uncovered lines. The IP-parsing logic was extracted into `config::parse_forwarded_allow_ips` so it can be unit-tested.

Deliberate gaps:

- **`download_pdf`'s timeout path.** The timeout is a fixed `DOWNLOAD_TIMEOUT` (30 s, in `index.rs`), so provoking it means a test that sleeps. It produces a `PlanError::Failed` from `transport_error` — the same arm every other transport fault takes, covered by `test_unreachable_host_refuses_to_start` via a `.invalid` host.
- **`cache.rs`'s `put` write/rename error arms.** Reaching them needs a directory that turns unwritable *between* `from_env` and the write, and they do the same thing (WARN, carry on) as the covered `from_env` path.
- **`errors.rs`'s `is_server_error()` branch** in `into_response`, which no current variant can reach. The root `CLAUDE.md` says why it stays.
- **`index_districts`'s `page_count()` error arm.** Reaching it needs bytes that `PdfDocument::from_bytes` accepts but whose page tree neither `/Count` nor `pdf_oxide`'s fallback scan can read. It produces the same `PlanError::Failed` as every other unreadable-PDF fault, covered by `test_invalid_bytes_rejected`.

## Which binary owns what

| Binary | Drives | Owns |
|---|---|---|
| `test_index.rs` | `build_index` / `AppState::build` with `PdfCache::disabled()` | Every download and parse fault: mockito, the size caps, retired plans |
| `test_cache.rs` | the same function with an enabled cache | The four `build_index` decisions tabulated in the root `CLAUDE.md` |
| `test_api.rs` | the router over a seeded or fixture-built index | What a client can observe: hit, miss, bad parameter |

Helpers more than one binary needs — the fixture bytes, `mock_fixture`, `temp_dir`, `state_from_fixture`, `body_to_json`, `get`, `EventRecorder` — live in `tests/common/mod.rs`, which carries a blanket `#![allow(dead_code)]` because each binary uses a different subset.

`test_cache.rs` gives each test its own `temp_dir(…)` (pid + nanos, like `write_temp` in `test_config.rs` — there is no `tempfile` dependency) and cleans it up at the end rather than in a `Drop` guard, so a failing assertion leaves the directory behind.

## Conventions

- Integration tests use `tower::ServiceExt::oneshot`, not `axum-test`, to avoid version conflicts.
- Network tests use `mockito`. `test_the_source_is_read_once_and_never_again` uses `.expect(1)` + `assert_async()`: five requests, four misses, one fetch.
- District names with special characters are URL-encoded with `urlencoding::encode`.
- The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.
- `test_missing_district_parameter_returns_400` checks for `StatusCode::BAD_REQUEST`: axum 0.8 changed missing-query-param responses from 422 to 400.

## The thread-local subscriber rule

Tests that assert on log output (`EventRecorder` in `tests/common`, `TraceRecorder` in `test_middleware.rs`) install the subscriber with `tracing::subscriber::set_default`, which is **thread-local**. Two things this depends on:

- `#[tokio::test]`'s current-thread runtime keeps the work on the calling thread. A `multi_thread` flavour — or an assertion on something logged inside the `spawn_blocking` parse — records nothing.
- A permissive **global** subscriber must be installed first (`init_global_tracing` / `init_tracing`). `tracing` caches each callsite's `Interest` globally; without a global subscriber it is computed against `NoSubscriber` and cached as "never", and the thread-local recorder is then skipped before the dispatcher is consulted. Since tests run in parallel, omitting this makes the assertions pass or fail depending on which test reached the callsite first.
