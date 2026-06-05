# Server Implementation Plan

Turn Rustic from a Tauri desktop-only app into something that **also** runs as a
single-user web server on a VM, accessed through a browser over a public port,
gated by a deploy-time password. The desktop app stays exactly as it is.

---

## 1. Goal & constraints

**Goal:** Run the existing Rust backend as a headless HTTP + WebSocket server on a
VM. The same React frontend is served over HTTP and runs in the user's browser.
The user operates the VM as their dev machine through this UI.

**Locked decisions (from discussion):**
- **Single user.** No multi-tenant / per-session isolation. Keep the current
  global `AppState` singleton.
- **Password guardrail.** A secret is set at deploy time via `.env`
  (e.g. `RUSTIC_AUTH_PASSWORD`). Every HTTP route and the WebSocket upgrade are
  gated behind a session token issued after login.
- **Desktop app untouched.** The server is an *additional* binary, not a
  modification of `src-tauri`. Both build from the same workspace + frontend.
- **No webview on the server.** The server binary does **not** link Tauri or any
  webview engine — that is the RAM saving. The browser is the client.
- **Same frontend.** The existing React/Vite UI is reused; only the transport
  layer (`invoke` → HTTP, `listen` → WebSocket) is swapped behind a shim.

**Open decisions (do not block Phase 0–2; resolve before deploy):**
- **TLS strategy:** reverse proxy (Caddy/nginx with HTTPS) **vs** VPN/Tailscale-only
  with plain HTTP. Affects deploy steps only.
- **Server OS:** assumed Linux. Affects secret storage + default shell.

---

## 2. Current architecture (what we have)

Three layers, already cleanly separated:

1. **Core crates (transport-agnostic, fully reusable):**
   `rustic-core`, `rustic-db`, `rustic-git`, `rustic-terminal`,
   `rustic-treesitter`, `rustic-agent`. ~90% of the product. No Tauri dependency.
2. **Tauri shell (`src-tauri/`) — thin transport layer:**
   - ~150 commands registered in `src-tauri/src/lib.rs` `invoke_handler![...]`.
   - ~50 server→client event types pushed via `app.emit(...)`
     (e.g. `terminal-output` in `commands/terminal.rs`, the `agent-*` stream in
     `commands/agent/mod.rs`).
   - Global state in `src-tauri/src/state.rs` (`AppState`) — all `Arc`/`Mutex`,
     already shareable across threads/handlers.
   - Secrets via OS keychain (`src-tauri/src/secrets.rs`).
   - App-data paths via Tauri (`src-tauri/src/app_paths.rs`).
   - Tauri plugins: `dialog`, `fs`, `shell`, `single-instance` (release).
3. **Frontend (React 19 + Vite + Zustand):**
   Talks to backend ONLY via `invoke('cmd', args)` (request/response) and
   `listen('event', cb)` (streaming). Calls are centralized in `src/state/*.js`
   and a few `src/lib/*.js` (`active-editor.js`, `clipboard-image.js`,
   `crash-logger.js`, `use-ui-zoom.js`).

**The core insight:** `invoke`/`emit` map 1:1 onto `HTTP POST` / `WebSocket push`.
This is a transport swap, not a rewrite.

---

## 3. Target architecture (what we build)

```
                       ┌─────────────────────────────────────┐
   Desktop (unchanged) │  src-tauri  →  webview window         │
                       └───────────────┬─────────────────────┘
                                       │  both depend on
                            ┌──────────▼───────────┐
                            │  shared command core  │  ← AppContext abstraction
                            │  + workspace crates    │     (decoupled from AppHandle)
                            └──────────▲───────────┘
                       ┌───────────────┴─────────────────────┐
   Server (new)        │  rustic-server  →  Axum HTTP + WS     │
                       │  serves static React build + API      │
                       └───────────────┬─────────────────────┘
                                       │  HTTPS (proxy) / VPN
                                  ┌────▼────┐
                                  │ Browser │  (the user)
                                  └─────────┘
```

- New binary crate `rustic-server` (Axum + tokio) holds `AppState`, exposes one
  route per command and a `/ws` endpoint that streams events.
- A small **`AppContext`** abstraction replaces direct `tauri::AppHandle` usage so
  both `src-tauri` and `rustic-server` share the same command bodies.
- Frontend gets a **transport shim**: `invoke()`/`listen()` keep their signatures
  but route over `fetch` + `WebSocket` when running in the browser build.

---

## 4. The central refactor: decouple commands from `AppHandle`

This is the largest and highest-risk piece; everything else depends on it.

Today commands are `#[tauri::command]` fns taking `State<'_, AppState>` and
`tauri::AppHandle`. `AppHandle` is used for four things:
1. **Emitting events** — `app.emit("event", payload)`.
2. **Resolving paths** — `app_data_dir`, `home_dir` (`app_paths.rs`).
3. **Accessing state** — `app.state::<AppState>()` (in spawned threads).
4. **Window control** — close/focus (desktop-only; server no-ops these).

**Plan:** introduce a context trait (working name `AppContext`) that provides:
- `emit(&self, event: &str, payload: impl Serialize)`
- `data_dir(&self) -> PathBuf`
- `home_dir(&self) -> PathBuf`
- access to `Arc<AppState>`

Two implementations:
- `TauriContext` — wraps `AppHandle`, emits via Tauri, paths via `app_paths`.
- `ServerContext` — emits onto a tokio broadcast/WS hub, paths from env/config.

Command bodies move into a shared module (or stay in `src-tauri` but become
generic over `AppContext`). The `#[tauri::command]` wrappers become thin adapters
that build a `TauriContext` and call the shared body. The server builds routes
that construct a `ServerContext` and call the same body.

---

## 5. Phases

Each phase has a concrete deliverable and a verification gate. **Do not start a
phase until the previous phase's desktop build still passes** (regression guard).

### Phase 0 — Spike & scaffolding (de-risk)
- Add `rustic-server` crate to the workspace (`Cargo.toml` members). Axum + tokio.
- Prove ONE read command (e.g. `list_projects`) and ONE event stream
  (e.g. `terminal-output`) end-to-end over HTTP + WS, with a hardcoded
  `AppContext`. Throwaway-quality is fine.
- **Deliverable:** browser can hit `GET /api/list_projects` and see one live WS event.
- **Verify:** `curl` the route; a tiny HTML page receives a WS message.
- **Gate:** confirms the AppContext shape works before mass refactor.

### Phase 1 — `AppContext` abstraction
- Define the `AppContext` trait + `TauriContext` impl.
- Refactor `app_paths.rs` and event emission to go through it.
- Migrate commands in **dependency order**, one module at a time:
  `app` → `workspace` → `file_tree` → `editor` → `search` → `git` →
  `terminal`/`agent_terminals` → `file_history` → `formatters` → `preview` →
  `settings`/`skills`/`workflows`/`rules` → `agent` (largest, last).
- After each module: desktop app must still build and run unchanged.
- **Deliverable:** every command body is generic over `AppContext`; desktop uses
  `TauriContext`.
- **Verify:** `cargo build` + manual desktop smoke test of each migrated area.
- **Gate:** desktop app fully functional with zero behavior change.

### Phase 2 — Server transport (`rustic-server`)
- **HTTP routes:** auto/explicit mapping of each command to `POST /api/<command>`
  with JSON body = the command's args. Build `ServerContext` per request,
  call shared body, return JSON.
- **WebSocket hub:** a tokio `broadcast` channel; `ServerContext::emit` publishes,
  `/ws` subscribes and forwards as JSON `{event, payload}`. Frontend `listen()`
  filters by event name.
- **Startup parity:** replicate `src-tauri/src/lib.rs` `setup()` — DB open,
  config/secret hydration, workspace + watcher restore, workflow seeding,
  file-history reconcile. Extract this into a shared `bootstrap()` both binaries call.
- **Secret backend:** `secrets.rs` keychain won't work headless on Linux. Add an
  env/encrypted-file secret provider selected at runtime (keychain for desktop,
  file for server).
- **Path/config:** server reads data dir from env (default e.g.
  `~/.local/share/rustic`); no Tauri path API.
- **Logging:** `src-tauri/src/logging.rs` writes under the Tauri app-data dir; the
  server bootstrap must point logging at `RUSTIC_DATA_DIR` (and/or stdout for
  container log capture).
- **Path-scope security (`src-tauri/src/path_scope.rs`):** port the read/write
  guards that block system roots and sensitive home dirs (`.ssh`, `.aws`, browser
  profiles, keychains). On a remote box these stay active — they protect the
  boundary even though the user otherwise has broad VM access. Note the Unix
  banned-roots list applies (the server is Linux).
- **MCP runtime (`crates/rustic-agent/src/mcp/`):** MCP servers spawn subprocesses
  over **stdio** (`Command::new`, e.g. `npx`/`uvx`). The container/VM must have
  node/npx/uvx (and any server's own deps) installed. **SSE transport is not
  implemented yet** — `Sse` returns an error. Carry over the project-scope
  `.mcp.json` content-hash consent flow (`mcp_consent.json`).
- **Deliverable:** server answers all command routes + streams all events.
- **Verify:** integration tests in Phase 7; for now `curl` + WS smoke per module.

### Phase 3 — Auth & security gate
- `.env` config: `RUSTIC_AUTH_PASSWORD`, `RUSTIC_BIND_ADDR`, `RUSTIC_DATA_DIR`,
  session secret.
- `POST /login` checks password (constant-time compare), issues a signed session
  token (httpOnly cookie or bearer). Add a middleware that rejects every
  `/api/*` and `/ws` request without a valid token (401).
- Rate-limit `/login`; lock out after N failures.
- Frontend: a login screen shown when the API returns 401; store/refresh token.
- **Deliverable:** nothing is reachable without the password.
- **Verify:** unauth requests 401; valid login unlocks; brute-force is throttled.

### Phase 4 — Frontend transport shim
- Create `src/lib/transport.js` exposing `invoke()` and `listen()` with the same
  signatures the app already uses.
- Build-time switch (Vite env flag, e.g. `VITE_TARGET=web|tauri`):
  - `tauri` → re-export `@tauri-apps/api` (current behavior, desktop unchanged).
  - `web` → `invoke` = `fetch('/api/'+cmd)`; `listen` = subscribe to the shared
    `/ws` connection (single multiplexed socket, filtered by event name).
- Repoint the centralized import sites (`src/state/*.js`, `src/lib/active-editor.js`,
  `clipboard-image.js`, `crash-logger.js`, `use-ui-zoom.js`) at the shim.
- **Deliverable:** `bun run build` with `VITE_TARGET=web` produces a browser bundle
  the server serves; desktop build path unchanged.
- **Verify:** load the served app in a browser; commands + live events work.

### Phase 5 — Browser-equivalents for native features
Handle the things Tauri plugins did natively:
- **File dialogs** (`plugin-dialog`): replace native open/save pickers with in-app
  modals (the file tree already exists) or browser upload/download.
- **Shell `open` URL** (`plugin-shell`): becomes a normal `target=_blank` link.
- **Clipboard image paste** (`clipboard-image.js`): browser Clipboard API path.
- **Zoom** (`use-ui-zoom.js`): CSS zoom instead of the Tauri webview API.
- **Single-instance / window close**: no-op on server.
- **Static asset / large-file routes**: `read_file_base64`, image paste save, etc.
  need HTTP endpoints (and size limits).
- **Deliverable:** no console errors from missing Tauri APIs in the web build.
- **Verify:** exercise each feature in the browser (covered again in Phase 8).

### Phase 6 — Deploy packaging
- Dockerfile (or systemd unit) that builds the server + web frontend and runs
  headless. Mount a data volume for `rustic.db` + file-history.
- `.env.example` documenting every variable.
- TLS branch (decide open question): Caddy/nginx reverse-proxy config **or**
  Tailscale doc. Document chosen path.
- **TLS is functionally required, not just for password safety.** Several
  frontend features only work in a browser *secure context* (HTTPS, or
  `localhost`): clipboard read/write (`navigator.clipboard` — terminal copy/paste,
  code copy), microphone (`getUserMedia` for audio transcription), and clipboard
  image paste. Over plain HTTP on a public IP these silently fail. So: either
  HTTPS via the reverse proxy, OR a VPN/Tailscale setup where the browser reaches
  it via `localhost`/a trusted secure origin. Reverse proxy must also forward the
  WebSocket `Upgrade`/`Connection` headers for `/ws`.
- Health check endpoint; graceful shutdown.
- **Deliverable:** `docker compose up` (or equivalent) yields a reachable,
  password-gated instance.
- **Verify:** fresh VM smoke test (Phase 8 runs against this).

---

## 6. Testing & verification (Phase 7) — backend / integration

- **Unit tests:** secret-provider selection, auth token issue/verify, AppContext
  emit routing, route↔command arg mapping.
- **Command parity tests:** for a representative subset across every module, call
  the shared command body through `ServerContext` and assert the same result the
  Tauri path produces. Prioritize: file ops, editor edit/save, git status/commit,
  terminal create/write/read, agent create_task/send_message, file-history revert.
- **WebSocket/event tests:** assert each emitted event type is delivered with the
  correct payload shape over `/ws`.
- **Auth tests:** 401 without token; success with token; throttle on repeated
  failures; token expiry/refresh.
- **Concurrency:** since single-user but multi-tab is possible, verify two browser
  tabs sharing one `AppState` don't corrupt state (terminal/agent streams fan out
  to both WS connections).
- **Regression:** desktop app full smoke test — confirm zero behavior change.
- **Gate:** all above green before UI testing.

## 7. UI / end-to-end testing (Phase 8) — browser

Run against the deployed server build in a real browser. **Per global rule #4, ask
the user before any browser-driving automation.** Options: Playwright E2E (needs
a login fixture) or guided manual checklist.

Checklist (each must work over HTTP/WS exactly like desktop):
- **Auth:** login screen, wrong password rejected, session persists across reload,
  logout.
- **Projects/explorer:** add/remove project, expand tree, create/rename/delete/
  move/copy files & folders.
- **Editor:** open file, edit, syntax highlight, format, save, undo/redo,
  external-change reload, large-file open.
- **Terminal:** create session, type commands, see streamed output, resize, close;
  multiple sessions; agent-owned terminal lifecycle.
- **Git:** status, stage/unstage, commit, diff, branches, push/pull (with token),
  conflict resolve, log.
- **Agent:** create task, send message, **streaming tokens render live**, tool-use
  events, permission prompts, ask-user dialog, cost updates, abort, sub-agents,
  todo updates, context-condense events, memory.
- **MCP:** add a stdio MCP server (e.g. via `.mcp.json`), project-consent prompt,
  server connects and tools are listed/callable; confirm node/npx present on the box.
- **Secure-context features (need HTTPS/localhost):** terminal copy/paste, code
  copy, clipboard image paste, microphone audio transcription — verify they work
  over the deployed TLS endpoint (and fail-gracefully, not crash, without it).
- **File history:** list changes, diff, revert file / from-message / task.
- **Settings:** themes, keybindings, AI provider config (keys persisted to the
  file secret backend), model switch.
- **Native-replacement features (Phase 5):** file picker modal, open-URL link,
  clipboard image paste, zoom.
- **Multi-tab:** open two tabs, confirm both receive live events.
- **Network resilience:** WS reconnect after a dropped connection; in-flight
  agent stream recovers or resyncs.
- **Gate:** full checklist passes on a fresh VM deployment over the public port.

---

## 8. Risk register

| Risk | Mitigation |
|---|---|
| `AppHandle` decoupling touches ~150 commands | Phase incrementally per module; desktop must build after each |
| Spawned threads grab state via `app.state()` | Pass `Arc<AppState>` into `AppContext` explicitly |
| Keychain absent on headless Linux | File/env secret backend selected at runtime |
| Event payloads drift between desktop & web | Shared serde types; parity tests in Phase 7 |
| Large-file / binary endpoints over HTTP | Dedicated streaming routes + size limits |
| Password on public port without TLS | Mandate reverse-proxy HTTPS or VPN-only before exposing |
| WS disconnects lose agent stream | Reconnect + server-side replay/resync of recent events |
| Multi-tab on single `AppState` | Fan-out events to all WS subs; test in Phase 7/8 |
| Clipboard/mic/paste need secure context | Mandate HTTPS or localhost-via-VPN; degrade gracefully otherwise |
| MCP stdio servers need node/npx/uvx on the box | Install in container image; document; SSE transport still unimplemented |
| path_scope guards differ on Linux server | Port `path_scope.rs`; verify Unix banned-roots; keep sensitive-dir blocks active |

---

## 9. Rough time estimate (solo developer)

These are working-time ranges, not calendar dates, and assume the desktop app
keeps building at every step.

| Phase | Scope | Estimate |
|---|---|---|
| 0 | Spike & scaffolding | 2–4 days |
| 1 | `AppContext` refactor (~150 cmds) | 8–15 days |
| 2 | Server transport + bootstrap + secrets | 6–10 days |
| 3 | Auth & security gate | 3–5 days |
| 4 | Frontend transport shim | 4–7 days |
| 5 | Browser-equivalents for native features | 4–7 days |
| 6 | Deploy packaging (+TLS) | 2–4 days |
| 7 | Backend/integration testing | 4–7 days |
| 8 | UI/E2E testing + fixes | 5–10 days |
| **Total** | | **~6–10 weeks** |

Biggest variables: Phase 1 (sheer command count) and Phase 8 (bugs found in real
browser use). The agent streaming path is the most intricate to get right end-to-end.

---

## 10. Suggested execution order

1. Lock the two open decisions (TLS strategy, server OS).
2. Phase 0 spike — if the AppContext shape feels wrong, adjust before scaling.
3. Phases 1→6 in order; keep desktop green as the regression guard.
4. Phases 7→8; fix and re-run until the VM checklist is fully green.
