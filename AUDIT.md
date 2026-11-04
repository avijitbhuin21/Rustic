# Rustic audit checklist (October 2026)

Replaces the old `issues/issues` checklist; every item in it was already done.
Tags: **[C] [H] [M] [L]**.
Status: `[x]` fixed · `[~]` reviewed and accepted (reason given) · `[ ]` deferred (reason given).
Rustic rule: backend fixes land in BOTH `src-tauri` and `rustic-server` unless noted.

**Verification (Oct 2026 pass):**
- `cargo check --workspace --all-targets`: 0 warnings, 0 errors.
- `cargo clippy --workspace --all-targets`: exit 0.
- `cargo audit` (no ignores): exit 0, no vulnerabilities.
- `cargo test -p rustic-server --test integration`: 10/10 pass (previously every `/api` test panicked).
- `cargo test -p rustic-app peers_round_trip`: passes.
- `bun run build`: OK.

---

## 1. Repo cleanup

- [x] Removed `issues/` (all 16 items done); this file replaces it. (It had been re-added in v0.7.6 with peer-sync notes; actually removed Nov 2026, items carried into `CHECKLIST.md`.)
- [x] Deleted `HOW-TO-USE.md` (Lattice docs). The website "How to use" link now points to the README (`website/static/index.html`).
- [~] `references/` (~105 MB, gitignored): reference source downloaded on purpose and not recoverable from git. **Kept**; delete by hand if no longer wanted.
- [x] Deleted `rustic-data/` (local Chromium profile, recreated on demand) and the empty `.codex/`. Added `.codex/` to `.gitignore`.
- [~] `dist/`: gitignored build output, left as is.
- [~] `src-tauri/gen/schemas/*`: kept tracked (Tauri default; editors use it for capability-schema completion).
- [x] `README.md` updated:
  - real crate layout (`rustic-io`, `rustic-app`, `rustic-server`, `vendor/`, `website/`, `deploy/`);
  - new features;
  - correct `path_scope` path;
  - server secrets storage;
  - updater/release status.
- [x] `.dockerignore` excludes `.updater-signing.key*`, `PLAN.md`, `references/`, `issues/`, `.codex/`.
- [x] `.gitattributes` forces LF on `*.sh` + `Dockerfile` (the container entrypoint breaks with CRLF).

## 2. Code hygiene / dead code

- [x] Deleted `src/assets/new_logo.png` and `src/assets/ai_agent_animated.svg`.
- [x] Deleted the spike/probe examples (`walk_timing_spike.rs`, `istty_probe.rs`, `fullscreen_probe.rs`).
- [x] `rustic-server/src/app.rs` `match_preview_host` / `host_proxy_middleware`: these were the missing half of the "subdomain" tunnel mode, which can be picked in Settings but was never routed. **Wired up** as the outermost layer (reads the live tunnel config) instead of deleted.
- [x] `rustic-server/src/commands/terminal.rs` `find_in_path`: actually has 11 callers. Only the stale `#[allow(dead_code)]` was removed. (The separate desktop copy in `src-tauri/src/commands/terminal.rs` had no callers; removed Nov 2026.)
- [x] Removed:
  - dead `FileTrackedKindPayload::EditTool`;
  - unused `CreateTaskArg.project_name/project_root` and `RespondPermissionArg.task_id` (serde ignores the extra wire fields).
- [x] Removed unused `AgentTask.cost` (`crates/rustic-app/src/state.rs`) and its initializers on both hosts.
- [x] Removed:
  - unused `McpClient.config`;
  - `local_timestamp_compact` and its two helpers (`src-tauri/src/commands/file_tree.rs`);
  - the stale allow on `KeychainSecretStore`.
- [~] The remaining allows on `JsonRpcResponse` and the Claude SSE enums are on deserialization-only types, so they're legitimate.
- [~] `package.json`: `rollup-plugin-visualizer` was already in `devDependencies` (the finding was wrong).
- [x] `[term-diag]` terminal diagnostics now only log when `localStorage['rustic.terminal.debug']='1'`.
- [x] Removed the operational `console.log` noise from `src/state/agent.js`.
- [x] `rustic-server/tests/integration.rs`:
  - harness fixed (a sync `run` helper drops the last router, and its `Arc<Runtime>`, outside async context);
  - added an S2 regression test.
- [x] `cargo machete`: removed 18 unused dependencies:
  - `src-tauri`: axum, hyper, hyper-util, rustls, tokio-rustls, rcgen, mdns-sd, tokio-util, futures-util (left over after LAN sync moved to `rustic-app`);
  - `rustic-git` from agent and app; `ignore` from app; `tracing` from core; `anyhow` + `uuid` from db; `serde_json` from terminal; `hyper` from server; `tower` from website.
  - Machete now reports none.

---

## 3. Security

`.env` and `.updater-signing.key` were never committed (checked against git history).

### High
- [x] **S1** The session-secret fallback (`crates/rustic-app/src/config.rs`) now comes from the OS CSPRNG (ring `SystemRandom`).
  - No start-gate: with a CSPRNG secret, the only cost of leaving `RUSTIC_SESSION_SECRET` unset is sessions not surviving a restart.
- [x] **S2** `/api/download` and `/api/asset` refuse Rustic's internal data:
  - `secrets.json`, `rustic.db*`, `mcp*.json`, `sync-*`;
  - `lan/`, `file-history/`, `browser-profile/`, `bin/`, `logs/`, `external-agents/`.
  - Projects and media under the data dir stay readable, and `/proc` was already blocked.
  - Test: `download_refuses_internal_data`.
- [x] **S3** Bumped `pdf-extract` 0.12 (`lopdf` 0.42), `calamine` 0.36, `docx-rs` 0.4.22 and `plist` 1.10 (all now on `quick-xml` ≥0.41). CI `--ignore`s removed.
- [x] **S5** Skill-zip install (both hosts) rejects entries containing `/`, `\` or `:`, and entries named `.` or `..`.

### Medium
- [x] **S4** `rustls` is now 0.23.45 (RUSTSEC-2026-0285 fixed). The unmaintained notices (`fxhash`, `proc-macro-error`, `serial`, `ttf-parser`, `unic-*`) are warnings only and transitive.
- [x] **S6** LAN peer tokens (`crates/rustic-app/src/peer/listener.rs`):
  - compared in constant time on SHA-256 hashes;
  - requests carrying an unknown bearer token wait 750 ms before they're handled, which throttles guessing.
- [x] **S7** `peers.json` stores `token_in` as `sha256:<hex>`. Existing plaintext entries migrate on the next load (covered by `peers_round_trip`).
- [x] **S8** The container drops root.
  - `deploy/docker-entrypoint.sh` fixes `/data` ownership, then runs as the uid-1000 `rustic` user via `setpriv`.
  - The toolchain homes are handed to that user, and `npm -g` goes to `/data/npm-global`.
  - Opt-outs: `RUSTIC_RUN_AS_ROOT=1`; `RUSTIC_CHROMIUM_SANDBOX=1` turns Chromium's own sandbox back on.
- [x] **S9** The signing key is excluded from the Docker build context.
- [~] **S10** The devtools iframe keeps `allow-same-origin`:
  - it runs Chromium's own DevTools frontend behind the authenticated proxy (trusted code);
  - the inspected page renders inside headless Chromium, not in this frame;
  - same-origin access is needed for the style injection.
- [x] **S11** Every third-party action in `ci.yml` and `release.yml` is pinned to a commit SHA, and `ci.yml` has `permissions: contents: read`.
- [x] **S12** `RUSTIC_DEBUG_PROVIDER_BODY` is documented as debug-only in `.env.example`.

### Low
- [x] **S13** Docker: `uv`, `air`, `typescript`, `ts-node` and `cargo-chef` are pinned, and the `|| true` that swallowed failures is gone.
- [~] **S14** Desktop CSP `img-src https:` / `connect-src localhost:*`: kept on purpose. Remote images in markdown are a feature, and local dev-server previews need loopback.
- [x] **S15** `process:default` narrowed to `process:allow-restart` (the frontend only calls `relaunch`).
- [~] **S16** The proxy's listening-port check relies on Linux `/proc`. That's acceptable under the single-user model, and only authenticated users can reach it.
- [~] **S17** Verified: project `.mcp.json` loading is gated on a content-hash consent (`crates/rustic-agent/src/mcp/mod.rs:198-218`).
- [ ] Follow-up pass not done yet: webhook HMAC re-check, `sync_transfer.rs` limits, cloudflared download verification, CLI-agent hook command construction, notebook bridge args, `bun audit`.

---

## 4. Optimization

### High
- [x] **P1** About 60 DB-, file- and process-heavy desktop commands now run off the UI thread (`#[tauri::command(async)]`):
  - `get_task_messages`, `get_message_block`, `truncate_task_messages`, `repair_task_history`, task rename/pin/goal, provider set/remove;
  - cost and sub-agent reads;
  - memory and MCP json;
  - rules, skills, workflows, settings and themes;
  - search replace, log reader, `git_check_available`, `formatter_list`.
  - Cheap getters and terminal I/O stay sync. The server was already async.
- [x] **P2** One recursive OS watcher per project now feeds both the UI and file-history capture.
  - `FileWatcherManager` owns the single watcher and forwards raw events to file-history feeds before the UI filtering.
  - `rustic_agent::FileWatcher::attach` reuses the same `notify_debouncer_full` logic with no OS watch of its own.
  - Both hosts are wired through `subscribe_file_history`.
  - Test: `watcher::tests::one_os_watcher_feeds_ui_and_file_history`.
- [x] **P3** `rustic_db::ReadPool` gives read-only WAL connections (up to 4 idle), exposed as `AppState::read_db`.
  - Moved onto it, on both hosts: `get_task_messages`, `list_tasks`, `get_message_block`, `get_task_todos`, archived history.
  - Cloud-sync import clears the pool before swapping the DB file.
  - Tests: `read_pool::tests` (readers see committed writes and are read-only; clear discards stale checkouts).

### Medium
- [x] **P4** xterm and its addons moved out of the entry chunk into a lazy `terminal-instance` chunk (497 KB). `TerminalPane` is loaded through `src/components/terminal/lazy-terminal-pane.jsx`, and the store imports the instance module lazily.
- [x] **P5** Baseline recorded: entry `index-*.js` is about 2,078 KB. Monaco (about 4.6 MB) and the previews are already lazy. (Had drifted to ~2,711 KB by Nov 2026; re-addressed in `CHECKLIST.md`.)
- [x] **P6** The parent-history snapshot is only cloned on turns that call `spawn_subagent`.
  - [x] `AiProvider::chat` now takes `&[Message]` / `&[ToolDef]`. All five providers and every caller (executor, condense, goal, commit message, web summarize) were updated, and the per-retry history and tool-def clones are gone.
- [x] **P7** In-memory transcripts are cached per task as slimmed `MessageDto`s.
  - The cache is keyed by `history_fingerprint`, an exact streamed hash of the history, so any change invalidates it. It holds up to 32 tasks on each host.
  - Timestamps are re-attached from the DB on every call.
  - Test: `history_fingerprint_tracks_any_change`.
- [x] **P8** The Docker build uses `cargo-chef`, so a source-only change no longer rebuilds every dependency.

### Low
- [x] **P9** Status-bar resource polling and the LAN / remote-backend polls in Cloud settings pause while the window is hidden. The retry banner ticks at 1 s.
- [x] **P10** `[profile.dev.package."*"] opt-level = 1`.
- [x] **P11** Server hub events are serialized once at publish (`Arc<str>`), and publishing is skipped when there are no subscribers.
- [x] **P12** Browser window-rect `localStorage` writes are debounced (250 ms).
- [ ] Not profiled: provider SSE string handling, TreeCache, symbol-index memory, file_history sweep.

## Verified elsewhere (user decision)
- [~] The Docker image (S8, S13, P8) is tested on the Railway production deploy, per the user's instruction (Docker Desktop doesn't run on this machine).
