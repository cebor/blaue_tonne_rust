# Blaue Tonne API (Rust)

Axum service that extracts waste collection dates from PDF schedules and exposes them via HTTP API. Rust rewrite of [blaue_tonne](../blaue_tonne). Currently supports the Rosenheim district (Landkreis Rosenheim).

## Features

- **PDF Parsing**: Downloads and parses the configured waste collection schedules **once, at startup**
- **In-Memory Index**: Every district is held in memory afterwards, so a request is a map lookup and the service does no network I/O at all
- **On-Disk Cache**: Plans are kept on disk for a month, so a restart needs no network — and still starts from the last known copy when the source is down
- **RESTful API**: Simple HTTP endpoints for date retrieval and health checks

## Project Structure

```
blaue_tonne_rust/
├── src/
│   ├── main.rs                   # Binary entry point (server setup, healthcheck subcommand)
│   ├── lib.rs                    # Module declarations and re-exports
│   ├── router.rs                 # Router builder (routes, Swagger UI, middleware layering)
│   ├── openapi.rs                # OpenAPI spec (utoipa ApiDoc)
│   ├── middleware.rs             # Client-IP resolution middleware + TraceLayer callbacks
│   ├── state.rs                  # AppState (the district index)
│   ├── index.rs                  # DistrictIndex + the startup build over all plans
│   ├── handlers.rs               # HTTP handlers, utoipa annotations
│   ├── download.rs               # PDF download with validation and a size cap
│   ├── cache.rs                  # On-disk cache for the downloaded plan PDFs
│   ├── config.rs                 # YAML config loading
│   ├── errors.rs                 # AppError (what a request answers with) + PlanError (startup only)
│   └── pdf_parser.rs             # PDF table extraction and date parsing
├── tests/
│   ├── common/mod.rs             # Helpers shared by the integration test binaries
│   ├── test_api.rs               # Integration tests for HTTP endpoints
│   ├── test_index.rs             # Startup index build (download/parse faults, mock HTTP server)
│   ├── test_cache.rs             # On-disk cache: hits, expiry, stale fallback, off switch
│   ├── test_pdf_parser.rs        # Unit tests for PDF parsing
│   ├── test_config.rs            # Config loading / allowlist parsing tests
│   ├── test_middleware.rs        # Client-IP middleware tests
│   ├── test_errors.rs            # AppError → HTTP response tests
│   └── fixtures/
│       └── lk_rosenheim_2026.pdf
├── plans.yaml                    # Configuration: PDF URLs and page ranges
├── Cargo.toml
├── Dockerfile                    # Multi-stage Docker build
└── README.md                     # This file
```

**Key Files:**
- `src/handlers.rs` – HTTP handlers (`health_check`, `lk_rosenheim_handler`)
- `src/index.rs` – `DistrictIndex` and `build_index`, which reads every plan once at startup
- `src/state.rs` – `AppState`, an `Arc<DistrictIndex>` and nothing else
- `src/pdf_parser.rs` – PDF text extraction via `pdf_oxide`, row reconstruction, date parsing
- `plans.yaml` – Single-source config for PDF URLs and page ranges (1-indexed)

**Startup:** the plans are read before the listener binds — from the on-disk cache when a copy is still fresh, from the source otherwise. A plan that can be read from neither is fatal: the process logs the reason and exits 1 rather than starting with an index that would answer some districts short of their dates for the rest of its lifetime. Two exceptions:

- A plan whose PDF is **gone upstream** (HTTP 404) is expected at the turn of the year. It is skipped with a warning, and only becomes fatal if no plan is left. A cached copy does not revive it — 404 means retired.
- If the **source is unreachable** and an expired copy is on disk, the process starts on that copy and warns loudly (`serving a stale cached copy`). The dates were correct when they were fetched, and a plan changes once a year; refusing to start would mean an outage at boot keeps the service down for as long as it lasts.

A changed `plans.yaml` requires a restart. A corrected PDF under an *unchanged* URL requires a restart **and** an expired or deleted cache entry — see [Configuration](#configuration).

## API Endpoints

### Get Collection Dates
```bash
GET /lk_rosenheim?district=<name>
```

Returns a JSON array of ISO-8601 datetime strings for the requested district.

**Example:**
```bash
curl 'http://localhost:8080/lk_rosenheim?district=Aschau'
# => ["2026-01-03T00:00:00+00:00", "2026-01-30T00:00:00+00:00", ...]
```

**Response codes:**
| Code | Meaning |
|------|---------|
| 200  | Dates found |
| 400  | Missing or invalid `district` query parameter (an empty or whitespace-only name is invalid) |
| 404  | The district is in none of the configured plans |

There is no 5xx on this route: the plans are read at startup, so a running process has already proven it can read them.

Errors carry a generic `{"detail": "..."}` message. Responses deliberately disclose **nothing** about where the data comes from: no upstream URLs, no library error text, and no hint that the dates are extracted from PDFs published by a third party. The real cause is logged.

### Health Check
```bash
GET /health
```

Returns `{"status": "healthy"}`.

### API Docs
```bash
GET /docs                   # Swagger UI
GET /docs/openapi.json      # OpenAPI JSON spec
```

## Development

### Prerequisites

- Rust 1.85+ (edition 2024)

### Local Setup

```bash
# Build (debug)
cargo build

# Run development server
cargo run

# Run production build
cargo run --release
```

The server binds to `0.0.0.0:8080` by default. Override with the `BIND_ADDR` env var:

```bash
BIND_ADDR=127.0.0.1:9090 cargo run
```

### Running Tests

```bash
# Run all tests
cargo test

# Run only PDF parser unit tests
cargo test --test test_pdf_parser

# Run only API integration tests
cargo test --test test_api

# With output
cargo test -- --nocapture
```

**Test coverage:**
- 57 PDF parser tests: one per district against the fixture PDF, plus district-name normalization
- 14 index-build tests (startup faults, retired plans, download size caps, mock HTTP server)
- 12 API integration tests (health, lookups, error responses)
- 14 config tests (incl. plan-URL validation), 9 middleware tests, 7 error-response tests
- 5 inline unit tests for internal parsing helpers

### Docker

```bash
# Build image
docker build -t blaue_tonne_rust .

# Run container, with a named volume for the plan cache
docker run --rm -p 8080:8080 -v blaue_tonne_cache:/cache blaue_tonne_rust
```

The image runs as uid 65532 and keeps its plan cache in `/cache`. Without a volume the cache still survives a container *restart* — enough to keep a source outage from turning into a restart loop — but `docker rm` (and therefore every deploy) throws it away.

Use a **named volume**, as above: Docker copies `/cache`'s ownership from the image into a fresh named volume, so the process can write it. A **bind mount** does not — the host directory's own ownership wins, so `chown 65532:65532` it first, or the service will start with a warning and no cache.

## Configuration

Edit `plans.yaml` to add or modify PDF sources:

```yaml
plans:
  - url: "https://example.com/schedule.pdf"
    pages: "1,2"  # Comma-separated page numbers (1-indexed)
```

The config path can be overridden with the `PLANS_PATH` env var. Changes take effect on the next restart — the plans are read once, when the process starts.

Plan URLs are validated at startup: the scheme must be `http`/`https` and the URL *path* must end in `.pdf` (a query string or fragment is fine). A URL that fails this aborts the process with an explicit message, rather than turning into a 503 on every request.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PLANS_PATH` | `plans.yaml` | Path to the plans config. Read once, at startup |
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address to listen on |
| `FORWARDED_ALLOW_IPS` | *(empty)* | Comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; `*` trusts all |
| `RUST_LOG` | `blaue_tonne_rust=info` | `tracing-subscriber` filter. `/health` is never logged, at any level |
| `PDF_CACHE_DIR` | `$XDG_CACHE_HOME/blaue_tonne_rust`, else `$HOME/.cache/…`, else `$TMPDIR/…` | Where downloaded plan PDFs are kept (`/cache` in the container) |
| `PDF_CACHE_TTL` | `30d` | How long a cached plan counts as fresh. `30d`, `12h`, `90m`, `45s`, or a bare number of seconds |

**Turning the cache off:** set `PDF_CACHE_DIR` to an *empty* value. Leaving it **unset** is different — that selects the default path. There is one variable for both the location and the off switch, so the two can never contradict each other.

**Forcing a refetch** of a PDF that changed under an unchanged URL: delete the cache directory, or start once with `PDF_CACHE_TTL=0s`. A bad value in either variable warns and falls back to the default rather than keeping the service from starting, and a cache directory that cannot be written warns and disables the cache — it is an optimization, never a data path.

## License

See [LICENSE](LICENSE) file for details.
