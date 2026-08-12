# ─── Stage 1: chef (cargo-chef base) ──────────────────────────────────────────
FROM rust:1-slim-trixie AS chef
# curl: utoipa-swagger-ui's build script downloads the Swagger UI assets.
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

# ─── Stage 3b: cachedir ───────────────────────────────────────────────────────
# The COPY source for /cache below. Distroless has no shell for a `RUN mkdir`,
# and uid 65532 cannot create /cache itself because / is root-owned.
FROM chef AS cachedir
RUN mkdir -p /cache

# ─── Stage 4: runtime (distroless, non-root) ──────────────────────────────────
FROM gcr.io/distroless/cc-debian13:nonroot
WORKDIR /app
COPY --from=builder /app/target/release/blaue_tonne_rust /usr/local/bin/blaue_tonne_rust
COPY --from=builder /app/plans.yaml /app/plans.yaml

# Downloaded plan PDFs, owned by uid 65532 so the process can write here. An
# empty named volume mounted here inherits that ownership; a bind mount does not
# and has to be chowned to 65532:65532 by hand. No VOLUME instruction, which
# would create an unnamed volume on every `docker run` without -v.
COPY --from=cachedir --chown=65532:65532 /cache /cache

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/blaue_tonne_rust", "healthcheck"]

ENV PLANS_PATH=/app/plans.yaml \
    BIND_ADDR=0.0.0.0:8080 \
    PDF_CACHE_DIR=/cache \
    PDF_CACHE_TTL=30d

# distroless :nonroot already runs as uid 65532; no USER/groupadd needed.
# The binary runs as PID 1 and handles SIGINT/SIGTERM itself (shutdown_signal in
# main.rs); it spawns no children, so there is nothing to reap.
ENTRYPOINT ["/usr/local/bin/blaue_tonne_rust"]
