# Agent Instructions — blaue_tonne_rust

Axum service that extracts Blaue Tonne (blue bin / Altpapier) collection dates from PDFs published by Chiemgau Recycling for Landkreis Rosenheim.

See [README.md](README.md) for full project overview.

## Build & Test

Tests require the fixture PDF at `tests/fixtures/lk_rosenheim_2026.pdf` (already committed).

## Middleware & Request Pipeline

`build_router` (`src/router.rs`) builds a **traced** sub-router and merges it into an untraced outer router. `Router::layer` only affects routes registered before it, so `/health` — registered on the outer router — carries neither middleware. Container health checks run every few seconds; keeping them off the layers is what stops them flooding the logs. The trade-off: `/health` is not traced **at all**, at any `RUST_LOG` level. `tests/test_middleware.rs` pins this with a span-counting subscriber.

Within the traced sub-router, `ip_middleware` is added last (`.layer()`) so it is outermost — it runs **before** `TraceLayer`, ensuring the span already has `client_ip` populated. The middleware logic lives in `src/middleware.rs`.

1. **`ip_middleware`** — `middleware::resolve_client_ip`, wired up via `axum::middleware::from_fn_with_state` with the `FORWARDED_ALLOW_IPS` allowlist as state. If the connecting peer is in the allowlist, the leftmost `X-Forwarded-For` entry is used; otherwise the socket IP is used. Falls back to `127.0.0.1` in unit tests (no `ConnectInfo`). Inserts `ResolvedClientIp` extension.
2. **`TraceLayer`** — uses `middleware::make_request_span` to create an `info_span!` per request (method, URI, client_ip) and `middleware::log_response` to log status + latency_ms at INFO.

`log_response` is deliberately **not** `DefaultOnResponse`: tower-http emits under the `tower_http::trace` target, which the default `RUST_LOG` fallback (`blaue_tonne_rust=info`) filters out — request logging would silently disappear in production. `test_response_is_logged_under_this_crates_target` pins this.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PLANS_PATH` | `plans.yaml` | Path to plans YAML config |
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address to listen on |
| `FORWARDED_ALLOW_IPS` | *(empty)* | Comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; use `*` to trust all |
| `RUST_LOG` | `blaue_tonne_rust=info` | Standard `tracing-subscriber` filter. When unset, falls back to `blaue_tonne_rust=info`; when set it takes full control. `/health` is never logged regardless — it is outside the traced router. |

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Row reconstruction in `src/pdf_parser.rs` sorts `pdf_oxide` spans by Y descending (PDF Y increases upward), then X ascending, grouping them into a row while the Y delta stays within `Y_TOLERANCE`. No per-character X-gap splitting is needed — `pdf_oxide` already returns coherent spans.

50 districts are supported (see `DISTRICTS` constant in `tests/test_pdf_parser.rs`).

## Per-plan failures in the handler

In `lk_rosenheim_handler`, **every** per-plan failure is skipped — download faults, a panicking parse task, and `PdfError` from `get_dates` alike. A plan that is momentarily unreachable, 5xx-ing, timing out, or serving truncated bytes must not take down requests that a surviving plan can answer. It is usually the *old* plan that breaks (retired at the turn of the year) while the current one is fine, so hard-failing on the first fault would break exactly the requests that should still work. Per-plan `DistrictNotFound` is likewise a skip — the district may live in a later plan's PDF.

The skipped fault is remembered in `unread_plan` via `remember_fault`, which keeps the fault that **says the most**, not the first one: a `PdfError` outranks any source-side fault. Remembering only the first would let a flaky upstream on an earlier plan downgrade our own parse bug from 500 to 503 — turning "we are broken" into "come back later". A single unparseable plan therefore still answers 500 regardless of plan order; it just no longer drags down a request another plan could serve.

If no dates are found *and* a plan was skipped, that fault is returned instead of `DistrictNotFound`: absence is only established if the plans were actually read, and claiming "District not found" without having looked asserts something never checked.

**Whether a 404 is even available is decided by `plans_read`, not by `unread_plan`.** The counter is raised only by a plan that produced a definitive answer — `Ok(dates)` or `Err(DistrictNotFound)` from `get_dates`. When it is still zero at the end, nothing was searched at all, and the handler returns a fault instead of a 404 (`unread_plan`, or a fresh `Upstream` when the plans were merely retired). Two configurations hit this: a `plans.yaml` whose plans have all gone 404 upstream, and one with no plans at all.

This is the property that makes a stale `plans.yaml` visible. `PdfNotFound` is skipped *without* being remembered — an upstream 404 means the plan is gone, expected at the turn of the year and permanent until someone prunes the config, and counting it as "we did not look" would make `DistrictNotFound` unreachable for the weeks in between (every typo answering 503, telling the caller to retry something that will never start working). But with a single plan configured, that same rule used to make total failure *silent*: every district answered "District not found" while `download.rs` logged the 404 at DEBUG only — no 5xx, no WARN, nothing in the log at the default filter. `plans_read` separates the two cases without giving either up:

- old plan retired **+** current plan fine → `plans_read == 1` → a typo is still a 404 (`test_retired_plan_404_does_not_mask_a_genuine_404`)
- every plan retired → `plans_read == 0` → 503 plus the ERROR from `into_response`, visible in the status code and the 5xx rate (`test_only_plan_retired_returns_503`, `test_no_plans_returns_503`)

This is also why there is no separate "all plans are gone" warning — the status code already carries it.

**An empty or whitespace-only `district` is rejected up front** (400, before the cache and before any network access). `normalize_district` strips whitespace, so both normalize to `""`, which is not a name: it would match any row whose cells are whitespace only, and establishing that would cost a download and a parse of every plan.

**Only a complete answer is cached.** `dates_cache` has no expiry, so writing a result that was assembled while a plan was skipped would freeze that plan's missing dates in for the lifetime of the process — one momentary upstream blip, and the district keeps answering with half its dates forever. The handler therefore reads `unread_plan.is_none()` into `complete` (before the empty-check consumes the Option) and only inserts when it holds. `test_partial_result_from_a_skipped_plan_is_not_cached` pins it.

## Error Responses

`AppError`'s `Display` text is the **internal** detail (upstream URLs, library error strings): it is logged, never serialized. Clients get the fixed `client_message()` for the variant. `into_response` logs at ERROR for any 5xx and at DEBUG for any 4xx — the 4xx detail (e.g. axum's query-rejection text) would otherwise be collected and dropped unseen, and DEBUG keeps caller noise off the default filter.

**Nothing a client can observe may reveal that this service fetches and parses PDFs from a third party** — not the message, not the status code, not the `/docs` response descriptions. Every source-side fault therefore collapses into 503 rather than 502/504: a gateway status is itself a statement about the architecture. `test_no_variant_discloses_the_data_source` asserts this over all variants at once against a list of giveaway substrings, so a message added later is covered without anyone remembering to extend the file. A new *variant* is caught by `assert_every_variant_is_covered` next to it — an exhaustive `match` that exists only to stop compiling when `AppError` grows.

`lk_rosenheim_handler` takes `Result<Query<DistrictQuery>, QueryRejection>` rather than a bare `Query` so the 400 also becomes an `AppError` — axum's own rejection is a plain-text body, which would be the one response not matching the documented `ErrorDetail` schema.

| Variant | Status | Client sees | Meaning |
|---------|--------|-------------|---------|
| `BadRequest` | 400 | Invalid or missing query parameter | The only caller-caused variant: missing/undeserializable `district`, or one that is empty after normalization |
| `DistrictNotFound` | 404 | District not found | At least one plan was read, none contained the district |
| `PdfError` | 500 | Internal server error | Bytes downloaded but unparseable — our own fault |
| `Upstream`, `ServiceUnavailable` | 503 | Service temporarily unavailable… | Source unreachable/non-2xx/not-a-PDF/timed out, or no plan could be read at all |
| `PdfNotFound` | 503 | Service temporarily unavailable… | Plan retired upstream. Skipped per plan and never remembered, so this reaches a client only if it is constructed outside the handler |

## Download Size Cap

`download.rs` caps plan PDFs at `MAX_PDF_BYTES` (16 MiB) with **two** guards: a `Content-Length` pre-check and an accumulating check inside the `chunk()` read loop. The second is not redundant — `Content-Length` can be absent (chunked transfer) or wrong. Both produce the same 503 and the same client message, so their tests assert on the logged internal detail (`"advertises"` vs `"exceeds the"`); status alone would let either guard be deleted silently.

## Test Coverage

`cargo llvm-cov` line coverage is ~85 % (≈96 % excluding the `main.rs` server-bootstrap entrypoint). The IP-parsing logic was extracted from `main` into `config::parse_forwarded_allow_ips` so it can be unit-tested. The `download_pdf` timeout path is intentionally untested (fixed 30 s client timeout); `test_errors.rs` covers the variant's mapping instead.

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`. District names with special chars are URL-encoded with `urlencoding::encode`. The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.

Note: `test_missing_district_parameter_returns_400` checks for `StatusCode::BAD_REQUEST` — axum 0.8 changed missing-query-param responses from 422 to 400.

Tests that assert on log output (`EventRecorder` in `test_api.rs`, `TraceRecorder` in `test_middleware.rs`) install the subscriber with `tracing::subscriber::set_default`, which is **thread-local**. Two things this depends on:

- `#[tokio::test]`'s current-thread runtime keeps the request on the calling thread. A `multi_thread` flavour — or a path that reaches the `spawn_blocking` parse — would silently record nothing.
- A permissive **global** subscriber must be installed first (`init_global_tracing` / `init_tracing`). `tracing` caches each callsite's `Interest` globally; without a global subscriber it is computed against `NoSubscriber` and cached as "never", and the thread-local recorder is then skipped before the dispatcher is consulted. Since tests run in parallel, omitting this makes the assertions pass or fail depending on which test reached the callsite first — it produced a genuine flake, not a theoretical one.

## `plans.yaml`

`pages` is passed directly to `get_dates`, which parses the comma-separated 1-based page numbers and uses them as 0-based indices for `pdf_oxide`.

`url` is validated in `config::validate_plan_url` at load time — scheme must be `http`/`https`, and the URL **path** must end in `.pdf`. Whether a URL is fetchable at all is a property of the config, not of a request: checked here, a typo fails once at startup with a clear message; left to `download_pdf` it becomes a 503 ("try again later" — it never will) plus a WARN on every request for the lifetime of the process. Matching on the path rather than the whole string is what lets a link carry a query string or fragment (`…/Abfuhrplan_2027.pdf?v=2`). `download.rs` keeps an equivalent path-based check as a guard for callers that build a URL some other way.

## Known costs, deliberately not fixed

- **Unknown districts are not negatively cached.** `dates_cache` only ever holds hits, so every request for a name that is in no plan costs, per plan, a `PdfDocument::from_bytes(pdf_bytes.to_vec())` — a full copy of the PDF — plus a blocking-pool thread, and `spawn_blocking` tasks are not cancelled when the client disconnects. A loop of random names is therefore arbitrarily expensive. Caching misses by district name would be *worse*: the key space is caller-controlled and would grow without bound. The real fix is an index cache per `(url, pages)` — parse each PDF once into a `district → dates` map — which makes misses O(1) as a side effect. Left for a later change.
- **Neither cache expires.** If the source corrects a PDF under the same URL, the process never sees it; at the turn of the year a restart is required.
- **No single-flight on downloads.** N concurrent cold requests fetch the same PDF N times, bounded only by `MAX_PDF_BYTES` per request.

## Docker

See the `docker-build` skill (`.claude/skills/docker-build/SKILL.md`) for the image build and runtime details.

## Key Conventions

- **All code comments must be in English** — never write German comments, even when the conversation is in German.
- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `dates_cache` is keyed by the **normalized** district name (`pdf_parser::normalize_district`); `pdf_cache` is keyed by PDF URL (`String`).
