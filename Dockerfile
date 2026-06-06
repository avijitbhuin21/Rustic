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
# Cloud builders intermittently drop crates.io downloads mid-stream ("unexpected
# eof"). Retry aggressively and disable HTTP/2 multiplexing, which is the usual
# trigger for those resets.
ENV CARGO_NET_RETRY=10 \
    CARGO_HTTP_MULTIPLEXING=false \
    CARGO_NET_GIT_FETCH_WITH_CLI=true
# Build ONLY the server crate — cargo will compile its dependency graph and skip
# the `src-tauri` member entirely, so no webkit/webview toolchain is required.
# Wrapped in a retry loop: cloud builders occasionally time out mid-download of a
# crate `.crate` file ([28] curl timeout); each retry resumes against the cargo
# cache populated by previous attempts, so a transient stall self-heals.
RUN for i in 1 2 3 4 5; do \
      echo "=== cargo build attempt $i ==="; \
      cargo build --release -p rustic-server && break; \
      echo "attempt $i failed (likely a transient crates.io download); retrying in 20s"; \
      sleep 20; \
    done; \
    test -f target/release/rustic-server

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
        git ca-certificates curl wget xz-utils unzip gnupg \
        build-essential pkg-config \
        python3 python3-venv python3-pip python3-dev python-is-python3 pipx \
        chromium fonts-liberation fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*
# Node.js — latest *current* release, fetched dynamically from nodejs.org so the
# version is never hardcoded. The official tarball bundles npm + corepack and
# installs into /usr/local. (debian's apt ships an EOL Node 18.)
RUN set -eux; \
    NODE_TARBALL="$(curl -fsSL https://nodejs.org/dist/latest/SHASUMS256.txt \
      | grep -oE 'node-v[0-9.]+-linux-x64\.tar\.xz' | head -n1)"; \
    curl -fsSL "https://nodejs.org/dist/latest/${NODE_TARBALL}" -o /tmp/node.tar.xz; \
    tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
        --exclude='*/CHANGELOG.md' --exclude='*/LICENSE' --exclude='*/README.md'; \
    rm /tmp/node.tar.xz; \
    node --version; npm --version
# uvx (for Python-based MCP servers) — best-effort; ignore failure on networks
# without PyPI access.
RUN pip3 install --break-system-packages uv 2>/dev/null || true

# cloudflared — powers the Cloudflare quick-tunnel preview mode (open a VM dev
# server on a public https URL with no Cloudflare account or domain). Best-effort:
# ignore failure on build networks without GitHub access (the feature just stays
# unavailable; path/subdomain modes are unaffected).
RUN set -eux; \
    curl -fsSL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 \
      -o /usr/local/bin/cloudflared \
      && chmod +x /usr/local/bin/cloudflared \
      && cloudflared --version || true

# ── language toolchains (Go / Rust / Bun / TypeScript) ───────────────────────
# Baked into the image so every deploy has them on the global PATH. User data
# (and any user-installed CLIs via `go install` / `cargo install`) lives on the
# /data volume so it persists across redeploys; the toolchains themselves are
# part of the image and reinstall on each build.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    GOROOT=/usr/local/go \
    PATH=/usr/local/go/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin

# Go — fetch the current stable release dynamically so the URL never goes stale.
RUN set -eux; \
    GO_VERSION="$(curl -fsSL 'https://go.dev/VERSION?m=text' | head -n1)"; \
    curl -fsSL "https://go.dev/dl/${GO_VERSION}.linux-amd64.tar.gz" -o /tmp/go.tgz; \
    tar -C /usr/local -xzf /tmp/go.tgz; \
    rm /tmp/go.tgz; \
    go version

# air — live-reload for Go. Installed to /usr/local/bin so it's on the global
# PATH (GOBIN override keeps it out of the volume-backed GOPATH).
RUN set -eux; \
    GOBIN=/usr/local/bin go install github.com/air-verse/air@latest; \
    air -v

# Rust — stable toolchain via rustup, with clippy + rustfmt.
RUN set -eux; \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable; \
    rustup component add clippy rustfmt; \
    rustc --version; cargo --version

# Bun — copy the binary straight out of the frontend build stage (no download).
COPY --from=web /usr/local/bin/bun /usr/local/bin/bun

# Global TypeScript toolchain (handles .ts/.tsx/.jsx via tsc/ts-node).
RUN npm install -g typescript ts-node 2>/dev/null || true

WORKDIR /app
COPY --from=server /build/target/release/rustic-server /usr/local/bin/rustic-server
COPY --from=web   /app/dist /app/dist

# CHROME_BIN lets the BrowserManager find Chromium without a PATH scan. The
# browser profile lives under the mounted data volume (/data/browser-profile)
# so cookies/logins survive deploys — important for in-VM OAuth flows.
# GOPATH/CARGO_INSTALL_ROOT point at the /data volume so binaries installed at
# runtime (`go install ...`, `cargo install ...`) persist across deploys. Their
# bin dirs are prepended to PATH. The toolchains themselves stay in the image.
ENV RUSTIC_DATA_DIR=/data \
    RUSTIC_STATIC_DIR=/app/dist \
    CHROME_BIN=/usr/bin/chromium \
    GOPATH=/data/go \
    CARGO_INSTALL_ROOT=/data/cargo \
    BUN_INSTALL=/data/bun \
    PATH=/data/go/bin:/data/cargo/bin:/data/bun/bin:/usr/local/go/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
EXPOSE 8787

# Healthcheck hits the unauthenticated /healthz endpoint. Honors $PORT (set by
# Railway and other PaaS) and falls back to the default 8787 used by compose.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT:-8787}/healthz" || exit 1

ENTRYPOINT ["rustic-server"]
