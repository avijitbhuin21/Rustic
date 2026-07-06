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
# Pinned toolchain versions + checksums (previously fetched "latest"
# dynamically, which made builds unreproducible and unverifiable). Bump these
# ARGs deliberately; the SHA256 values come from the vendors' published
# checksum manifests (nodejs.org SHASUMS256.txt, go.dev/dl JSON, GitHub
# release asset digests).
ARG NODE_VERSION=24.18.0
ARG NODE_SHA256=55aa7153f9d88f28d765fcdad5ae6945b5c0f98a36881703817e4c450fa76742
ARG GO_VERSION=1.26.4
ARG GO_SHA256=1153d3d50e0ac764b447adfe05c2bcf08e889d42a02e0fe0259bd47f6733ad7f
ARG CLOUDFLARED_VERSION=2026.6.1
ARG CLOUDFLARED_SHA256=5861a10a438fe8ddcfebb3b830f83966cbf193edafce0fe2eeb198fbae1f7a22
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
# Node.js — pinned LTS release, checksum-verified against nodejs.org's
# published SHASUMS256.txt. The official tarball bundles npm + corepack and
# installs into /usr/local. (debian's apt ships an EOL Node 18.)
RUN set -eux; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
      -o /tmp/node.tar.xz; \
    echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c -; \
    tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
        --exclude='*/CHANGELOG.md' --exclude='*/LICENSE' --exclude='*/README.md'; \
    rm /tmp/node.tar.xz; \
    node --version; npm --version
# uvx (for Python-based MCP servers) — best-effort; ignore failure on networks
# without PyPI access.
RUN pip3 install --break-system-packages uv 2>/dev/null || true

# cloudflared — powers the Cloudflare quick-tunnel preview mode (open a VM dev
# server on a public https URL with no Cloudflare account or domain). Installed
# at build time so the feature is READY on first boot; only the per-port tunnel
# *process* is spawned on demand at runtime. The download is retried because the
# GitHub release CDN occasionally blips, and the final `cloudflared --version`
# is NOT swallowed — a build that can't install it FAILS here rather than
# silently shipping an image where the feature is dead with no error.
RUN set -eux; \
    for i in 1 2 3 4 5; do \
      curl -fsSL --retry 3 --retry-delay 2 \
        "https://github.com/cloudflare/cloudflared/releases/download/${CLOUDFLARED_VERSION}/cloudflared-linux-amd64" \
        -o /usr/local/bin/cloudflared && break; \
      echo "cloudflared download attempt $i failed; retrying…"; sleep 5; \
    done; \
    echo "${CLOUDFLARED_SHA256}  /usr/local/bin/cloudflared" | sha256sum -c -; \
    chmod +x /usr/local/bin/cloudflared; \
    cloudflared --version

# ── language toolchains (Go / Rust / Bun / TypeScript) ───────────────────────
# Baked into the image so every deploy has them on the global PATH. User data
# (and any user-installed CLIs via `go install` / `cargo install`) lives on the
# /data volume so it persists across redeploys; the toolchains themselves are
# part of the image and reinstall on each build.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    GOROOT=/usr/local/go \
    PATH=/usr/local/go/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin

# Go — pinned stable release, checksum-verified against go.dev's published
# per-file SHA256 (https://go.dev/dl/?mode=json).
RUN set -eux; \
    curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" -o /tmp/go.tgz; \
    echo "${GO_SHA256}  /tmp/go.tgz" | sha256sum -c -; \
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
