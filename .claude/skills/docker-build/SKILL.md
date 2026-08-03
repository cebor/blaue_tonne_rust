---
name: docker-build
description: Docker image build and runtime details for blaue_tonne_rust — the cargo-chef multi-stage layout, why specific build dependencies are (and are not) installed, distroless runtime constraints, TLS trust, PID 1 signal handling, and health checks. Use when editing the Dockerfile, .dockerignore, or debugging image builds and container startup.
---

# Docker

Four-stage build (`cargo-chef`): `chef` base (`rust:1-slim-trixie` + `cargo-chef`) → `planner` (writes `recipe.json`) → `builder` (`cargo chef cook` caches deps, then `cargo build --release`) → `gcr.io/distroless/cc-debian13:nonroot` runtime (~60 MB).

## Build dependencies — what's needed and what isn't

`reqwest` 0.13's `rustls` feature uses the `aws-lc-rs` crypto provider; its `aws-lc-sys` C code builds with the base image's gcc/libc6-dev via aws-lc-sys's cmake-less fallback (no `cmake`/`make` needed, verified by a `--no-cache` build), and still no OpenSSL, so no `libssl-dev`/`pkg-config`.

`curl` **is** required in the builder because `utoipa-swagger-ui`'s build script downloads the Swagger UI assets with it.

## Runtime

Runtime TLS trust comes from `rustls-platform-verifier` reading the distroless image's native CA bundle (`/etc/ssl/certs`), not compiled-in `webpki-roots`.

The distroless runtime has no shell/curl, no `tini`, and no manual user (the `:nonroot` tag already runs as uid 65532).

The binary runs as PID 1 and handles SIGINT/SIGTERM itself via `axum::serve(...).with_graceful_shutdown(shutdown_signal())` (`shutdown_signal` in `src/main.rs`) — without that an unhandled signal would be ignored by PID 1, so ctrl+c / `docker stop` wouldn't work.

Health checks use the binary's own `healthcheck` subcommand (`blaue_tonne_rust healthcheck` → GET `/health`, exit 0/1) since curl isn't available.

See `.dockerignore` for the build-context exclusions.
