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
| `PLANS_PATH` | `plans.yaml` | Path to plans YAML config. Read once, at startup — a change needs a restart |
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address to listen on |
| `FORWARDED_ALLOW_IPS` | *(empty)* | Comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; use `*` to trust all |
| `RUST_LOG` | `blaue_tonne_rust=info` | Standard `tracing-subscriber` filter. When unset, falls back to `blaue_tonne_rust=info`; when set it takes full control. `/health` is never logged regardless — it is outside the traced router. |

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Row reconstruction in `src/pdf_parser.rs` sorts `pdf_oxide` spans by Y descending (PDF Y increases upward), then X ascending, grouping them into a row while the Y delta stays within `Y_TOLERANCE`. No per-character X-gap splitting is needed — `pdf_oxide` already returns coherent spans.

50 districts are supported (see `DISTRICTS` constant in `tests/test_pdf_parser.rs`).

`index_districts` reads a whole plan in one pass and returns `district → dates`. It is the only entry point: there is deliberately no per-district search function, because two ways into the same row-matching rules is exactly the duplication this design removed. A row that carries dates itself is skipped as a key (it is a date row, not a name row), and a name row without dates around it is not an entry — the same rule a linear search would follow when it keeps scanning past a bare match. First occurrence wins, so pages are read in order.

## The index is built at startup

`build_index` (`src/index.rs`) downloads and parses every plan once, before `main` binds the listener. `AppState` holds nothing but the resulting `Arc<DistrictIndex>`; the `reqwest::Client` is local to the build, because after it returns the service does no network I/O at all. A request is `index.lookup(&normalize_district(name))` and nothing else.

**A plan that cannot be read is fatal.** There is no second attempt at request time any more, so starting anyway would mean serving a district short of its dates for the lifetime of the process, silently — the failure would live in the data instead of in the status. `main` logs the fault at ERROR and exits 1, which a container restart makes visible. The trade-off is accepted deliberately: an upstream outage during boot is a restart loop, not a degraded service.

**The one exception is an upstream 404**, which means the plan is *gone* — expected at the turn of the year, when last year's PDF goes offline while it is still listed in `plans.yaml`, and permanent until someone prunes the config. Treating that as fatal would keep the service down for weeks over a plan nobody needs. It is skipped with a WARN naming the URL — once, at startup, so there is no per-request log flooding to weigh against saying it loudly.

**`plans_indexed == 0` is fatal too.** Every plan retired, or none configured, means nothing was read: an empty index would answer "District not found" for every name in the county, an assertion about data nobody looked at. Refusing to start is what makes a fully stale `plans.yaml` impossible to miss (`test_only_plan_retired_refuses_to_start`, `test_no_plans_refuses_to_start`).

Dates for a district that several plans carry are concatenated in plan order — not deduplicated, not sorted. That is what lets a district keep both the old and the new plan's dates while both are configured.

**An empty or whitespace-only `district` is still rejected up front** (400). `normalize_district` strips whitespace, so both normalize to `""`, which is not a name; without the guard they would fall through to a plain index miss and answer 404, reporting a district the caller never named as missing.

**Consequence: `plans.yaml` is read exactly once.** A new plan, or a corrected PDF under an unchanged URL, needs a restart. This is the accepted cost of the design, not an oversight.

## Error Responses

`AppError`'s `Display` text is the **internal** detail (upstream URLs, library error strings): it is logged, never serialized. Clients get the fixed `client_message()` for the variant. `into_response` logs at ERROR for any 5xx and at DEBUG for any 4xx — the 4xx detail (e.g. axum's query-rejection text) would otherwise be collected and dropped unseen, and DEBUG keeps caller noise off the default filter.

**Nothing a client can observe may reveal that this service fetches and parses PDFs from a third party** — not the message, not the status code, not the `/docs` response descriptions. `test_no_variant_discloses_the_data_source` asserts this over all variants at once against a list of giveaway substrings, so a message added later is covered without anyone remembering to extend the file. A new *variant* is caught by `assert_every_variant_is_covered` next to it — an exhaustive `match` that exists only to stop compiling when `AppError` grows. The source-side variants keep their 503 mapping (rather than 502/504, which would itself be a statement about the architecture) even though nothing on the route produces them any more: they exist as a type, and `IntoResponse` has to stay correct for one.

`lk_rosenheim_handler` takes `Result<Query<DistrictQuery>, QueryRejection>` rather than a bare `Query` so the 400 also becomes an `AppError` — axum's own rejection is a plain-text body, which would be the one response not matching the documented `ErrorDetail` schema.

Only the first two rows below are reachable over HTTP; the rest are startup faults, logged by `main` and never serialized. The `#[utoipa::path]` annotation lists 200/400/404 accordingly.

| Variant | Status | Client sees | Meaning |
|---------|--------|-------------|---------|
| `BadRequest` | 400 | Invalid or missing query parameter | The only caller-caused variant: missing/undeserializable `district`, or one that is empty after normalization |
| `DistrictNotFound` | 404 | District not found | The district is in no plan — an observation, since every plan was read at startup |
| `PdfError` | 500 | Internal server error | Startup: bytes downloaded but unparseable — our own fault |
| `Upstream`, `ServiceUnavailable` | 503 | Service temporarily unavailable… | Startup: source unreachable/non-2xx/not-a-PDF/timed out, or no plan could be read at all |
| `PdfNotFound` | 503 | Service temporarily unavailable… | Startup: plan retired upstream. Skipped by `build_index`, so it never propagates |

## Download Size Cap

`download.rs` caps plan PDFs at `MAX_PDF_BYTES` (16 MiB) with **two** guards: a `Content-Length` pre-check and an accumulating check inside the `chunk()` read loop. The second is not redundant — `Content-Length` can be absent (chunked transfer) or wrong. Both produce the same `Upstream` variant, so their tests assert on the internal detail (`"advertises"` vs `"exceeds the"`); the variant alone would let either guard be deleted silently.

## Test Coverage

`cargo llvm-cov` line coverage is ~80 % — ≈98 % excluding the `main.rs` server-bootstrap entrypoint, which is 89 uncovered lines and now a much larger share of a smaller crate than it used to be; the headline number fell while the tested code got *better* covered. The IP-parsing logic was extracted from `main` into `config::parse_forwarded_allow_ips` so it can be unit-tested. The `download_pdf` timeout path is intentionally untested (fixed `DOWNLOAD_TIMEOUT`, 30 s, in `index.rs`); `test_errors.rs` covers the variant's mapping instead.

The split follows where things can fail: `tests/test_index.rs` drives `build_index`/`AppState::build` and owns every download and parse fault (mockito, the size caps, retired plans); `tests/test_api.rs` drives the router over a seeded or fixture-built index and owns what a client can still observe (hit, miss, bad parameter). Helpers both need — the fixture bytes, `state_from_fixture`, `body_to_json`, `get`, `EventRecorder` — live in `tests/common/mod.rs`, which carries a blanket `#![allow(dead_code)]` because each binary uses a different subset.

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`; `test_the_source_is_read_once_and_never_again` uses `.expect(1)` + `assert_async()` to pin the property this design exists for — five requests, four of them misses, one fetch. District names with special chars are URL-encoded with `urlencoding::encode`. The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.

Note: `test_missing_district_parameter_returns_400` checks for `StatusCode::BAD_REQUEST` — axum 0.8 changed missing-query-param responses from 422 to 400.

Tests that assert on log output (`EventRecorder` in `tests/common`, `TraceRecorder` in `test_middleware.rs`) install the subscriber with `tracing::subscriber::set_default`, which is **thread-local**. Two things this depends on:

- `#[tokio::test]`'s current-thread runtime keeps the work on the calling thread. A `multi_thread` flavour — or an assertion on something logged inside the `spawn_blocking` parse — would silently record nothing.
- A permissive **global** subscriber must be installed first (`init_global_tracing` / `init_tracing`). `tracing` caches each callsite's `Interest` globally; without a global subscriber it is computed against `NoSubscriber` and cached as "never", and the thread-local recorder is then skipped before the dispatcher is consulted. Since tests run in parallel, omitting this makes the assertions pass or fail depending on which test reached the callsite first — it produced a genuine flake, not a theoretical one.

## `plans.yaml`

`pages` is passed directly to `index_districts`, which parses the comma-separated 1-based page numbers and uses them as 0-based indices for `pdf_oxide`. A page number past the end of the document is a `PdfError` — and now a refused startup rather than a per-request 500.

`url` is validated in `config::validate_plan_url` at load time — scheme must be `http`/`https`, and the URL **path** must end in `.pdf`. Matching on the path rather than the whole string is what lets a link carry a query string or fragment (`…/Abfuhrplan_2027.pdf?v=2`). `download.rs` keeps an equivalent path-based check as a guard for callers that build a URL some other way.

## Known costs, deliberately not fixed

- **The index is only as fresh as the process.** A changed `plans.yaml`, or a corrected PDF under an unchanged URL, is picked up on restart and not before. This is the trade that removed the per-request download, the two caches, and every request-time failure mode with them.
- **An upstream outage during boot is a restart loop.** The process refuses to start rather than serve an incomplete index, so a container will keep restarting until the source is reachable. Visible in the restart count and the ERROR line; preferred over a silently half-populated index, which nothing would surface at all.
- **The whole index lives in memory, unbounded by anything but `MAX_PDF_BYTES` per plan at build time.** Fine for ~50 districts across one or two plans; it is not a design that scales to hundreds of plans.

## Docker

See the `docker-build` skill (`.claude/skills/docker-build/SKILL.md`) for the image build and runtime details.

## Key Conventions

- **All code comments must be in English** — never write German comments, even when the conversation is in German.
- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `DistrictIndex` is keyed by the **normalized** district name (`pdf_parser::normalize_district`), and every lookup has to normalize first. `DistrictIndex::from_pairs` normalizes what it is given, so a test cannot seed a key `lookup` could never reach.
