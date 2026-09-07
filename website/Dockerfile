# ── rustic-website image ─────────────────────────────────────────────────────
# Standalone axum static server. Build context is the `website/` folder itself
# (Railway: set the service root directory to `website`).

FROM rust:1-bookworm AS build
WORKDIR /build
ENV CARGO_NET_RETRY=10 \
    CARGO_HTTP_MULTIPLEXING=false
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/rustic-website* target/release/deps/rustic_website*
COPY src ./src
RUN cargo build --release && test -f target/release/rustic-website

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /build/target/release/rustic-website /usr/local/bin/rustic-website
COPY static /app/static
ENV WEBSITE_STATIC_DIR=/app/static \
    PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT:-8080}/healthz" || exit 1
ENTRYPOINT ["rustic-website"]
