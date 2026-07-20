# ─── Stage 1: chef (cargo-chef base) ──────────────────────────────────────────
FROM rust:1-slim-trixie AS chef
# curl is needed at build time by utoipa-swagger-ui's build script (downloads the
# Swagger UI assets). No other build deps: aws-lc-sys (via reqwest's rustls/aws-lc-rs)
# builds with the base image's gcc/libc6-dev; no cmake/make, no OpenSSL.
RUN apt-get update && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# ─── Stage 2: planner ─────────────────────────────────────────────────────────
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# ─── Stage 3: builder ─────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY plans.yaml ./plans.yaml
RUN cargo build --release

# ─── Stage 4: runtime (distroless, non-root) ──────────────────────────────────
FROM gcr.io/distroless/cc-debian13:nonroot
WORKDIR /app
COPY --from=builder /app/target/release/blaue_tonne_rust /usr/local/bin/blaue_tonne_rust
COPY --from=builder /app/plans.yaml /app/plans.yaml

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/blaue_tonne_rust", "healthcheck"]

ENV PLANS_PATH=/app/plans.yaml \
    BIND_ADDR=0.0.0.0:8080

# distroless :nonroot already runs as uid 65532; no USER/groupadd needed.
# No tini: the binary runs as PID 1 and handles SIGINT/SIGTERM itself via
# axum's graceful shutdown (see shutdown_signal in main.rs), so ctrl+c and
# `docker stop` terminate cleanly. The app spawns no children (no reaping).
ENTRYPOINT ["/usr/local/bin/blaue_tonne_rust"]
