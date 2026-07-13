# ── Stage 1: Builder ──
FROM rust:1.96-slim-bookworm AS builder

ARG MAYAB_GIT_SHA=local
ARG MAYAB_BUILD_TIME=not-recorded
ARG MAYAB_RELEASE_VERSION=0.1.0
ARG MAYAB_BUILD_ENV=production
ENV MAYAB_GIT_SHA=${MAYAB_GIT_SHA} \
    MAYAB_BUILD_TIME=${MAYAB_BUILD_TIME} \
    MAYAB_RELEASE_VERSION=${MAYAB_RELEASE_VERSION} \
    MAYAB_BUILD_ENV=${MAYAB_BUILD_ENV}

RUN rustup set profile minimal
WORKDIR /build

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY mayab-arbitrage/Cargo.toml mayab-arbitrage/Cargo.toml
COPY mayab-arbitrage/benches ./mayab-arbitrage/benches
COPY mayab-cli/Cargo.toml mayab-cli/Cargo.toml
RUN mkdir -p mayab-arbitrage/src mayab-cli/src && \
    echo '' > mayab-arbitrage/src/lib.rs && \
    echo 'fn main() {}' > mayab-cli/src/main.rs && \
    echo 'fn main() {}' > mayab-cli/src/capture_tape.rs && \
    echo 'fn main() {}' > mayab-cli/src/verify_tape.rs && \
    cargo build --release --locked -p mayab-cli --bin mayab-arbitrage --features timescaledb; \
    rm -rf mayab-arbitrage/src mayab-cli/src \
           target/release/deps/mayab_arbitrage-* \
           target/release/deps/libmayab_arbitrage-* \
           target/release/mayab-arbitrage; \
    if [ -d target/release/.fingerprint ]; then \
      find target/release/.fingerprint -maxdepth 1 \
        \( -name 'mayab-arbitrage-*' -o -name 'mayab-cli-*' \) \
        -exec rm -rf {} +; \
    fi

# Real source
COPY mayab-arbitrage/src ./mayab-arbitrage/src
COPY mayab-cli/src ./mayab-cli/src
COPY internal/webui ./internal/webui
RUN touch mayab-cli/src/main.rs mayab-arbitrage/src/lib.rs && \
    cargo build --release --locked -p mayab-cli --bin mayab-arbitrage --features timescaledb && \
    objcopy --compress-debug-sections \
      target/release/mayab-arbitrage /mayab-arbitrage

# ── Stage 2: Runtime ──
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="Mayab Arbitraje BTC" \
      org.opencontainers.image.description="Motor de arbitraje BTC estrictamente simulado" \
      org.opencontainers.image.source="https://github.com/raulivan1200/mayab-rust-coding-challenge-mx"

RUN groupadd --system --gid 10001 nonroot \
    && useradd --system --uid 10001 --gid nonroot --no-create-home nonroot

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /mayab-arbitrage /app/mayab-arbitrage
COPY --from=builder /build/internal/webui /app/internal/webui
COPY data/captura_real.json /app/data/captura_real.json

RUN mkdir -p /data \
    && chown nonroot:nonroot /data \
    && chmod 0755 /app /app/mayab-arbitrage /app/internal /app/internal/webui

WORKDIR /app
USER nonroot:nonroot

ENV PORT=8080 \
    RUST_LOG=info \
    STORAGE_MODE=sqlite_ephemeral \
    AUDITORIA_DB_PATH=/data/mayab-auditoria.sqlite

EXPOSE 8080

HEALTHCHECK --interval=10s --timeout=3s --retries=3 --start-period=10s \
  CMD curl -sf http://localhost:8080/healthz || exit 1

STOPSIGNAL SIGTERM
ENTRYPOINT ["/app/mayab-arbitrage"]
