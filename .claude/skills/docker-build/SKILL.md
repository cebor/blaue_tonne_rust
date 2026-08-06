---
name: docker-build
description: Docker image build and runtime details for blaue_tonne_rust — the cargo-chef multi-stage layout, why specific build dependencies are (and are not) installed, distroless runtime constraints, the writable /cache directory and its volume, TLS trust, PID 1 signal handling, and health checks. Use when editing the Dockerfile, .dockerignore, or debugging image builds and container startup.
---

# Docker

Five-stage build (`cargo-chef`): `chef` base (`rust:1-slim-trixie` + `cargo-chef`) → `planner` (writes `recipe.json`) → `builder` (`cargo chef cook` caches deps, then `cargo build --release`) → `cachedir` (see below) → `gcr.io/distroless/cc-debian13:nonroot` runtime (~60 MB).

## Build dependencies — what's needed and what isn't

`reqwest` 0.13's `rustls` feature uses the `aws-lc-rs` crypto provider; its `aws-lc-sys` C code builds with the base image's gcc/libc6-dev via aws-lc-sys's cmake-less fallback (no `cmake`/`make` needed, verified by a `--no-cache` build), and still no OpenSSL, so no `libssl-dev`/`pkg-config`.

`curl` **is** required in the builder because `utoipa-swagger-ui`'s build script downloads the Swagger UI assets with it.

## Runtime

Runtime TLS trust comes from `rustls-platform-verifier` reading the distroless image's native CA bundle (`/etc/ssl/certs`), not compiled-in `webpki-roots`.

The distroless runtime has no shell/curl, no `tini`, and no manual user (the `:nonroot` tag already runs as uid 65532).

The binary runs as PID 1 and handles SIGINT/SIGTERM itself via `axum::serve(...).with_graceful_shutdown(shutdown_signal())` (`shutdown_signal` in `src/main.rs`) — without that an unhandled signal would be ignored by PID 1, so ctrl+c / `docker stop` wouldn't work.

Health checks use the binary's own `healthcheck` subcommand (`blaue_tonne_rust healthcheck` → GET `/health`, exit 0/1) since curl isn't available.

## `/cache` — the only writable directory

The service caches downloaded plan PDFs (`src/cache.rs`, `PDF_CACHE_DIR=/cache` in the image). Getting a writable directory into a distroless image takes a whole extra stage, for two reasons that compound:

- No shell, so `RUN mkdir` is impossible in the runtime stage.
- uid 65532 cannot create `/cache` at runtime either — `/` is root-owned, so `create_dir_all` gets EACCES.

`COPY --chown=65532:65532` is therefore the only way in, and `COPY` needs a source. The `cachedir` stage (`FROM chef`, so no extra image pull) exists solely to be that source. It is a separate stage rather than a `RUN mkdir` in `builder` because it has nothing to do with building.

The `--chown` matters even when a volume is mounted over `/cache`: Docker copies an image path's content **and ownership** into a fresh **named** volume on first use, so the volume ends up owned by 65532. A **bind mount** does not — the host directory's ownership wins and has to be `chown 65532:65532`ed by hand, or the service starts with a warning and no cache.

No `VOLUME` instruction: it would create an anonymous volume on every `docker run` without `-v`, and nothing ever reaps those. The volume belongs to the caller — `.gitlab-ci.yml`'s deploy job passes `-v blaue_tonne_cache:/cache`, which is what carries the cache across the `docker rm` it does on every deploy.

An unwritable `/cache` is never fatal: the cache degrades to off with a WARN.

See `.dockerignore` for the build-context exclusions.
