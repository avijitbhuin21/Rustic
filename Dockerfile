# ── rustic-server image ──────────────────────────────────────────────────────
# Multi-stage: build the web frontend (bun), build the headless server (cargo),
# then assemble a slim runtime that also carries node/npx + uvx so stdio MCP
# servers can spawn. The server binary does NOT link Tauri or any webview.

# ---- stage 1: web frontend (VITE_TARGET=web via --mode web) -----------------
FROM oven/bun:1 AS web
WORKDIR /app
COPY package.json bun.lockb* bun.lock* ./
RUN bun install --frozen-lockfile || bun install
COPY . .
RUN bun run build:web   # outputs ./dist

# ---- stage 2: rust server ---------------------------------------------------
FROM rust:1-bookworm AS server
WORKDIR /build
# System deps for the pure-Rust stack are minimal; git is needed at RUNTIME for
# state-mutating VCS ops, not for the build.
COPY . .
# Build ONLY the server crate — cargo will compile its dependency graph and skip
# the `src-tauri` member entirely, so no webkit/webview toolchain is required.
RUN cargo build --release -p rustic-server

# ---- stage 3: runtime -------------------------------------------------------
FROM debian:bookworm-slim AS runtime
# git: required for state-mutating VCS operations (commit/push/pull) per the
#      pure-Rust workspace's runtime contract.
# nodejs/npm: required so stdio MCP servers spawned via `npx` work.
# ca-certificates: for outbound HTTPS to AI providers.
# chromium + fonts: the embedded VM browser feature spawns a headless Chromium
#      on demand (loopback CDP, never published). ~400 MB; the fonts give it
#      sane Latin + emoji glyphs so rendered pages/screenshots aren't tofu.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        git ca-certificates curl nodejs npm python3 python3-venv \
        chromium fonts-liberation fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*
# uvx (for Python-based MCP servers) — best-effort; ignore failure on networks
# without PyPI access.
RUN pip3 install --break-system-packages uv 2>/dev/null || true

WORKDIR /app
COPY --from=server /build/target/release/rustic-server /usr/local/bin/rustic-server
COPY --from=web   /app/dist /app/dist

# CHROME_BIN lets the BrowserManager find Chromium without a PATH scan. The
# browser profile lives under the mounted data volume (/data/browser-profile)
# so cookies/logins survive deploys — important for in-VM OAuth flows.
ENV RUSTIC_DATA_DIR=/data \
    RUSTIC_STATIC_DIR=/app/dist \
    CHROME_BIN=/usr/bin/chromium
EXPOSE 8787

# Healthcheck hits the unauthenticated /healthz endpoint. Honors $PORT (set by
# Railway and other PaaS) and falls back to the default 8787 used by compose.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT:-8787}/healthz" || exit 1

ENTRYPOINT ["rustic-server"]
