# Server Implementation — Status Report

Status of implementing `SERVER_IMPLEMENTATION_PLAN.md` (run Rustic as a headless
web server alongside the unchanged desktop app).

**Branch:** `rebuilt-ui`. **Desktop build:** green throughout (regression guard
held). **New tests:** 13 passing (4 `rustic-app`, 4 + 5 `rustic-server`).

---

## What's done and verified

### Phase 0–1 — `AppContext` abstraction + shared crate ✅
New Tauri-free crate **`crates/rustic-app`** holds the shared application layer:

- `AppContext` / `EventEmitter` traits — the seam between command logic and
  transport (emit, `data_dir`, `home_dir`, `state`, `secrets`). Object-safe;
  generic `emit<T: Serialize>` via a blanket extension trait.
- `AppState` (+ `AgentState`, `FileHistoryHandle`, …) — **moved** here; `src-tauri`
  re-exports them so all ~150 existing `crate::state::AppState` references keep
  resolving unchanged.
- `path_scope` (system-root / sensitive-home guards) — moved + re-exported.
- `sync_ext` (poison-resilient mutex) — moved + re-exported.
- `watcher` (`FileWatcherManager`) — moved; its emit is now abstracted behind
  `EventEmitter` instead of `AppHandle`.
- `SecretStore` trait + `FileSecretStore` (JSON file, `0600` on Unix) +
  `EnvSecretStore` (reads `RUSTIC_SECRET_<ACCOUNT>`, falls through to file).
- `ServerConfig::from_env` and a transport-agnostic **`bootstrap()`** (open DB,
  hydrate AI/tool config + secrets, restore git token, seed workflows, restore
  projects + start watchers).

`src-tauri` change footprint: 4 modules became one-line re-export shims, a new
`transport.rs` (`TauriEmitter` + `KeychainSecretStore`), and 2 watcher call
sites now pass a `TauriEmitter`. **No command signatures changed.**

### Phase 2 — Server transport (`rustic-server`) ✅
New binary+lib crate (axum + tokio), holds `Arc<AppState>`:

- `ServerContext: AppContext` — emits onto a `tokio::broadcast` **EventHub**.
- `/ws` — one multiplexed socket per tab; a single emit fans out to all tabs
  (multi-tab requirement); lag-tolerant; client reconnect handled frontend-side.
- `POST /api/<command>` dispatch with the JSON body as args
  (`#[serde rename_all=camelCase]` to match the frontend wire format).
- Shared `bootstrap()`; env/file secret backend; data dir + static dir from env;
  stdout logging for container capture.
- Path-scope guards active (verified: refuses `C:\Windows\...`).
- **Live command surface:** workspace (list/add/remove), file tree
  (read_dir, read_file_content, create_file, create_folder, rename_entry,
  delete_entry, stat_path), preview (read_file_base64, get_file_size), git
  (is_repo, status, branches, diff, diff_staged), get_ai_config. Unwired
  commands return **501 with the command name** (no silent fallback).

### Phase 3 — Auth & security gate ✅
- `POST /login` — constant-time password compare; issues an HMAC-SHA256 session
  token (httpOnly cookie + bearer); `POST /logout`.
- Middleware rejects every `/api/*` and `/ws` without a valid token (401). Token
  read from bearer header, cookie, or `?token=` (browser WebSocket).
- Per-IP login rate-limiter with lockout. `/healthz` + graceful shutdown.

### Phase 4 — Frontend transport shim ✅
- `src/lib/web/transport-core.js` — `invoke()` over `fetch`, `listen()`/`once()`
  over one auto-reconnecting `/ws`, and a built-in **login overlay** shown on 401.
- Vite swaps `@tauri-apps/api/{core,event,window,app}` and
  `@tauri-apps/plugin-{shell,dialog,fs}` to web shims when built with
  `--mode web`. **Zero component imports changed.** Desktop build path untouched.
- `bun run build:web` succeeds → browser bundle in `dist/`.

### Phase 5 — Browser equivalents ✅
Web shims for shell (`open` → new tab), dialog (server-path prompt /
confirm / message), fs (routed through server commands), window (no-op stubs),
app (`getVersion`). The web build bundles with no missing Tauri APIs.

### Phase 6 — Deploy packaging ✅
`Dockerfile` (3-stage: bun web build → cargo server build → slim runtime with
git + node/npx + uv/uvx), `.dockerignore`, `docker-compose.yml` (server + Caddy
auto-HTTPS), `deploy/Caddyfile` (forwards WS upgrade + real client IP),
`.env.example`, and `rustic-server/README.md` documenting both TLS topologies
and the Linux OS decision.

### Phase 7 — Tests ✅ (for the implemented scope)
`rustic-app`: secret store roundtrip + env precedence, mutex poison recovery.
`rustic-server`: token issue/verify/expiry, password compare, rate-limiter, and
5 router integration tests (health open, api requires auth, wrong password 401,
login→authed call, 501 for unwired command). Manual end-to-end smoke against a
running server confirmed: login, list/add_project (DB-backed), git_status,
read_dir/read_file_content, path-scope refusal, and a live `rustic:fs-change`
event delivered over `/ws` when a watched file changed.

---

## Full command surface wired ✅ (2026-06-02)

The whole `invoke_handler!` command set is now served. Rather than the
`Arc<AppState>` desktop refactor, the server **reimplements each command's thin
plumbing by calling the same core crates** (the established `api.rs` pattern) —
so **`src-tauri` is byte-for-byte untouched and the desktop build cannot
regress** (verified: `cargo check -p rustic` green). Dispatch is split into
per-module files under `rustic-server/src/commands/` (`api.rs` chains them):

- **meta / workspace / file_tree / editor / search / preview** — explorer, all
  buffer/edit/highlight/save ops, search + replace, file CRUD/copy/move,
  base64/hex preview, log files.
- **git** — full surface incl. stage/commit/push/pull/fetch/branch/rebase/
  conflicts/clone/log + the `github_*` device-flow set (reqwest).
- **terminal** — create/write/resize/close/list/read/detect_shells, with the
  **PTY reader + session-monitor threads emitting `terminal-output` /
  `terminal-list-changed` over the WS hub** (ported 1:1; emit via
  `ServerContext`, state via the shared `Arc<AppState>`).
- **file_history** — all `fh_*` (server-side `get_or_create_handle` builds the
  gix shadow + sweep worker, emitting via the hub).
- **formatters** — registry/list/format/custom + install/update/check (reqwest +
  zip/tar/flate2 extraction).
- **settings / skills / workflows / rules** — full surface incl. repo install
  (reqwest + zip), VS Code keybinding import, themes.
- **agent_config** — provider config (keys via `ctx.secrets()` under the same
  accounts), model fetch (OpenRouter/known), MCP management, memory, budget,
  permissions, freebuff tokens, audio config + `transcribe_audio`.
- **agent_chat** — `create_task` + the full streaming `send_message` executor
  (every `agent-*` event forwarded over the hub), list/get/todos/subagents,
  delete/rename/truncate, abort, permission/ask-user/ceiling responses,
  model switch, cost, input-queue notifications. **Agent-owned background
  terminals** are bridged via a server `AgentTerminals` broker.

`rustic-server` deps added: reqwest, form_urlencoded, zip, tar, flate2, ignore,
uuid. All 13 tests green; `bun run build:web` green; `cargo build -p
rustic-server` green.

### Intentionally not served (no silent fallback — these 501)
Desktop-native commands that cannot run headless and are handled by the
**frontend web shims** instead: `read_clipboard_files`, `write_clipboard_files`,
`paste_clipboard_image_into` (OS clipboard — browser Clipboard API),
`reveal_in_file_manager` (OS file manager). `save_pasted_image_base64` and
`list_project_files` ARE wired.

### Soft degradations (documented in code)
- OpenRouter per-model cost/context falls back to the static registry until
  `fetch_openrouter_model_specs` populates the cache (matches desktop cold-start).

## What remains

1. **Large-file / asset routes.** `convertFileSrc` → `/api/asset?path=` GET route
   (with size limits) for very large binary previews is still pending;
   `read_file_base64` covers the common path.
2. **SSE MCP transport** remains unimplemented (as on desktop). stdio MCP works
   where node/npx/uvx are present.
3. **Phase 8 browser E2E.** Per the plan + global rule, browser-driving
   automation needs explicit user sign-off; a guided manual checklist is in the
   plan §7. **← you are here: ready for manual browser testing.**

---

## How to run what exists

```bash
bun run build:web
cargo build --release -p rustic-server
RUSTIC_AUTH_PASSWORD=hunter2 RUSTIC_STATIC_DIR=dist RUSTIC_DATA_DIR=./rustic-data \
  ./target/release/rustic-server
# http://localhost:8787  — log in, explorer + git + file preview work live.
```
