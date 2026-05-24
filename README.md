# Blaue Tonne API (Rust)

Axum service that extracts waste collection dates from PDF schedules and exposes them via HTTP API. Rust rewrite of [blaue_tonne](../blaue_tonne). Currently supports the Rosenheim district (Landkreis Rosenheim).

## Features

- **PDF Parsing**: Automatically downloads and parses waste collection schedules from PDF files
- **In-Memory Caching**: Caches both downloaded PDFs and extracted dates for fast subsequent requests
- **RESTful API**: Simple HTTP endpoints for date retrieval and health checks

## Project Structure

```
blaue_tonne_rust/
├── src/
│   ├── main.rs            # Binary entry point (server setup, config loading)
│   ├── lib.rs             # App state, router builder, HTTP handlers
│   ├── config.rs          # YAML config loading
│   ├── errors.rs          # AppError enum with IntoResponse
│   └── pdf_parser.rs      # PDF table extraction and date parsing
├── tests/
│   ├── test_api.rs        # Integration tests for HTTP endpoints
│   ├── test_pdf_parser.rs # Unit tests for PDF parsing
│   └── fixtures/
│       └── lk_rosenheim_2026.pdf
├── plans.yaml             # Configuration: PDF URLs and page ranges
├── Cargo.toml
├── Dockerfile             # Multi-stage Docker build
└── README.md              # This file
```

**Key Files:**
- `src/lib.rs` – Axum app with handlers, in-memory DashMap cache, YAML config loading
- `src/pdf_parser.rs` – PDF text extraction via `pdf-extract` (lopdf), table reconstruction, date parsing
- `plans.yaml` – Single-source config for PDF URLs and page ranges (1-indexed)

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
| 400  | Invalid/non-PDF URL in config |
| 404  | District not found |
| 504  | Upstream PDF server unavailable |

### Health Check
```bash
GET /health
```

Returns `{"status": "healthy"}`.

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
- 54 parametrized district tests verifying date extraction from the fixture PDF
- 12 API integration tests (health, caching, error responses, mock HTTP server)
- 4 unit tests for internal parsing helpers

### Docker

```bash
# Build image
docker build -t blaue_tonne_rust .

# Run container
docker run --rm -p 8080:8080 blaue_tonne_rust
```

## Configuration

Edit `plans.yaml` to add or modify PDF sources:

```yaml
plans:
  - url: "https://example.com/schedule.pdf"
    pages: "1,2"  # Comma-separated page numbers (1-indexed)
```

The config path can be overridden with the `PLANS_PATH` env var.

## License

See [LICENSE](LICENSE) file for details.
