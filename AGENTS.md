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
| `src/main.rs` | Binary entry point: loads `plans.yaml`, binds `0.0.0.0:8080` |
| `src/lib.rs` | `AppState`, `build_router`, all HTTP handlers |
| `src/pdf_parser.rs` | PDF → date extraction (public API: `get_dates`) |
| `src/config.rs` | `Plan` struct, `load_plans(path)` |
| `src/errors.rs` | `AppError` enum → `IntoResponse` (404/400/500/504) |
| `plans.yaml` | PDF URLs and page ranges (single source of truth) |

`AppState` contains two `Arc<DashMap<_>>` caches: one for raw PDF bytes (keyed by URL) and one for extracted dates (keyed by district name). Both are populated lazily on first request.

## PDF Parsing

District names in this PDF are rendered as character fragments (e.g. "Bad Aibling" → cells `["B","ad","A","ib","ling"]`). Matching strips whitespace from both the row text and the district name before comparing. Dates live on the row **before** and the row **after** the district name row. See `src/pdf_parser.rs` for details.

## Key Conventions

- **Edition 2024** — requires Rust ≥ 1.85.
- No `unwrap()` in production paths; errors propagate via `AppError`.
- `download_pdf` validates `content-type: application/pdf` before caching bytes.
- Date format from PDFs: `%d.%m.%y` (e.g. `06.01.26`). Returned as RFC 3339 UTC strings.
- Integration tests use `tower::ServiceExt::oneshot` (not `axum-test`) to avoid version conflicts.
- Network-dependent tests use `mockito` to avoid real HTTP calls.
- URL-encode district names with `urlencoding::encode` when building test URIs.
