# Agent Instructions — blaue_tonne_rust

Axum service that extracts Blaue Tonne (blue bin / Altpapier) collection dates from PDFs published by Chiemgau Recycling for Landkreis Rosenheim.

See [README.md](README.md) for full project overview.

## Build & Test

Tests require the fixture PDF at `tests/fixtures/lk_rosenheim_2026.pdf` (already committed).

## Middleware & Request Pipeline

`build_router` (`src/router.rs`) builds a **traced** sub-router and merges it into an untraced outer router. `Router::layer` only affects routes registered before it, so `/health` — registered on the outer router — carries neither middleware and is not traced at any `RUST_LOG` level. This keeps container health checks out of the logs. `tests/test_middleware.rs` pins it with a span-counting subscriber.

Within the traced sub-router, `ip_middleware` is added last (`.layer()`) so it is outermost and runs **before** `TraceLayer`, ensuring the span already has `client_ip`. The middleware logic lives in `src/middleware.rs`.

1. **`ip_middleware`** — `middleware::resolve_client_ip`, wired up via `axum::middleware::from_fn_with_state` with the `FORWARDED_ALLOW_IPS` allowlist as state. If the connecting peer is in the allowlist, the leftmost `X-Forwarded-For` entry is used; otherwise the socket IP. Falls back to `127.0.0.1` in unit tests (no `ConnectInfo`). Inserts the `ResolvedClientIp` extension.

   **`*` is resolved in `config::parse_forwarded_allow_ips`, not here.** It expands to the two default routes (`0.0.0.0/0`, `::/0`), which already contain every address, so "trust every peer" stays a value in the allowlist and `resolve_client_ip` keeps its single `contains` check. Startup logs the two networks rather than the `*` that produced them.
2. **`TraceLayer`** — `middleware::make_request_span` creates an `info_span!` per request (method, URI, client_ip); `middleware::log_response` logs status + latency_ms at INFO.

`log_response` is **not** `DefaultOnResponse`: tower-http emits under the `tower_http::trace` target, which the default `RUST_LOG` fallback (`blaue_tonne_rust=info`) filters out. `test_response_is_logged_under_this_crates_target` pins this.

## Environment Variables

**The full table lives in [README.md](README.md#environment-variables)** and is the one to edit when a variable changes. Three whose behaviour matters elsewhere in this file:

- **`PLANS_PATH`** is read once, at startup. A new plan needs a restart.
- **`RUST_LOG`** takes full control when set; the `blaue_tonne_rust=info` fallback applies only when it is absent. `/health` is never logged at any level, because it is registered outside the traced router.
- **`PDF_CACHE_DIR`** unset means the default path; set-but-empty means the cache is off. One variable for both.

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Row reconstruction in `src/pdf_parser.rs` sorts `pdf_oxide` spans by Y descending (PDF Y increases upward), then X ascending, grouping them into a row while the Y delta stays within `Y_TOLERANCE`. No per-character X-gap splitting is needed — `pdf_oxide` already returns coherent spans.

50 districts are supported (see `DISTRICTS` in `tests/test_pdf_parser.rs`).

`index_districts` reads a whole plan in one pass and returns `district → dates`. It is the only entry point; there is no per-district search function. A row that carries dates itself is skipped as a key (it is a date row, not a name row), and a name row without dates around it is not an entry. First occurrence wins, so pages are read in order.

## The index is built at startup

`build_index` (`src/index.rs`) downloads and parses every plan once, before `main` binds the listener. `AppState` holds nothing but the resulting `Arc<DistrictIndex>`; the `reqwest::Client` is local to the build, because after it returns the service does no network I/O. A request is `index.lookup(&normalize_district(name))` and nothing else.

**A plan that can be read from neither the source nor the cache is fatal.** There is no second attempt at request time, so starting anyway would serve a district short of its dates for the lifetime of the process. `main` logs the fault at ERROR and exits 1.

Loading `plans.yaml` fails the same way, through the same `match`-log-exit shape rather than an `expect`, so the detail (a path, a URL, a serde message) goes through tracing like every other startup fault.

**The one exception is an upstream 404**, which means the plan is gone — expected at the turn of the year, when last year's PDF goes offline while still listed in `plans.yaml`. It is skipped with a WARN naming the URL, once, at startup.

**`plans_indexed == 0` is fatal too.** An empty index would answer "District not found" for every name in the county (`test_only_plan_retired_refuses_to_start`, `test_no_plans_refuses_to_start`). A plan served from the cache counts as indexed.

## The plan PDFs are cached on disk

`src/cache.rs`. `build_index` takes a `&PdfCache` and consults it before every download, so a normal start does no network I/O at all.

`PdfCache { dir: Option<PathBuf>, ttl: Duration }`. **`dir: None` is a disabled cache and is not a special case anywhere** — `get`/`put` are no-ops, and the three ways to get there (empty `PDF_CACHE_DIR`, a directory that could not be created, `PdfCache::disabled()` in tests) converge on the same code. `build_index` therefore has no "if caching is on" branches.

**Unset `PDF_CACHE_DIR` ≠ empty `PDF_CACHE_DIR`.** Unset picks the default location; empty switches the cache off. `config::cache_dir_from` holds the resolution logic as a pure function of the three env values, so it is testable without mutating process-wide environment — only `PdfCache::from_env`'s two edge cases need `set_var`, and they share one serial `#[test]`.

**Key:** `{sha256(url)[..16 hex]}-{URL's own file name}`. The hash makes it unique and filesystem-safe for any URL (including one with `/` or `..` in it); the readable tail lets `ls` on the cache directory say which plan is which. Not `DefaultHasher`, whose output is not stable across Rust releases. `put` writes to a sibling temp file (the key with its extension replaced by `tmp-{pid}`) and renames it into place, so a crash cannot leave a half-written PDF. It is called **after** the parse succeeds, so bytes that will not parse are never written.

`put` returns `()`, not `Result`. Every fault in the module — unwritable directory, unreadable file, failed rename — degrades to "no cache" plus a log line. The cache is an optimization, never a data path.

The I/O is blocking `std::fs`: it happens only inside `build_index`, before the listener binds, so there is no executor to starve and no need for tokio's `fs` feature.

Four `build_index` decisions, each pinned by a test in `tests/test_cache.rs`:

| Situation | Behaviour | Why |
|---|---|---|
| Fresh cache entry | Used, no request | The point of the feature |
| Fresh entry that will not parse | WARN, refetch | Otherwise a corrupt file is a startup error **no restart can clear** |
| Download fails, expired entry exists | WARN "serving a stale cached copy", start anyway | Turns a boot-time outage from a restart loop into a degraded start |
| Download 404s, entry exists | Entry ignored, plan skipped | 404 means *retired*; the copy would keep a withdrawn plan alive |

If the stale copy does not parse either, the **download** error is returned, not the parse error.

**One INFO line per indexed plan**, from a single callsite at the end of the loop body:

```
indexed plan url=… source="cache" age_secs=9 districts=52
```

`source` is `url`, `cache`, or `stale-cache`; `age_secs` is 0 for `url` and the file's age otherwise. A *field* rather than three different messages, so "downloaded or read off disk?" is answerable by filtering. `test_a_second_start_reads_the_plan_from_disk_and_makes_no_request` asserts on the two values, so the field is part of the interface. `stale-cache` comes *in addition to* the WARN, which stays.

Dates for a district that several plans carry are concatenated in plan order — not deduplicated, not sorted. That lets a district keep both the old and the new plan's dates while both are configured.

**An empty or whitespace-only `district` is rejected up front** (400). `normalize_district` strips whitespace, so both normalize to `""`; without the guard they would fall through to a plain index miss and answer 404.

**Consequence: `plans.yaml` is read exactly once.** A new plan, or a corrected PDF under an unchanged URL, needs a restart.

## Download Size Cap

`download.rs` caps plan PDFs at `MAX_PDF_BYTES` (16 MiB) with **two** guards: a `Content-Length` pre-check and an accumulating check inside the `chunk()` read loop. The second is not redundant — `Content-Length` can be absent (chunked transfer) or wrong. Both produce the same `PlanError::Failed` variant, so their tests assert on the message (`"advertises"` vs `"exceeds the"`); the variant alone would let either guard be deleted silently.

## Two error types, split by whether anyone can see them

Both live in `src/errors.rs`. A new variant belongs in `AppError` only if a client can actually receive it.

**`AppError` is what a request can be answered with:**

| Variant | Status | Client sees | Meaning |
|---------|--------|-------------|---------|
| `BadRequest` | 400 | Invalid or missing query parameter | Missing/undeserializable `district`, or one that is empty after normalization |
| `DistrictNotFound` | 404 | District not found | The district is in no plan |

That is the complete list, because the route is: normalize, reject `""`, look up. The `#[utoipa::path]` annotation lists 200/400/404 to match.

**`PlanError` is what reading a plan can fail with**, and it never becomes a response — `build_index` runs before the listener binds, and `main` logs the fault and exits 1.

| Variant | Meaning |
|---------|---------|
| `Retired(url)` | Upstream 404: the plan is gone. Exists for **control flow** — `build_index` matches on it to skip the plan with a WARN |
| `Failed(detail)` | Everything else: unreachable, non-2xx, wrong content-type, timed out, over the size cap, unparseable bytes. All fatal, all handled identically |

`Failed` is one variant and not five: what tells those faults apart is the **message**, which is why the tests assert on substrings (`"advertises"`, `"exceeds the"`, `"text/html"`, `"cross-reference"`) rather than on the variant.

Keeping startup faults out of `AppError` is what lets `IntoResponse` carry status codes and client messages for exactly the cases that can occur, and lets `test_errors.rs` hold the no-disclosure invariant over messages that are actually served.

**The response body is the `ErrorDetail` struct** in `errors.rs`, next to the only thing that produces one. It is both what `into_response` serializes and what `/docs` advertises, so the served shape and the documented schema cannot drift apart. Serializing a struct is also why `serde_json` is a dev-dependency and not a runtime one: only the tests deserialize.

`AppError`'s `Display` is the internal detail (axum's rejection text) and is logged, never serialized. `into_response` logs at DEBUG for 4xx so caller noise stays off the default filter, and keeps the ERROR branch for a future 5xx variant. `PlanError`'s `Display` may name plan URLs and library text freely.

**Nothing a client can observe may reveal that this service fetches and parses PDFs from a third party** — not the message, not the status code, not the `/docs` response descriptions. It holds by construction: no `AppError` variant has a plan URL to leak. `test_no_variant_discloses_the_data_source` covers what may be *added*; `assert_every_variant_is_covered` next to it is an exhaustive `match` that stops compiling when `AppError` grows.

`lk_rosenheim_handler` takes `Result<Query<DistrictQuery>, QueryRejection>` rather than a bare `Query`, so the 400 also becomes an `AppError` — axum's own rejection is a plain-text body, which would be the one response not matching the documented `ErrorDetail` schema.

## Test Coverage

Which test binary owns which failure mode, the deliberate coverage gaps, and the thread-local-subscriber rule the log-asserting tests depend on: [tests/CLAUDE.md](tests/CLAUDE.md), which loads when working under `tests/`.

## `plans.yaml`

**A plan is a URL and nothing else** — literally: `plans` deserializes into `Vec<String>`, there is no `Plan` struct, and `build_index` takes `&[String]`. `index_districts` reads every page of the PDF, from `0..doc.page_count()`. There is no page selection to configure, because the row shape already is the filter: a page carrying no district table produces no name row with dates around it and contributes nothing. That also means no config value can fall out of step with a re-paginated PDF.

`Config` carries `#[serde(deny_unknown_fields)]`, so a top-level key the service does not read aborts startup instead of being silently ignored, and a plan written as a mapping (`- url: …`) fails on the type rather than being half-read. `test_load_plans_rejects_a_top_level_key_the_service_does_not_read` and `test_load_plans_rejects_a_plan_written_as_a_mapping` pin both.

`test_every_page_of_the_document_is_read` in `test_pdf_parser.rs` asserts on a district from each page of the fixture, so an index that stopped early fails.

`url` is validated in `config::validate_plan_url` at load time — scheme must be `http`/`https`, and the URL **path** must end in `.pdf`. Matching on the path rather than the whole string lets a link carry a query string or fragment (`…/Abfuhrplan_2027.pdf?v=2`). This is the **only** place the rule lives; `download.rs` does not repeat it, because every URL it can be handed has already been through `load_plans`. `test_load_plans_rejects_non_pdf_url` in `test_config.rs` pins it.

## Known costs, deliberately not fixed

- **The index is only as fresh as the process, and as fresh as the cache.** A changed `plans.yaml` is picked up on restart and not before; a corrected PDF under an *unchanged* URL additionally waits out `PDF_CACHE_TTL` (a month by default). Restarting with `PDF_CACHE_TTL=0s` — or deleting the cache directory — forces the refetch.
- **A start can succeed on data nobody re-checked.** With the source down and an expired copy on disk, the process starts and serves last known dates. The only signal is the `serving a stale cached copy` WARN; there is no unhealthy status and no restart count.
- **Nothing ever prunes the cache directory.** One file per plan URL ever configured, capped at `MAX_PDF_BYTES` each.
- **The whole index lives in memory**, unbounded by anything but `MAX_PDF_BYTES` per plan at build time. Fine for ~50 districts across one or two plans; not a design that scales to hundreds of plans.

## Docker

See the `docker-build` skill (`.claude/skills/docker-build/SKILL.md`) for the image build and runtime details.

## Key Conventions

- **All code comments must be in English** — never write German comments, even when the conversation is in German.
- **Comments describe what the code does not show.** Technical and short, no narrative, no rationale essays, no history of what changed. The current state of the code is the source of truth. Doc comments (`///`, `//!`) may be longer, but describe contract and behaviour, not justification.
- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError` (request path) or `PlanError` (startup path).
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `DistrictIndex` is keyed by the **normalized** district name (`pdf_parser::normalize_district`), and every lookup has to normalize first. `DistrictIndex::from_pairs` normalizes what it is given, so a test cannot seed a key `lookup` could never reach.
