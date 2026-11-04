# Rustic work checklist (Nov 2026)

Active work list from the Nov 2026 perf / cost / cleanliness audits, agent-observed bugs, and user requests.
`AUDIT.md` stays as the Oct 2026 record. Mark `[x]` done, `[~]` decided against / partial (with reason).

## Bugs: user-reported
- [x] Clickable file paths in tool-call cards (path chip per tool + linkified input/output; images open in preview) — `src/lib/file-links.js`
- [x] Clickable paths in prose: relative/absolute/Windows paths, `file:line[:col]`, `#L12`, inline code, local markdown links
- [x] Paths that rendered as links but didn't open: leading `/` stripped, wrong project resolution, trailing punctuation, `[x](a.js:12)` treated as a scheme; missing files now toast
- [x] Clickable file paths in terminal output (xterm link provider, resolves against the terminal cwd then the projects)
- [~] Scroll in Agent tools → Skills → Browse: that Browse/Install UI doesn't exist in this codebase (the screenshot came from another build); fixed the sheet's flex/min-h-0 layout pattern that breaks inner scrolling in all four library panels

## Bugs: agent-observed
- [x] run_command never detecting completion on macOS (missing macOS "shell has a child" probe; real-pty test for bash/zsh/sh)
- [x] Echoed wrapped command tail in captured output (zsh wrap redraw + zsh/macOS prompt shapes stripped; 4 tests)
- [x] Agent shell matches the system prompt: shared `rustic_agent::agent_shell` picks zsh→bash→sh on macOS/Linux and Git Bash→PowerShell on Windows; the prompt label comes from the same choice; user terminals + CLI agents keep the login shell / PowerShell
- [x] Project tree always hides sensitive files (`.updater-signing.key*`, `.env*`, keys/certs, credentials)
- [x] Project tree noise: asset/fixture dirs collapse to one line, `.rustic/memory/` summarized, gitignored top-level dirs collapsed under bypass
- [x] Blocked sub-agent write reported as "written": Files-written now recorded only on success
- [x] Read-only sub-agents may write scratch under `.rustic/tmp/`
- [x] Sub-agent length limits: child told the limit; result cut at 1.5× with a marker
- [x] Refreshed stale AUDIT.md facts (issues/, find_in_path, P5 baseline)

## Performance: app
- [x] Entry bundle: 2,915 KB → 2,080 KB (lazy Settings dialog, lazy Agent tools sheet, settings no longer imports xterm)
- [x] Host-side stream delta batching on both hosts (the old stream_coalesce.rs was never compiled); 24 ms coalescing, ordering preserved
- [x] Incremental turn building (`src/lib/turn-builder.js`; randomized equivalence check 12,000 steps, 0 mismatches)
- [x] Symbol index stores each entry once, shared paths/names
- [x] SseLineBuffer: incremental scan, one compaction per chunk, no lossy decode on valid UTF-8
- [x] Blocking std::fs in async condense/prompt-dump writes moved off the runtime (and gated behind RUSTIC_DUMP_PROMPTS)
- [x] Hidden-window poll pausing (status bar, cli-activity via `useVisiblePoll`; use-lan/sync-overlay inline)
- [~] Streaming hash for file history: audit was wrong — stat_cache reads its own JSON, shadow needs full bytes to store, shadow.rs:1196 hashes a path

## Cost: agent loop / context
- [x] Tree block noise collapsing
- [x] Tree joins the cached prefix: anchored on its first user message, small +/- delta block on later turns, re-anchored when cold/condensed/large change
- [x] No tree injection for sub-agents
- [x] Age-based tool-result clearing (keep newest 8 / ~40K tokens; batches only when cold or ≥120K chars freeable)
- [x] Dedup + clearing for sub-agents (keep 12)
- [x] Anthropic 1h TTL on tools + system breakpoints (native API only)
- [x] OpenAI `prompt_cache_key` (stable per conversation hash)
- [x] Gemini explicit cachedContents for system + tools (≥4K tokens, second-use creation, TTL, fail-safe retry)
- [x] OpenRouter: cache_control for Anthropic/Gemini routes, `usage.include`, prompt_cache_key, cached-token parsing
- [x] Condense ceiling min(93%, 200K) for >400K windows (fixed constant, not a user setting)
- [x] Fast-tier sub-agents inherit at most 12K chars of parent transcript
- [x] read_file default cap 10K tokens (explicit ranges up to 25K)
- [x] Tool defs sorted by name in every adapter
- [x] Tree instructions moved into the cached STATIC_BODY
- [x] Thinking effort: condenser, goal evaluator and sub-agents already run with thinking_budget 0
- [x] Measurement: `[usage] request completed` log line per request (input / cache read / cache write / output / hit %)

## Cleanup: dead code / duplication
- [x] Deleted `cloud-settings.jsx` and `chat-input.jsx`
- [x] Removed 9 unused `@codemirror/lang-*` deps
- [x] Removed unused frontend exports + the old linkifier helpers in markdown-assets.js
- [x] Removed revert-to-checkpoint leftovers (anchors, snapshot listing, turn-started listener)
- [x] Removed dead worktree-era git helpers (+ merge_ff_only, rebase_in_progress) and remove_shadow_for_worktree; rev_parse kept (used)
- [x] Desktop `find_in_path` limited to Windows (its only caller); unused `Path` import removed
- [x] Never-read serde fields + allows removed (claude.rs, mcp/client.rs); watcher.rs allow is needed (kept)
- [x] Shared SSE helpers (`classify_sse_line` / `parse_sse_json`)
- [x] Shared frontend helpers: copyText, toastError (~47 sites), single formatBytes
- [x] One library panel for Skills/Workflows/Rules (already in the WIP refactor)
- [x] Host logic moved into `crates/rustic-app/src/host_ops/`: formatters, rules, workflows, skills, settings (−2,530 net lines)
- [~] Host dedup round 2 (−1,010 net): shared `batch_deltas`, all 13 editor commands, agent history DTOs/helpers, mic transcription moved to `host_ops/`; stream loop / send_message / create_task / sub-agent park kept per host (host-specific events + secrets, risk to history-loss fixes); fetch_ai_models, OpenRouter specs, list_tasks are the next candidates
- [x] .ps1/.sh pairs merged into cross-platform `scripts/*.mjs` (+ npm scripts)
- [x] Removed android/ios icon sets (no config references them)
- [x] Deleted PLAN.md (open items moved to memory)
- [x] clippy.toml allows unwrap/expect in tests (1,460 → 448 warnings)
- [x] Empty-catch review: user-visible failures in the agent panels now toast/warn; best-effort ones left
- [~] God-file splits: opportunistic by definition; agent/mod.rs shrinks via host dedup; prompt-box/chat-view are being edited by another agent

## Carried over from issues/
- [x] Device → projects + metadata + pull file/folder (already in WIP)
- [x] Cancel on the pairing dialog (already done; e2e test added)
- [x] Address refresh on network change (already done on desktop; server has no mDNS)
- [x] Flaky reconnect: 401 from a one-sided forget now flags "Pair again" (e2e test)
- [x] Cancel a running transfer, both modes (already done; e2e test added)
- [x] Stuck loading: 12 s listing timeout + retry + error state
- [x] Cloudflare popup: prompts now restore + flash the window; real refusal reason instead of "declined"
- [~] Prod logs 06/10/26 15:52 IST: only the marketing website is on Railway; no rustic-server deployment/logs exist for that window
- [~] Cloud & Sync out of Settings: already moved (Settings entry gone); kept the newer sidebar design instead of the dock
- [x] Transfers size / ETA / progress / speed (already done)
- [x] Top-bar transfer icon + popup (already done)
