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
| `src/main.rs` | Binary entry point: loads `plans.yaml`, parses `FORWARDED_ALLOW_IPS`, binds to `BIND_ADDR` |
| `src/lib.rs` | `build_router`, OpenAPI spec (`ApiDoc`), module re-exports |
| `src/state.rs` | `AppState` (plans + two DashMap caches + reqwest `Client`), `ResolvedClientIp` extension type |
| `src/handlers.rs` | `health_check`, `lk_rosenheim_handler`, `download_pdf`, `dates_to_iso`; utoipa annotations |
| `src/pdf_parser.rs` | PDF → date extraction (public API: `get_dates`, `debug_extract`) |
| `src/config.rs` | `Plan` struct, `load_plans(path)` |
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

Layer order in `build_router`: `ip_middleware` is added last (`.layer()`) so it is outermost — it runs **before** `TraceLayer`, ensuring the span already has `client_ip` populated.

1. **`ip_middleware`** — resolves real client IP. If the connecting peer is in `FORWARDED_ALLOW_IPS`, the leftmost `X-Forwarded-For` entry is used; otherwise the socket IP is used. Falls back to `127.0.0.1` in unit tests (no `ConnectInfo`). Inserts `ResolvedClientIp` extension.
2. **`TraceLayer`** — creates a `tracing::info_span!` per request (method, URI, client_ip); logs response status + latency_ms at INFO.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PLANS_PATH` | `plans.yaml` | Path to plans YAML config |
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address to listen on |
| `FORWARDED_ALLOW_IPS` | *(empty)* | Comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; use `*` to trust all |
| `RUST_LOG` | — | Standard `tracing-subscriber` filter (e.g. `blaue_tonne_rust=debug`) |

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the concatenated row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row.

Internal algorithm in `src/pdf_parser.rs`:
1. `TableExtractor` (implements `OutputDev`) collects per-character `(x, y, ch)` triples via `output_doc_page`.
2. `reconstruct_rows`: sort by Y descending, group by `ROW_Y_TOLERANCE = 3.0` pts, then split each row into cells by X gaps > `CELL_X_GAP = 4.0` pts. Cell-advancement uses `prev_x = ch.x + 1.0` to avoid re-splitting multi-byte characters.
3. `parse_date`: takes the last 8 characters of a cell string and parses `%d.%m.%y` (e.g. `"Mo. 06.01.26"` → `06.01.26`).
4. `get_dates` returns up to ~24 dates per district (two rows × many cells).
5. `debug_extract` (pub, `#[doc(hidden)]`) — returns raw table rows for debugging; used in `test_debug_extraction`.

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
| API integration tests | `tests/test_api.rs` | 12 |
| `parse_date` inline unit tests | `src/pdf_parser.rs` (`#[cfg(test)]`) | 4 |

Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts. Network tests use `mockito`. District names with special chars are URL-encoded with `urlencoding::encode`.

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

`pages` is passed directly to `get_dates` and forwarded to `pdf-extract`.

## Docker

Two-stage build: `rust:1-slim` builder → `debian:bookworm-slim` runtime. Build dependencies: `libssl-dev`, `pkg-config`. Runtime uses `tini` as PID-1 init and a non-root `axum` user. Layer-cache trick: stub `src/main.rs` + `src/lib.rs` are built first to cache compiled dependencies before real sources are copied.

## Key Conventions

- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings (`Utc.from_utc_datetime(&dt).to_rfc3339()`).
- `dates_cache` is keyed by district name (`String`); `pdf_cache` is keyed by PDF URL (`String`).
- Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts.
- Network-dependent tests use `mockito` to avoid real HTTP calls.
- URL-encode district names with `urlencoding::encode` when building test URIs.
