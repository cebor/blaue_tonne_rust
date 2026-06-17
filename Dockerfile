# ─── Stage 1: builder ────────────────────────────────────────────────────────
FROM rust:1-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev curl && rm -rf /var/lib/apt/lists/*

# Layer-cache trick: build dependencies first with a stub main, then overwrite
# with real sources. This avoids re-downloading/re-compiling dependencies on
# every source change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs && \
    echo '' > src/lib.rs && \
    cargo build --release && \
    rm -rf src

# Now copy real sources and rebuild (only recompiles changed crates)
COPY src ./src
COPY plans.yaml ./plans.yaml
RUN touch src/main.rs src/lib.rs && cargo build --release

# ─── Stage 2: runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl tini && \
    rm -rf /var/lib/apt/lists/*

# Non-root user
RUN groupadd -r axum && useradd -r -g axum axum

COPY --from=builder /app/target/release/blaue_tonne_rust /usr/local/bin/blaue_tonne_rust
COPY --chown=axum:axum plans.yaml /app/plans.yaml

USER axum
WORKDIR /app

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENV PLANS_PATH=/app/plans.yaml \
    BIND_ADDR=0.0.0.0:8080

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/blaue_tonne_rust"]
