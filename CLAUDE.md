# Agent Instructions — blaue_tonne_rust

Axum service that extracts Blaue Tonne (blue bin / Altpapier) collection dates from PDFs published by Chiemgau Recycling for Landkreis Rosenheim.

See [README.md](README.md) for full project overview.

## Build & Test

```bash
cargo build                        # debug build
cargo build --release              # release build
cargo test                         # all tests (unit + integration)
cargo test --test test_pdf_parser  # PDF parser unit tests only
cargo test --test test_api         # API integration tests only
```

Tests require the fixture PDF at `tests/fixtures/lk_rosenheim_2026.pdf` (already committed).

## Architecture

| File | Responsibility |
|------|---------------|
| `src/main.rs` | Binary entry point: loads `plans.yaml`, calls `parse_forwarded_allow_ips`, binds to `BIND_ADDR` |
| `src/lib.rs` | `build_router`, OpenAPI spec (`ApiDoc`), module re-exports |
| `src/middleware.rs` | `resolve_client_ip` (IP-resolution middleware), `make_request_span` + `log_response` (TraceLayer callbacks) |
| `src/state.rs` | `AppState` (plans + two DashMap caches + reqwest `Client`), `ResolvedClientIp` extension type |
| `src/handlers.rs` | `health_check`, `lk_rosenheim_handler`, `download_pdf`, `dates_to_iso`; utoipa annotations |
| `src/pdf_parser.rs` | PDF → date extraction (public API: `get_dates`, `debug_extract`) |
| `src/config.rs` | `Plan` struct, `load_plans(path)`, `parse_forwarded_allow_ips(raw)` |
| `src/errors.rs` | `AppError` enum → `IntoResponse` (404/400/500/504) |
| `plans.yaml` | PDF URLs and page ranges (single source of truth) |

`AppState` holds `Arc<Vec<Plan>>`, two `Arc<DashMap<_>>` caches (PDF bytes keyed by URL; dates keyed by district name), and a `reqwest::Client` with a 30 s timeout. All caches are populated lazily on first request.

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Returns `{"status":"healthy"}` |
| `GET` | `/lk_rosenheim?district=<name>` | RFC 3339 UTC dates array for the given district |
| `GET` | `/docs` | Swagger UI (utoipa) |
| `GET` | `/docs/openapi.json` | OpenAPI JSON spec |

## Middleware & Request Pipeline

Layer order in `build_router`: `ip_middleware` is added last (`.layer()`) so it is outermost — it runs **before** `TraceLayer`, ensuring the span already has `client_ip` populated. The middleware logic lives in `src/middleware.rs`.

1. **`ip_middleware`** — `middleware::resolve_client_ip`, wired up via `axum::middleware::from_fn_with_state` with the `FORWARDED_ALLOW_IPS` allowlist as state. If the connecting peer is in the allowlist, the leftmost `X-Forwarded-For` entry is used; otherwise the socket IP is used. Falls back to `127.0.0.1` in unit tests (no `ConnectInfo`). Inserts `ResolvedClientIp` extension.
2. **`TraceLayer`** — uses `middleware::make_request_span` to create a span per request (method, URI, client_ip) and `middleware::log_response` to log response status + latency_ms. Most requests use an `info_span!`/INFO; `/health` requests use a `trace_span!` and are logged at TRACE only (high-frequency health checks would otherwise flood the logs). `log_response` recovers the level from the span's metadata.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PLANS_PATH` | `plans.yaml` | Path to plans YAML config |
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address to listen on |
| `FORWARDED_ALLOW_IPS` | *(empty)* | Comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; use `*` to trust all |
| `RUST_LOG` | `blaue_tonne_rust=info` | Standard `tracing-subscriber` filter. When unset, falls back to `blaue_tonne_rust=info`; when set it takes full control (e.g. `blaue_tonne_rust=trace` surfaces the TRACE-level `/health` request logs). |

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Internal algorithm in `src/pdf_parser.rs` (backed by `pdf_oxide`):
1. `PdfDocument::extract_spans(page_idx)` returns `TextSpan`s, each with a `bbox` (`x`, `y`) and `text`.
2. `spans_to_rows`: sort spans by Y descending (PDF Y increases upward), then X ascending; group consecutive spans into a row while their Y delta is `<= Y_TOLERANCE = 5.0` pts. Each row is a `Vec<String>` of span texts (no per-character X-gap splitting — `pdf_oxide` already returns coherent spans).
3. `parse_date`: takes the last `DATE_LENGTH = 8` characters of a cell string and parses `%d.%m.%y` (e.g. `"Mo. 06.01.26"` → `06.01.26`).
4. `get_dates` returns up to ~24 dates per district (the row before + the row after the district-name row).
5. `debug_extract` (pub, `#[doc(hidden)]`) — returns `Result<Vec<Vec<String>>, AppError>` of reconstructed rows for debugging; used in `test_debug_extraction`.

50 districts are supported (see `DISTRICTS` constant in `tests/test_pdf_parser.rs`).

## `download_pdf` Validation (in `src/handlers.rs`)

1. Check `pdf_cache` first (returns clone if hit).
2. Reject URLs that don't end with `.pdf` (case-insensitive) → `AppError::InvalidUrl`.
3. HTTP GET; timeout → `ServiceUnavailable`; 404 → `PdfError("not found")`; non-2xx → `PdfError`.
4. Validate `content-type: application/pdf` → `AppError::InvalidUrl` if absent.
5. Cache bytes and return.

In `lk_rosenheim_handler`, a `PdfError` whose message contains `"not found"` is treated as a soft skip (`continue` to next plan); all other errors propagate immediately.

## Test Coverage

| Suite | File | Count |
|-------|------|-------|
| PDF parser unit tests (50 districts + 4 error/utility) | `tests/test_pdf_parser.rs` | 54 |
| API integration tests (incl. `download_pdf` HTTP paths via mockito) | `tests/test_api.rs` | 20 |
| Config tests (`load_plans`, `parse_forwarded_allow_ips`) | `tests/test_config.rs` | 10 |
| Middleware tests (`resolve_client_ip` via mini-router, span/log helpers) | `tests/test_middleware.rs` | 8 |
| `AppError::into_response` tests | `tests/test_errors.rs` | 4 |
| `parse_date` inline unit tests | `src/pdf_parser.rs` (`#[cfg(test)]`) | 4 |

`cargo llvm-cov` line coverage is ~85 % (≈96 % excluding the `main.rs` server-bootstrap entrypoint). The IP-parsing logic was extracted from `main` into `config::parse_forwarded_allow_ips` so it can be unit-tested. The `download_pdf` timeout→504 path is intentionally untested (fixed 30 s client timeout).

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`. District names with special chars are URL-encoded with `urlencoding::encode`. The middleware tests inject `ConnectInfo<SocketAddr>` via `Request::builder().extension(...)` to exercise the X-Forwarded-For trusted-proxy path.

Note: `test_missing_district_parameter_returns_422` checks for `StatusCode::BAD_REQUEST` (400) — axum 0.8 changed missing-query-param responses from 422 to 400.

## Error Status Codes

| `AppError` variant | HTTP status |
|--------------------|-------------|
| `DistrictNotFound` | 404 |
| `InvalidUrl(_)` | 400 |
| `ServiceUnavailable` | 504 Gateway Timeout |
| `PdfError(_)` | 500 |

Response body is always `{"detail": "<message>"}`.

## `plans.yaml` Schema

```yaml
plans:
  - url: "https://…/file.pdf"   # full PDF URL
    pages: "1,2"                # comma-separated page numbers (string)
```

`pages` is passed directly to `get_dates`, which parses the comma-separated 1-based page numbers and uses them as 0-based indices for `pdf_oxide`.

## Docker

Four-stage build (`cargo-chef`): `chef` base (`rust:1-slim-trixie` + `cargo-chef`) → `planner` (writes `recipe.json`) → `builder` (`cargo chef cook` caches deps, then `cargo build --release`) → `gcr.io/distroless/cc-debian13:nonroot` runtime (~60 MB). `reqwest` 0.13's `rustls` feature uses the `aws-lc-rs` crypto provider; its `aws-lc-sys` C code builds with the base image's gcc/libc6-dev via aws-lc-sys's cmake-less fallback (no `cmake`/`make` needed, verified by a `--no-cache` build), and still no OpenSSL, so no `libssl-dev`/`pkg-config`. `curl` **is** required in the builder because `utoipa-swagger-ui`'s build script downloads the Swagger UI assets with it. Runtime TLS trust comes from `rustls-platform-verifier` reading the distroless image's native CA bundle (`/etc/ssl/certs`), not compiled-in `webpki-roots`. The distroless runtime has no shell/curl, no `tini`, and no manual user (the `:nonroot` tag already runs as uid 65532). The binary runs as PID 1 and handles SIGINT/SIGTERM itself via `axum::serve(...).with_graceful_shutdown(shutdown_signal())` (`shutdown_signal` in `src/main.rs`) — without that an unhandled signal would be ignored by PID 1, so ctrl+c / `docker stop` wouldn't work. Health checks use the binary's own `healthcheck` subcommand (`blaue_tonne_rust healthcheck` → GET `/health`, exit 0/1) since curl isn't available. See `.dockerignore` for the build-context exclusions.

## Key Conventions

- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `dates_cache` is keyed by district name (`String`); `pdf_cache` is keyed by PDF URL (`String`).
- Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts.
- Network-dependent tests use `mockito` to avoid real HTTP calls.
- URL-encode district names with `urlencoding::encode` when building test URIs.
