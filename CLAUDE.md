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

**The full table lives in [README.md](README.md#environment-variables)** and is the one to edit when a variable changes. Kept here are only the three whose *behaviour* the rest of this file argues about:

- **`PLANS_PATH`** is read exactly once, at startup. A new plan needs a restart — see below for why that is the accepted cost and not an oversight.
- **`RUST_LOG`** takes full control when set; only when it is absent does the `blaue_tonne_rust=info` fallback apply. `/health` is never logged at any level, because it is registered outside the traced router rather than filtered out.
- **`PDF_CACHE_DIR`** unset means the default path, set-but-empty means the cache is off. One variable for both, deliberately — see the cache section.

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Row reconstruction in `src/pdf_parser.rs` sorts `pdf_oxide` spans by Y descending (PDF Y increases upward), then X ascending, grouping them into a row while the Y delta stays within `Y_TOLERANCE`. No per-character X-gap splitting is needed — `pdf_oxide` already returns coherent spans.

50 districts are supported (see `DISTRICTS` constant in `tests/test_pdf_parser.rs`).

`index_districts` reads a whole plan in one pass and returns `district → dates`. It is the only entry point: there is deliberately no per-district search function, because two ways into the same row-matching rules is exactly the duplication this design removed. A row that carries dates itself is skipped as a key (it is a date row, not a name row), and a name row without dates around it is not an entry — the same rule a linear search would follow when it keeps scanning past a bare match. First occurrence wins, so pages are read in order.

## The index is built at startup

`build_index` (`src/index.rs`) downloads and parses every plan once, before `main` binds the listener. `AppState` holds nothing but the resulting `Arc<DistrictIndex>`; the `reqwest::Client` is local to the build, because after it returns the service does no network I/O at all. A request is `index.lookup(&normalize_district(name))` and nothing else.

**A plan that can be read from neither the source nor the cache is fatal.** There is no second attempt at request time any more, so starting anyway would mean serving a district short of its dates for the lifetime of the process, silently — the failure would live in the data instead of in the status. `main` logs the fault at ERROR and exits 1, which a container restart makes visible.

Loading `plans.yaml` fails the same way, through the same `match`-log-exit shape rather than an `expect`. Both are startup faults whose detail (a path, a URL, a serde message) belongs in tracing next to every other one — a panic would put it on stderr in a different format and add a backtrace nobody needs.

**The one exception is an upstream 404**, which means the plan is *gone* — expected at the turn of the year, when last year's PDF goes offline while it is still listed in `plans.yaml`, and permanent until someone prunes the config. Treating that as fatal would keep the service down for weeks over a plan nobody needs. It is skipped with a WARN naming the URL — once, at startup, so there is no per-request log flooding to weigh against saying it loudly.

**`plans_indexed == 0` is fatal too.** Every plan retired, or none configured, means nothing was read: an empty index would answer "District not found" for every name in the county, an assertion about data nobody looked at. Refusing to start is what makes a fully stale `plans.yaml` impossible to miss (`test_only_plan_retired_refuses_to_start`, `test_no_plans_refuses_to_start`). A plan served from the cache counts as indexed.

## The plan PDFs are cached on disk

`src/cache.rs`. `build_index` takes a `&PdfCache` and consults it before every download, so a normal start does no network I/O at all — plans change once a year, and re-downloading identical bytes on every restart bought nothing.

`PdfCache { dir: Option<PathBuf>, ttl: Duration }`. **`dir: None` is a disabled cache and is not a special case anywhere** — `get`/`put` are no-ops, and the three ways to get there (empty `PDF_CACHE_DIR`, a directory that could not be created, `PdfCache::disabled()` in tests) all converge on the same code. That is what keeps `build_index` free of "if caching is on" branches.

**Unset `PDF_CACHE_DIR` ≠ empty `PDF_CACHE_DIR`.** Unset picks the default location, empty switches the cache off. One variable therefore covers both the path and the off switch; a separate `PDF_CACHE_ENABLED` would allow "enabled, no path", a contradiction someone would have to define behaviour for. `config::cache_dir_from` holds the resolution logic as a pure function of the three env values so it is testable without mutating process-wide environment — only `PdfCache::from_env`'s two edge cases need `set_var`, and they share one serial `#[test]`.

**Key:** `{sha256(url)[..16 hex]}-{URL's own file name}`. The hash makes it unique and filesystem-safe for any URL (including one with `/` or `..` in it); the readable tail is what lets `ls` on the cache directory say which plan is which. Deliberately not `DefaultHasher`, whose output is explicitly unstable across Rust releases — the cache would silently empty itself on every toolchain upgrade. `put` writes to a sibling temp file (the key with its extension replaced by `tmp-{pid}`) and renames it into place, so a crash cannot leave a half-written PDF that a later start reads back as corrupt. It is also called **after** the parse succeeds, not before: bytes that will not parse are never written, so the next start does not find a fresh entry it has to throw away, and a later outage does not fall back to a copy that cannot be read either.

`put` returns `()`, not `Result`: no caller could act on a failure differently than by carrying on with the bytes it already holds. Every fault in the module — unwritable directory, unreadable file, failed rename — degrades to "no cache" plus a log line. The cache is an optimization, never a data path, and nothing in it can make the service fail.

The I/O is blocking `std::fs` on purpose. It happens only inside `build_index`, before the listener binds, when the runtime has nothing else to do — so there is no executor to starve and no need for tokio's `fs` feature.

Four decisions in `build_index` that the code alone would not explain, each pinned by a test in `tests/test_cache.rs`:

| Situation | Behaviour | Why |
|---|---|---|
| Fresh cache entry | Used, no request | The point of the feature |
| Fresh entry that will not parse | WARN, refetch | Otherwise a corrupt file is a startup error **no restart can clear** |
| Download fails, expired entry exists | WARN "serving a stale cached copy", start anyway | The dates were right when fetched and a plan changes once a year. This is what turns a boot-time outage from a restart loop into a degraded start |
| Download 404s, entry exists | Entry ignored, plan skipped | 404 means *retired*. Serving the copy would keep a withdrawn plan alive for as long as the file survives |

If the stale copy does not parse either, the **download** error is returned, not the parse error — the failed fetch is the cause, the unusable fallback only a consequence.

**One INFO line per indexed plan**, from a single callsite at the end of the loop body:

```
indexed plan url=… source="cache" age_secs=9 districts=52
```

`source` is `url`, `cache`, or `stale-cache`; `age_secs` is 0 for `url` and the file's age otherwise. Deliberately a *field* rather than three different messages — the question "was this downloaded or read off disk?" should be answerable by filtering, not by knowing which wording each branch chose. `test_a_second_start_reads_the_plan_from_disk_and_makes_no_request` asserts on the two values, so the field is part of the interface rather than incidental log text. `stale-cache` comes *in addition to* the WARN, which stays: an operator scanning WARN must not have to reconstruct it from an INFO field.

Dates for a district that several plans carry are concatenated in plan order — not deduplicated, not sorted. That is what lets a district keep both the old and the new plan's dates while both are configured.

**An empty or whitespace-only `district` is still rejected up front** (400). `normalize_district` strips whitespace, so both normalize to `""`, which is not a name; without the guard they would fall through to a plain index miss and answer 404, reporting a district the caller never named as missing.

**Consequence: `plans.yaml` is read exactly once.** A new plan, or a corrected PDF under an unchanged URL, needs a restart. This is the accepted cost of the design, not an oversight.

## Download Size Cap

`download.rs` caps plan PDFs at `MAX_PDF_BYTES` (16 MiB) with **two** guards: a `Content-Length` pre-check and an accumulating check inside the `chunk()` read loop. The second is not redundant — `Content-Length` can be absent (chunked transfer) or wrong. Both produce the same `PlanError::Failed` variant — as does every other startup fault — so their tests assert on the internal detail (`"advertises"` vs `"exceeds the"`); the variant alone would let either guard be deleted silently.

## Two error types, split by whether anyone can see them

Both live in `src/errors.rs`, deliberately in one file: the split between them *is* the design, and it stays honest more easily when both are in view — a new variant belongs in `AppError` only if a client can actually receive it.

**`AppError` is what a request can be answered with.** Two variants, both the caller's own doing:

| Variant | Status | Client sees | Meaning |
|---------|--------|-------------|---------|
| `BadRequest` | 400 | Invalid or missing query parameter | Missing/undeserializable `district`, or one that is empty after normalization |
| `DistrictNotFound` | 404 | District not found | The district is in no plan — an observation, since every plan was read at startup |

That is the complete list because the route is: normalize, reject `""`, look up. The `#[utoipa::path]` annotation lists 200/400/404 to match.

**`PlanError` is what reading a plan can fail with**, and it never becomes a response — `build_index` runs before the listener binds, and `main` logs the fault and exits 1. Two variants, because the startup path asks exactly two questions:

| Variant | Meaning |
|---------|---------|
| `Retired(url)` | Upstream 404: the plan is gone. The only variant that exists for **control flow** — `build_index` matches on it to skip the plan with a WARN |
| `Failed(detail)` | Everything else: unreachable, non-2xx, wrong content-type, timed out, over the size cap, unparseable bytes. All fatal, all handled identically |

`Failed` is deliberately one variant and not five. The old `AppError` drew finer lines (`Upstream`, `ServiceUnavailable`, `PdfError`), but nothing ever matched on them: they differed only in a status code and a client message that no client could reach. What actually tells those faults apart is the **message**, which is why the tests assert on substrings (`"advertises"`, `"exceeds the"`, `"text/html"`, `"cross-reference"`) rather than on the variant — a variant match would now assert nothing.

**Why the split at all:** with the startup faults in `AppError`, `IntoResponse` carried status codes and client messages for four cases that could not occur, and `test_errors.rs` maintained the no-disclosure invariant over messages nobody serves. Both are now scoped to what is actually served.

**The response body is the `ErrorDetail` struct**, which lives in `errors.rs` next to the only thing that produces one and is the same type `/docs` advertises. It used to be declared in `handlers.rs` for the OpenAPI annotation alone while `into_response` built the real body by hand as `json!({"detail": …})` — two shapes coupled by nothing but convention, either of which could be renamed without the other noticing. Serializing the struct is also what took `serde_json` out of `[dependencies]`; the tests still deserialize with it, so it is a dev-dependency now.

`AppError`'s `Display` is still the internal detail (axum's rejection text) and is logged, never serialized; `into_response` logs at DEBUG for 4xx so caller noise stays off the default filter, and keeps the ERROR branch for a future 5xx variant so one could not be added and then vanish under the default filter. `PlanError`'s `Display` may name plan URLs and library text freely — nothing serializes it.

**Nothing a client can observe may reveal that this service fetches and parses PDFs from a third party** — not the message, not the status code, not the `/docs` response descriptions. Since the split this is mostly true by construction: no `AppError` variant even has a plan URL to leak. `test_no_variant_discloses_the_data_source` stays anyway, because the invariant is about what may be *added* — a variant carrying upstream text is the first thing someone would write if request-time fetching ever came back. `assert_every_variant_is_covered` next to it is an exhaustive `match` that exists only to stop compiling when `AppError` grows.

`lk_rosenheim_handler` takes `Result<Query<DistrictQuery>, QueryRejection>` rather than a bare `Query` so the 400 also becomes an `AppError` — axum's own rejection is a plain-text body, which would be the one response not matching the documented `ErrorDetail` schema.

## Test Coverage

`cargo llvm-cov` line coverage is ~83 % — ≈96 % excluding the `main.rs` server-bootstrap entrypoint, which is 90 uncovered lines. The IP-parsing logic was extracted from `main` into `config::parse_forwarded_allow_ips` so it can be unit-tested. The `download_pdf` timeout path is intentionally untested: the timeout is a fixed `DOWNLOAD_TIMEOUT` (30 s, in `index.rs`), so provoking it would mean a test that sleeps. What it produces is a `PlanError::Failed` from `transport_error` — the same arm every other transport fault takes, and `test_unreachable_host_refuses_to_start` covers it via a `.invalid` host that never resolves. `cache.rs`'s remaining gaps are `put`'s write/rename error arms — reaching them needs a directory that turns unwritable *between* `from_env` and the write, and they all do the same thing (WARN, carry on) as the `from_env` path that is covered. `errors.rs` shows two uncovered lines for the `is_server_error()` branch in `into_response`, which no current variant can reach; see above for why it stays.

The split follows where things can fail: `tests/test_index.rs` drives `build_index`/`AppState::build` and owns every download and parse fault (mockito, the size caps, retired plans); `tests/test_cache.rs` drives the same function with an enabled cache and owns the four decisions in the table above; `tests/test_api.rs` drives the router over a seeded or fixture-built index and owns what a client can still observe (hit, miss, bad parameter). Helpers more than one binary needs — the fixture bytes, `plan`, `mock_fixture`, `temp_dir`, `state_from_fixture`, `body_to_json`, `get`, `EventRecorder` — live in `tests/common/mod.rs`, which carries a blanket `#![allow(dead_code)]` because each binary uses a different subset.

Every `build_index` call in `test_index.rs` passes `&PdfCache::disabled()`, which is what keeps those tests about downloads and nothing else. `test_cache.rs` gives each test its own `temp_dir(…)` (pid + nanos, like `write_temp` in `test_config.rs` — there is no `tempfile` dependency) and cleans it up at the end rather than in a `Drop` guard, so a failing assertion leaves the directory behind to look at.

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`; `test_the_source_is_read_once_and_never_again` uses `.expect(1)` + `assert_async()` to pin the property this design exists for — five requests, four of them misses, one fetch. District names with special chars are URL-encoded with `urlencoding::encode`. The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.

Note: `test_missing_district_parameter_returns_400` checks for `StatusCode::BAD_REQUEST` — axum 0.8 changed missing-query-param responses from 422 to 400.

Tests that assert on log output (`EventRecorder` in `tests/common`, `TraceRecorder` in `test_middleware.rs`) install the subscriber with `tracing::subscriber::set_default`, which is **thread-local**. Two things this depends on:

- `#[tokio::test]`'s current-thread runtime keeps the work on the calling thread. A `multi_thread` flavour — or an assertion on something logged inside the `spawn_blocking` parse — would silently record nothing.
- A permissive **global** subscriber must be installed first (`init_global_tracing` / `init_tracing`). `tracing` caches each callsite's `Interest` globally; without a global subscriber it is computed against `NoSubscriber` and cached as "never", and the thread-local recorder is then skipped before the dispatcher is consulted. Since tests run in parallel, omitting this makes the assertions pass or fail depending on which test reached the callsite first — it produced a genuine flake, not a theoretical one.

## `plans.yaml`

`pages` is passed directly to `index_districts`, which parses the comma-separated 1-based page numbers and uses them as 0-based indices for `pdf_oxide`. A page number past the end of the document is a `PlanError::Failed`, and therefore a refused startup — a wrong `pages` value cannot survive as a district quietly missing its dates.

`url` is validated in `config::validate_plan_url` at load time — scheme must be `http`/`https`, and the URL **path** must end in `.pdf`. Matching on the path rather than the whole string is what lets a link carry a query string or fragment (`…/Abfuhrplan_2027.pdf?v=2`). This is the **only** place the rule lives: `download.rs` used to repeat it as a guard, but every URL it can be handed has already been through `load_plans`, so the second copy asserted nothing and had its own hand-rolled path parsing to keep in step. `test_load_plans_rejects_non_pdf_url` in `test_config.rs` is what pins it.

## Known costs, deliberately not fixed

- **The index is only as fresh as the process, and now also as fresh as the cache.** A changed `plans.yaml` is picked up on restart and not before; a corrected PDF under an *unchanged* URL additionally waits out `PDF_CACHE_TTL` (a month by default), because a fresh cache entry is used without asking the source. Restarting with `PDF_CACHE_TTL=0s` — or deleting the cache directory — forces the refetch.
- **A start can now succeed on data nobody re-checked.** With the source down and an expired copy on disk, the process starts and serves last known dates instead of refusing. The only signal is the `serving a stale cached copy` WARN — there is no unhealthy status and no restart count to notice. Chosen over the alternative it replaces: a boot-time outage used to be a restart loop that could last as long as the outage, over data that changes once a year.
- **Nothing ever prunes the cache directory.** A retired plan's file stays until someone deletes it. One file per plan URL ever configured, capped at `MAX_PDF_BYTES` each — bounded by how often `plans.yaml` changes, which is once a year.
- **The whole index lives in memory, unbounded by anything but `MAX_PDF_BYTES` per plan at build time.** Fine for ~50 districts across one or two plans; it is not a design that scales to hundreds of plans.

## Docker

See the `docker-build` skill (`.claude/skills/docker-build/SKILL.md`) for the image build and runtime details.

## Key Conventions

- **All code comments must be in English** — never write German comments, even when the conversation is in German.
- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError` (request path) or `PlanError` (startup path).
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `DistrictIndex` is keyed by the **normalized** district name (`pdf_parser::normalize_district`), and every lookup has to normalize first. `DistrictIndex::from_pairs` normalizes what it is given, so a test cannot seed a key `lookup` could never reach.
