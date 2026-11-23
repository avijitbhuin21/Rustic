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
# COPY keeps the sources' original mtimes, which are older than the placeholder
# build above; newer cargo then treats that placeholder as fresh and ships a
# binary that exits immediately. Bump the mtime so the real sources are compiled.
RUN touch src/main.rs && cargo build --release && test -f target/release/rustic-website

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /build/target/release/rustic-website /usr/local/bin/rustic-website
COPY static /app/static
COPY issue-actions.json /app/issue-actions.json
COPY theme-actions.json /app/theme-actions.json
COPY seed-themes /app/seed-themes
ENV WEBSITE_STATIC_DIR=/app/static \
    ISSUE_ACTIONS_FILE=/app/issue-actions.json \
    THEME_ACTIONS_FILE=/app/theme-actions.json \
    THEME_SEEDS_DIR=/app/seed-themes \
    PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT:-8080}/healthz" || exit 1
ENTRYPOINT ["rustic-website"]
