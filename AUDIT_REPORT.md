# Rustic — Full Codebase Audit Report

**Date:** 2026-07-06
**Scope:** All Rust crates (`crates/*`), both hosts (`rustic-server/`, `src-tauri/`), React frontend (`src/`), dependencies & build config. Read-only — no code was changed.
**Method:** Four parallel audit passes (core crates, hosts, frontend, deps/build), findings consolidated and de-duplicated below.

**What's already good:** the codebase is unusually defensive. The web_fetch SSRF gate (IP pinning + redirect re-validation), the tier-1/2/3 sensitive-file model, atomic writes, bounded caches (TreeCache, PTY ring buffer, search caps), chat virtualization, DOMPurify on all markdown, lazy-loaded heavy editors, and clean lock discipline (no mutex-across-await found; `unwrap()` density almost entirely confined to test modules) are all well done. The findings below are the gaps.

---

## 1. CRITICAL

### 1.1 Real credential committed in `.env.example`
`.env.example:7,12` — `RUSTIC_AUTH_PASSWORD=avijitbhuin21` and `RUSTIC_SESSION_SECRET= avijitbhuin21` (note the leading-space bug on the second). This is a tracked file containing what appears to be a real personal password. Anyone copying the template ships a known-password instance; `docker-compose.yml` consumes `.env` with no non-default check.
**Fix:** replace with placeholders (`change-me`, `openssl rand -hex 32`); rotate the password anywhere it's actually used; add a server startup check that refuses to run with the example default. Consider the value burned — it's in git history.

---

## 2. SECURITY — High

### 2.1 `run_command` `cwd` is unvalidated and hidden from the approval prompt
`crates/rustic-agent/src/tools/terminal.rs:313-316` — `cwd` gets no scope check; `project_root.join(cwd)` with an absolute path *replaces* the root, so a command can execute anywhere on disk. The permission preview (lines 284-304) shows the full command but omits `cwd`, so a prompt-injected call can display benign `git status` while running it in `~/.ssh`. File tools rigorously scope paths; the exec tool doesn't.
**Fix:** resolve `cwd` through the same canonical-prefix check as file paths; append `[cwd: …]` to the approval preview whenever it differs from the project root.

---

## 3. SECURITY — Medium

### 3.1 FullAuto + web tools = silent secret-exfiltration channel
`crates/rustic-agent/src/tools/file_ops.rs:296-337` — in FullAuto, tier-2 (`.env`, `credentials*`, `*.token`) and tier-3 (gitignored) prompts are skipped entirely. Combined with `web_fetch`/`web_search` in the same mode, a prompt-injected agent can read `.env` and exfiltrate it via a crafted URL with no user-visible approval. Tier-1 still blocks, but tier-2 is where real API keys live. The tier-2 matcher also misses `production.env` / `foo.env` (only `.env` / `.env.*`).
**Fix:** keep the tier-2 prompt even in FullAuto, or taint-track (once a tier-2 file is read, require approval for later `web_fetch`/`run_command`). Widen matcher to `ends_with(".env")`.

### 3.2 `X-Forwarded-For` spoofing bypasses the login rate limiter
`rustic-server/src/app.rs:113-121` — `client_ip` blindly trusts the first `X-Forwarded-For` value. On deployments without a trusted reverse proxy (Railway direct, bare Docker), an attacker spoofs the header per-request and defeats the per-IP login brute-force limiter entirely.
**Fix:** default to socket peer IP; only trust the header when a configurable `RUSTIC_TRUSTED_PROXIES` CIDR list is set and the peer matches.

### 3.3 `/proxy/:port` has no port allow-list (SSRF to loopback services)
`rustic-server/src/proxy.rs:43-55` — the tunnel proxy connects to `http://127.0.0.1:<port>` for any `u16` from the URL, with no validation. An authenticated user or prompt-injected agent with a session token can reach any loopback service (DB port, admin panels, metadata endpoints).
**Fix:** enforce an allow-list of ports actually registered by the user's dev servers (the `port_monitor` already tracks listening ports); 403 everything else.

### 3.4 Gemini API key embedded in URLs; reqwest errors echo the URL
`crates/rustic-agent/src/tools/media_tools.rs:1016-1431` — image/video calls use `...?key={api_key}`. `reqwest::Error`'s `Display` includes the full URL, and these errors are formatted into `ToolOutput` and stored in the conversation transcript/DB — so a transient network error persists the API key into chat history.
**Fix:** send the key via the `x-goog-api-key` header; scrub `key=` from any error string before surfacing.

### 3.5 `web_fetch` / skills+workflows `download_bytes` buffer the whole body before the size cap
`crates/rustic-agent/src/tools/web_tools.rs:524-544`; `rustic-server/src/commands/skills.rs:477`, `workflows.rs:448` — `resp.bytes().await` buffers the entire response, then checks `MAX_FETCH_BYTES`. A malicious/huge endpoint can stream hundreds of MB (or an infinite chunked body until timeout) into memory. No `Content-Length` pre-check.
**Fix:** check `Content-Length` first, then read via `resp.chunk()` accumulating to `cap + 1` and abort on overflow.

### 3.6 No CORS policy on the web server
`rustic-server/src/app.rs` — no `CorsLayer`. Same-origin deploys are fine, but a cross-origin `fetch` (with a stolen token) executes mutating commands before the browser blocks the *response*.
**Fix:** add a `CorsLayer` restricted to configured allowed origins on protected routes.

### 3.7 Untrusted model-output links piped to `shell.open()` without a scheme allow-list
`src/components/agent/chat-turn.jsx:47-56`; same in `markdown-preview.jsx:99`, `svg-preview.jsx:63` — agent/model markdown `<a href>` is handed to Tauri `shell.open()`. DOMPurify strips `javascript:`/`file:`, but defense-in-depth is thin (one config regression from `file:`/custom-scheme launch), and links can disguise their real target.
**Fix:** centralize via the (currently dead) `src/lib/markdown-assets.js`, enforce `http/https/mailto` via `new URL(href)` before `shell.open`. Also tighten the Tauri `shell:allow-open` capability (§3.10) — the two compound.

---

## 4. SECURITY — Low

- **4.1 Logout doesn't bump session generation** — `rustic-server/src/app.rs:167-180`: copied tokens (`?token=`, Bearer) stay valid until TTL. `power::shutdown` already bumps `session_gen`; wire logout to do the same (~3 lines).
- **4.2 Session token in `?token=` query param** — `app.rs:276-283`: needed for WS upgrades, but logged by proxies. Mitigate with short TTL + a one-time `ws_ticket` exchange.
- **4.3 `FileSecretStore` cleartext + no Windows ACL** — `crates/rustic-app/src/secrets.rs:88-101`: `lock_down_perms` is a unix-only no-op; `secrets.json` sits with inherited ACLs on Windows. Also a doc/behavior mismatch: `get()` reads only the in-memory cache despite claiming per-access re-read.
- **4.4 Tauri CSP `connect-src https: ws: wss:`** — `src-tauri/tauri.conf.json`: fully-open network egress; a future DOMPurify regression could exfiltrate anywhere. Tighten to the explicit provider allow-list.
- **4.5 Tauri `shell:allow-open` unscoped** — `src-tauri/capabilities/default.json:17`: restrict to `["https://*","http://*","mailto:*"]`.
- **4.6 SSRF gate residual gaps** — `web_tools.rs:737-786`: strong design; missing NAT64, some reserved IPv4 ranges. Low practical risk.
- **4.7 git token visible in process listings** — `crates/rustic-git/src/remote.rs:34-50`: `-c http.extraHeader=…` in argv (documented tradeoff); optionally use `GIT_CONFIG_*` env vars.
- **4.8 Worktree hook naive shell quoting** — `crates/rustic-app/src/worktree.rs:257-272`: `format!("{cmd} \"{arg}\"")`; no injection today (arg is an internal task id) but fragile. Pass via env var.
- **4.9 HTML-preview iframe `allow-same-origin`** — `src/components/editor/previews/html-preview.jsx:249`: safe today (no `allow-scripts`); drop `allow-same-origin` so adding scripts later can't execute with app origin.

---

## 5. PERFORMANCE

### High (frontend streaming hot path)
- **5.1 Per-token wide re-render of the task tree** — `src/state/agent.js:1151,1188` writes a new `streamingByTask` object identity even when the value is already `true`; `agent-task-tree.jsx:238-239` subscribes to the whole map, so every `ProjectSection` + task row re-renders on every streamed token.
  **Fix:** only write when `!s.streamingByTask[taskId]`; subscribe per-task (`(s) => !!s.streamingByTask[task.id]`).
- **5.2 O(n²) main-thread work per streamed token** — `chat-view.jsx:719-774` re-scans the entire transcript in four `useMemo`s per delta, and `chat-turn.jsx:26-36` re-runs `marked.parse` + DOMPurify over the whole accumulated block, re-attaches listeners, and re-decorates code blocks — per token.
  **Fix:** batch deltas in the store (~30–50 ms rAF/timeout flush before `set`). Also shrinks the blast radius of 5.1.

### Medium
- **5.3 Gitignore matcher rebuilt on every file access** — `file_ops.rs:339-345`: `check_sensitive_path` re-reads `.gitignore` and rebuilds a `GitignoreBuilder` per call; batch reads pay it N times. Cache the built `Gitignore` keyed by `.gitignore` mtime.
- **5.4 MCP call timeout leaks a blocking thread + wedges the client mutex** — `task/executor.rs:2335-2380` (dup at 2462): on timeout the `spawn_blocking` closure keeps running holding the per-client `Mutex`, and the stdio stream desyncs (late reply becomes the response to the *next* call — see 8.1). Give `McpClient` an internal per-request deadline; mark broken + reconnect on executor timeout.
- **5.5 `strip_html` is O(n²)** — `web_tools.rs:865-903`: `rest.to_ascii_lowercase()` per `<script>`/`<style>` block copies the whole remaining document each iteration. Lowercase once, or use case-insensitive search.
- **5.6 `std::sync::Mutex` for DB/agent state in async handlers** — `crates/rustic-app/src/state.rs`: the `db` lock is taken on almost every request; a future refactor could hold it across an `await`. Migrate `db` to `tokio::sync::Mutex`; keep others but guard against await-holding.
- **5.7 SCM panel renders unvirtualized file lists** — `src/components/scm/scm-panel.jsx:742,795`: thousands of changed files jank. Cap + "show more" or reuse `@tanstack/react-virtual`.
- **5.8 `thinkingByTask` grows unboundedly and is never rendered** — `agent.js:1189-1192`: dead side-map appended per thinking delta. Delete it (thinking already lives in content blocks).

### Low
- **5.9 Per-request `reqwest::Client` in proxy** — `rustic-server/src/proxy.rs:147-157`: build one `Client` at startup and share via state (HMR is high-RPS).
- **5.10 Directory download buffers the whole ZIP in memory** — `rustic-server/src/app.rs:676-712`: stream the ZIP incrementally instead of `Vec<u8>`.
- **5.11 Rate-limiter map unbounded** — `rustic-server/src/auth.rs:87-139`: random-IP scans grow it forever; add periodic eviction / max size.
- **5.12 Foreground commands hold a runtime thread in a 50 ms poll loop** — `terminal.rs:583-630`: use async `tokio::process` + `timeout(child.wait())`.
- **5.13 Full conversation clone per stream attempt** — `executor.rs:1385`: `Arc<[Message]>` or clone only on retry.
- **5.14 Unbounded canonicalize cache, never invalidated** — `file_ops.rs:11-34`: also a stale-path correctness smell; bound + key to mtime.
- **5.15 `read_tail` copies PTY ring byte-by-byte** — `pty.rs:275-282`: use `as_slices()`.
- **5.16 No SQLite `busy_timeout`** — `connection.rs:45-47`: desktop+server sharing a data dir get immediate `SQLITE_BUSY`; add `PRAGMA busy_timeout=5000`.
- **5.17 Debug logging left at `warn!` in hot paths** — `executor.rs:2431`, `subagent_tools.rs:1477-1494`: demote to `trace!`/`debug!`.

---

## 6. REDUNDANT / DEAD CODE

### High — host duplication (~12,000 lines kept in sync by hand)
`rustic-app` exists as the shared crate; most of this belongs there behind a `dyn EventEmitter` adapter.
- **6.1 `rustic-server/.../agent_chat.rs` (~3.2k) vs `src-tauri/.../agent/mod.rs` (~3.5k)** — the entire streaming agent loop (start/send/resume/cancel, goal, cost, thinking accounting, sub-agent wiring). The single highest-leverage refactor.
- **6.2 `download_text`/`download_bytes`/`download_repo_zip` quadruplicated** — server+tauri × skills.rs+workflows.rs. Fixing the 3.5 size cap currently means 4 edits. Move to a shared `github_download` module.
- **6.3 `terminal.rs`, `editor.rs`, `formatters.rs`, `git.rs`, `search.rs`, `preview.rs`, `workspace.rs`, `settings.rs`, `rules.rs`, `worktree.rs`** — near-identical server/tauri pairs (~5k lines). Migrate smallest-first: `search`/`preview` → `workspace` → `git` → `formatters`.
- **6.4 `provider_account()` duplicated** — `crates/rustic-app/src/secrets.rs:44` vs `src-tauri/src/secrets.rs:52`.

### Medium — crate-level duplication
- **6.5 `atomic_write` triplicated verbatim** — `rustic-core/io_util.rs`, `rustic-git/io_util.rs`, `rustic-agent/io_util.rs` (only core has tests). A future fsync/rename fix lands in one and misses two. Extract a leaf `rustic-io` (or host it in `rustic-git`, which both others reach).
- **6.6 `markdown-assets.js` is 100% dead but holds the canonical link handler** — `src/lib/markdown-assets.js`: never imported, yet contains the correct `handleMarkdownLinkClick` + `rewriteLocalAssetSrcs` that three components hand-rolled (chat-turn, markdown-preview, svg-preview). Local-image rewriting is silently unwired. Resurrect it as the single handler (also fixes 3.7).
- **6.7 `ui/agent-plan.jsx` (624 lines) — dead**; also dead: `ui/card.jsx`, `ui/avatar.jsx`, `editor/find-replace.jsx`, `editor/previews/diff-preview.jsx` (verified zero import sites).

### Low
- **6.8 `resolve_with_scope` vs `resolve_within_project`** — `file_ops.rs:130-201`: same algorithm; make one a wrapper.
- **6.9 MCP dispatch block copy-pasted** — `executor.rs:2321` & `2444`: extract one `call_mcp_tool` (fixes 5.4 once).
- **6.10 Redundant SSRF host parsers** — `web_tools.rs:684` vs `788`: one canonical host extractor so the two gates can't disagree.
- **6.11 `isTauriAvailable` check copy-pasted in 14 files** — centralize in `src/lib/platform.js`.
- **6.12 Two near-identical lightbox `<Dialog>`s** — `chat-turn.jsx:155-280`: extract one `<ImageLightbox>`.

---

## 7. DEPENDENCIES & BUILD

### High
- **7.1 `notify` major-version split (v7 vs v8)** — `rustic-agent` uses notify@8 while `rustic-app`/`src-tauri` use notify@7; both compile into every binary. Unify on 8 (migrate `rustic-app/src/watcher.rs`).
- **7.2 No `[workspace.dependencies]`** — root `Cargo.toml` repeats 18+ deps across 4–8 crates; `base64` has already drifted (0.21 in rustic-git vs 0.22 elsewhere). Centralize; this is the root cause of drift like 7.1.
- **7.3 Provably-unused frontend deps** — remove `fortune-sheet`, `xlsx` (CDN-tarball, supply-chain smell), `unidiff`, all 13 scoped `@radix-ui/react-*` (code uses the `radix-ui` umbrella), and move `shadcn` out of runtime deps.
- **7.4 CI gaps** — no `cargo audit`/`cargo deny` (network-heavy server), no `cargo fmt --check`, no frontend lint; Rust CI is Windows-only while the server deploys on Linux (add an `ubuntu-latest` `cargo check -p rustic-server`).

### Medium / Low
- **7.5 Dockerfile non-reproducible** — Node (`latest`), Go (`go.dev/VERSION`), and cloudflared (`releases/latest`) all fetched dynamically with no pin/checksum. Pin versions as `ARG` + verify checksums.
- **7.6 `release.ps1`** — empty `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; no `--locked` on the release build.
- **7.7 `.gitignore` gaps** — add `*.db`, `*.db-wal`, `*.db-shm`. (`app-audit/`/`docs/` already removed in this task.)
- **7.8 Caddyfile has no security headers** — add HSTS, `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`.
- **7.9 `run-server.ps1`** — `-BindAll` + default password only warns; should abort.
- **7.10 No bundle visibility** — add `rollup-plugin-visualizer` (dev) to confirm Monaco/pdfjs/prettier/Univer chunking.

---

## 8. CORRECTNESS SMELLS

- **8.1 MCP stdio desync after timeout** (see 5.4) — a timed-out call's late reply becomes the response to the *next* request; results attributed to the wrong tool call.
- **8.2 Windows `path_is_in_system_root` uses substring `contains`** — `crates/rustic-app/src/path_scope.rs:191-211`: misses UNC system paths / `Windows.old`; prefer a component-wise check.
- **8.3 `EnvSecretStore` test mutates process env** — `secrets.rs:196-201`: `set_var` races parallel tests; add a serial guard.
- **8.4 Foreground kill can hang on grandchild pipe** — `terminal.rs:611-616`: a grandchild holding stdout keeps `read_to_end` alive past the watchdog; bound the join.

---

## Top 10 Priorities (cross-cutting)

1. **1.1** Purge the real credential from `.env.example` + rotate (Critical).
2. **2.1** Scope-check `run_command` `cwd` and surface it in the approval preview.
3. **3.2 / 3.3 / 4.1** Server auth hardening: don't trust `X-Forwarded-For` by default, port allow-list the proxy, invalidate tokens on logout.
4. **5.1 + 5.2** Kill per-token wide re-renders + batch streaming deltas — biggest UX perf win.
5. **3.1** Close the FullAuto tier-2-read → web-exfiltration channel; widen the `.env` matcher.
6. **5.4 / 8.1** Fix the MCP timeout thread/mutex leak + stream desync (dedup 6.9 while there).
7. **3.5** Stream-cap all body downloads (`web_fetch` + the 4 `download_bytes` copies) — memory-safety.
8. **7.1 + 7.2** Unify `notify`, add `[workspace.dependencies]`, fix the `base64` drift.
9. **7.3** Prune unused frontend deps (`fortune-sheet`, `xlsx`, `unidiff`, scoped radix, `shadcn`).
10. **6.6 + 3.7** Resurrect `markdown-assets.js` as the single link handler with an `http/https/mailto` allow-list; delete the dead components (6.7).

---

## Decisions that need your judgment (not bugs)
- **Tauri `devtools` in release builds** (`src-tauri/Cargo.toml:25`) — documented as intentional; end-users can inspect the app. Fine for a personal tool, flag before public distribution.
- **`prettier` / `monaco-editor` in runtime `dependencies`** — intentional (Web Worker / direct worker setup); left as-is.
- **Deliberate host duplication** — §6.1–6.4 note that some duplication was a conscious "no silent fallbacks" choice; the refactor is a large effort, listed as a maintenance-cost observation, not a demand.
