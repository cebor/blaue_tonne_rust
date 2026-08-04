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

## Soft-skipped errors in the handler

In `lk_rosenheim_handler`, `PdfNotFound` and per-plan `DistrictNotFound` are soft skips (`continue` to the next plan — a district may live in a later plan's PDF); the final `all_dates.is_empty()` check turns "found nowhere" into a 404. All other errors propagate immediately.

## Error Responses

`AppError`'s `Display` text is the **internal** detail (upstream URLs, library error strings): it is logged, never serialized. Clients get the fixed `client_message()` for the variant. `into_response` logs at ERROR for any 5xx.

**Nothing a client can observe may reveal that this service fetches and parses PDFs from a third party** — not the message, not the status code, not the `/docs` response descriptions. Every source-side fault therefore collapses into 503 rather than 502/504: a gateway status is itself a statement about the architecture. `test_no_variant_discloses_the_data_source` asserts this over all variants at once against a list of giveaway substrings, so a message added later is covered without anyone remembering to extend the file. A new *variant* is caught by `assert_every_variant_is_covered` next to it — an exhaustive `match` that exists only to stop compiling when `AppError` grows.

`lk_rosenheim_handler` takes `Result<Query<DistrictQuery>, QueryRejection>` rather than a bare `Query` so the 400 also becomes an `AppError` — axum's own rejection is a plain-text body, which would be the one response not matching the documented `ErrorDetail` schema.

| Variant | Status | Client sees | Meaning |
|---------|--------|-------------|---------|
| `BadRequest` | 400 | Invalid or missing query parameter | The only caller-caused variant |
| `DistrictNotFound` | 404 | District not found | No plan's PDF contained the district |
| `PdfError` | 500 | Internal server error | Bytes downloaded but unparseable — our own fault |
| `Upstream`, `ServiceUnavailable` | 503 | Service temporarily unavailable… | Plan URL bad, source unreachable/non-2xx/not-a-PDF/timed out |
| `PdfNotFound` | 503 | Service temporarily unavailable… | Plan retired upstream; soft-skipped per plan |

## Test Coverage

`cargo llvm-cov` line coverage is ~85 % (≈96 % excluding the `main.rs` server-bootstrap entrypoint). The IP-parsing logic was extracted from `main` into `config::parse_forwarded_allow_ips` so it can be unit-tested. The `download_pdf` timeout path is intentionally untested (fixed 30 s client timeout); `test_errors.rs` covers the variant’s mapping instead.

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`. District names with special chars are URL-encoded with `urlencoding::encode`. The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.

Note: `test_missing_district_parameter_returns_422` checks for `StatusCode::BAD_REQUEST` (400) — axum 0.8 changed missing-query-param responses from 422 to 400.

## `plans.yaml`

`pages` is passed directly to `get_dates`, which parses the comma-separated 1-based page numbers and uses them as 0-based indices for `pdf_oxide`.

## Docker

See the `docker-build` skill (`.claude/skills/docker-build/SKILL.md`) for the image build and runtime details.

## Key Conventions

- **All code comments must be in English** — never write German comments, even when the conversation is in German.
- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `dates_cache` is keyed by district name (`String`); `pdf_cache` is keyed by PDF URL (`String`).
