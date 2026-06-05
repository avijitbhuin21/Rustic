# rustic-server

The **headless web transport** for Rustic. Runs the same Rust backend the
desktop app uses as an HTTP + WebSocket server on a VM, serves the React
frontend over HTTP, and gates everything behind a deploy-time password. **It
does not link Tauri or any webview** — the browser is the client.

The desktop app is unchanged. Both binaries build from the same workspace and
share the transport-agnostic `rustic-app` crate (`AppState`, the filesystem
watcher, path-scope guards, secret storage, and the `bootstrap()` sequence).

## Architecture

```
browser ──HTTP POST /api/<cmd>──▶ axum ──▶ ServerContext ──▶ shared command logic
        ◀──── WS /ws (events) ─── EventHub ◀── ServerContext::emit ◀── (watcher, …)
```

* `AppContext` (in `rustic-app`) is the seam between command logic and the
  transport. The desktop injects a `TauriContext` (emit via `AppHandle`, paths
  via Tauri, secrets via the OS keychain); the server injects a `ServerContext`
  (emit onto a `tokio::broadcast` hub that every `/ws` connection forwards,
  paths from env, secrets from an env-overridable file).
* `invoke(cmd, args)` → `POST /api/cmd` with the args as the JSON body.
* `listen(event, cb)` → one multiplexed `/ws` socket, filtered client-side by
  event name. A single emit fans out to **all** connected tabs.

## Quick start (local)

### Windows — one command

The repo ships a runner that builds the web bundle + server, persists a session
secret, sets the env, and starts the server (binds `127.0.0.1`, a secure context
so clipboard/mic work):

```powershell
.\scripts\run-server.ps1                 # builds everything; password defaults to "rustic"
.\scripts\run-server.ps1 -SkipWebBuild   # reuse an existing dist/ (skips the ~2 min web build)
.\scripts\run-server.ps1 -Password hunter2 -Port 9000 -Release
```

Then open the URL it prints (e.g. `http://127.0.0.1:8787`) and log in. Stop with
Ctrl-C. Flags: `-Password`, `-Port`, `-DataDir`, `-StaticDir`, `-Release`,
`-SkipWebBuild`, `-BindAll`. Full help: `Get-Help .\scripts\run-server.ps1 -Detailed`.

### Manual (any OS)

```bash
# 1. Build the web frontend (note: --mode web swaps the transport shims in)
bun install
bun run build:web              # outputs ./dist

# 2. Build + run the server
cargo build --release -p rustic-server
RUSTIC_AUTH_PASSWORD=hunter2 \
RUSTIC_SESSION_SECRET=$(openssl rand -hex 32) \   # else logins reset on every restart
RUSTIC_STATIC_DIR=dist \
RUSTIC_DATA_DIR=./rustic-data \
  ./target/release/rustic-server
# open http://localhost:8787  (localhost is a secure context, so clipboard/mic work)
```

To use the AI agent chat, set a provider API key in the Settings UI once the app
loads — it's persisted to `<RUSTIC_DATA_DIR>/secrets.json` (or inject it via
`RUSTIC_SECRET_<ACCOUNT>`; see [`.env.example`](../.env.example)).

## Docker

```bash
cp .env.example .env          # set RUSTIC_AUTH_PASSWORD (and RUSTIC_DOMAIN for TLS)
docker compose up --build     # Caddy terminates HTTPS and forwards /ws
```

The image carries `git`, `node`/`npx`, and `uv`/`uvx` so state-mutating VCS ops
and stdio MCP servers work.

## Configuration

See [`.env.example`](../.env.example) for every variable. Required:
`RUSTIC_AUTH_PASSWORD`. Recommended: `RUSTIC_SESSION_SECRET` (stable, so sessions
survive restarts) and a mounted volume at `RUSTIC_DATA_DIR`.

## The two open decisions

### TLS strategy — **required, not optional**

Several frontend features only work in a browser **secure context** (HTTPS, or
`localhost`): clipboard read/write (terminal + code copy), microphone
(`getUserMedia` for audio transcription), and clipboard image paste. Over plain
HTTP on a public IP they silently fail. Choose one:

* **Reverse proxy + HTTPS (default)** — `docker compose up` runs Caddy, which
  auto-provisions a Let's Encrypt cert for `RUSTIC_DOMAIN` and forwards the
  WebSocket `Upgrade`/`Connection` headers for `/ws` natively. See
  [`deploy/Caddyfile`](../deploy/Caddyfile).
* **VPN / Tailscale only** — drop the `caddy` service, publish the server port,
  and reach it via `localhost`/MagicDNS (a trusted secure origin). Plain HTTP is
  then acceptable because the browser treats the origin as secure.

### Server OS

Assumed **Linux** (the image is `debian:bookworm-slim`). The path-scope guards
apply the Unix banned-roots list (`/etc`, `/proc`, `/usr/bin`, …) and the
sensitive-home blocks (`.ssh`, `.aws`, browser profiles) stay active — they
protect the IPC boundary even though the single user otherwise has broad VM
access.

## Security model

* Single user, single global `AppState` (no per-session isolation).
* Every `/api/*` and `/ws` request requires a valid HMAC-signed session token
  (issued by `POST /login` after a constant-time password check). Tokens live in
  an httpOnly cookie and are also returned for `Authorization: Bearer` use.
* `/login` is rate-limited per IP (lock out after N failures).
* Path-scope guards reject reads/writes to system roots and credential dirs.
* **Embedded browser (CDP proxy).** The `browser_*` commands drive a headless
  Chromium on the VM; `/ws/browser/cdp` and `/api/browser/*` reverse-proxy its
  Chrome DevTools Protocol. **CDP is a full RCE surface** — it can read `file://`
  and reach internal endpoints — so those routes sit behind the same session-token
  auth as everything else and must never be exposed unauthenticated. As a second
  layer Chromium binds its debugging port to `127.0.0.1` only (never published).
  This is acceptable under the single-user trust model (the user already has a
  terminal on the VM), but it's exactly why the proxy auth is non-negotiable.
  Chromium is spawned lazily on first window-open and fully terminated (process
  group reaped) on close / idle / shutdown — no idle browser process, ever.

## Command coverage

**The full `invoke_handler!` command surface is wired** (see
`SERVER_IMPLEMENTATION_STATUS.md`). Handlers live in per-module files under
`src/commands/` (`api.rs` chains them); each reuses the same core-crate function
the desktop `#[tauri::command]` body calls, so behavior matches the desktop —
**without touching `src-tauri`** (the desktop build cannot regress).

* **Explorer / editor / search / preview** — workspace, file tree CRUD/copy/move,
  buffer open/edit/highlight/format/save/undo, search + replace, base64/hex preview.
* **git** — full surface incl. stage/commit/push/pull/rebase/conflicts/clone/log
  and the `github_*` device-flow.
* **terminal** — create/write/resize/close/list/read + live `terminal-output`
  streaming (the PTY reader + monitor threads emit over the WS hub).
* **file_history**, **formatters** (incl. download/install), **settings**,
  **skills / workflows / rules** (incl. repo install), **agent config**
  (providers, models, MCP, memory, budget, audio, freebuff).
* **agent chat** — `create_task` + the full streaming `send_message` executor
  (every `agent-*` event forwarded over the hub), list/get/abort/respond,
  model switch, and agent-owned background terminals.

Intentionally **501** (handled by the frontend web shims instead, never a silent
success): OS-clipboard commands (`read_clipboard_files`, `write_clipboard_files`,
`paste_clipboard_image_into`) and `reveal_in_file_manager`. The web build hides
these menu items and instead offers browser-native **Download** (file = raw,
folder = generated zip via `GET /api/download?path=`) and **Upload** (files or a
whole folder via the `upload_file` command, plus OS drag-and-drop onto a folder).
In-app copy / cut / paste work over the already-wired `copy_entry` / `move_entry`.

Still pending: a streaming `/api/asset?path=` route for very large binary
previews (`read_file_base64` covers the common path), and SSE MCP transport
(unimplemented on desktop too).
