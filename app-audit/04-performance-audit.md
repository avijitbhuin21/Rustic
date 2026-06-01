# Rustic — Performance Audit (Runtime / Rendering)

**Date:** 2026-06-01
**Branch:** `rebuilt-ui`
**Scope:** Application *runtime* performance — frontend rendering, large-list handling, IPC event volume, hot Rust paths, and bundle/startup.
**Note:** This is distinct from the existing `docs/perf_findings*.md`, which measure the **agent's token/cache/turn efficiency** (LLM-loop cost). That work is not re-covered here. The memory-referenced `audit-report/performance-audit.md` does not exist on this branch.
**Method:** Read-only static review. No profiling/flamegraphs were run — findings are derived from code structure and should be confirmed with a profiler on a large project + long chat session.

---

## Executive summary

The performance-critical infrastructure is **already in good shape**, and in several places notably so. Before listing gaps, the things done right (so they aren't "fixed" needlessly):

- **Aggressive, layered code-splitting.** Every heavy editor surface is `React.lazy` / dynamic `import()`: Monaco, `pdfjs-dist` (+ worker via `?url`), `pdf-lib`, `xlsx`, `@eigenpal/docx-editor-react`, CodeMirror language modules, the diff editor. Startup bundle stays lean; multi-MB deps load only when a matching file is opened. The vite config even documents *why* `manualChunks` was removed (an ESM-cycle `undefined`-import bug) — a real, hard-won decision.
- **Content search is bounded and streamed.** Hard caps (`MAX_FILES 1000`, `MAX_MATCHES_PER_FILE 500`, `MAX_TOTAL_MATCHES 5000`, `MAX_FILE_SIZE 10 MB`; global `5000`/`1500`), batched emission (`std::mem::take` on a pending buffer with a size cap), and the walk runs on `spawn_blocking`. The backend cannot OOM or freeze the UI thread on a huge repo.
- **File tree is virtualized + lazy.** It uses `react-arborist` (row virtualization via `rowHeight`/`height`), and the Rust `read_dir` is a **single-level** walk (`read_directory(path, 0)`) on a blocking thread. Large projects don't pay for what isn't expanded.
- **Terminal rendering uses xterm's WebGL renderer** (GPU-accelerated, paint-batched), and output is delivered in 4 KB chunks that xterm buffers internally. High-volume output (builds, `cat` of a big file) is handled by xterm, not by React state churn.
- **Other wins:** a process-wide `canonicalize` cache (avoids N+1 `stat`s in path scoping), per-project `WorkspaceServices` dedup (1× RAM across concurrent tasks instead of N×), a background-built symbol index, and **per-text-memoized markdown rendering** (`useMemo(renderMarkdown, [text])`) so DOMPurify doesn't re-run for unchanged messages.

The remaining opportunities are concentrated on the **React render side** — specifically two surfaces that are *not* virtualized and a near-total absence of component memoization. These bite on exactly two workflows: **a search that returns thousands of matches**, and **a long agent chat session that's actively streaming**.

| ID | Severity | Title |
|----|----------|-------|
| PERF-01 | Medium | Search results & chat-turn lists are not virtualized (file tree already is) |
| PERF-02 | Medium | Almost no component memoization (1 `React.memo` repo-wide) → re-render cascades during streaming |
| PERF-03 | Low | `buildTurns` / `groupToolResults` recompute O(n) on every streaming delta |
| PERF-04 | Low | `canonicalize_cache` in `file_ops.rs` never evicts (unbounded over very long sessions) |
| PERF-05 | Info | Confirm IPC `terminal-output` event rate under sustained high-volume output |

---

## Findings

### PERF-01 — Two high-volume lists render every row  **[Medium]**

The file tree is virtualized (good), but two other lists are plain `.map()` with no windowing:

- **Search results** (`search/search-results.jsx`): renders `entries.map(...)` × nested `matches.map(...)`. The backend will hand back up to **5000 matches across 1500 files** before truncating — so the DOM can be asked to materialize ~6500 result rows at once. The backend cap protects *memory*; it does nothing for *render/scroll* cost. A broad search (`function`, `import`) in a medium repo will visibly jank.
- **Chat history** (`agent/chat-view.jsx`): `turns.map(...)` renders every turn of the conversation. A long session (dozens–hundreds of turns, each with markdown + tool cards) keeps every turn mounted.

(Commit history is `log.map` but bounded by `max_count` 50–100, so it's fine.)

**Recommendation:** You already depend on a virtualizer indirectly (`react-arborist`) and the patterns are well understood here. Add windowing to the search results list and the chat turn list — `@tanstack/react-virtual` is the lightest fit and works with variable row heights (needed for chat). For search specifically, virtualizing the flattened (file-header + match) row list gives the biggest win because that's the list that can hit thousands of rows.

---

### PERF-02 — Re-render cascades from missing memoization  **[Medium]**

Repo-wide there is **exactly one** `React.memo`. `ChatTurn`, `ChatMessage`, `FileNode`, and the search-result rows are all plain function components exported without memo. Consequences:

- **During streaming**, each text-delta updates `messagesByTask`, which changes the `messages` array reference, which re-runs `buildTurns`/`groupToolResults` and **re-renders every `ChatTurn`** in the list — not just the one receiving tokens. (The expensive part — markdown sanitization — is saved by the per-`text` `useMemo` in `MarkdownBlock`, which is the one thing keeping this from being severe. But React still reconciles every turn on every token.)
- **During a streaming search**, each emitted batch appends to the store and **re-renders the entire results list** (all already-rendered rows), so cost grows with results-so-far on every batch.

**Recommendation:**
- Wrap `ChatTurn`, `ChatMessage`, `FileNode`, and the search row/file-group components in `React.memo` with stable keys, so only the changed item re-renders.
- Ensure the props passed to them are referentially stable (memoize callbacks with `useCallback`, derive per-item data so the item's props don't change unless that item changed). The store already keys data by id, which makes stable per-item selection feasible.
- This pairs with PERF-01: virtualization reduces *how many* mount; memoization reduces *how often* the mounted ones re-render. Do both.

---

### PERF-03 — O(n) turn rebuild per streaming token  **[Low]**

`buildTurns(messages)` and `groupToolResults(messages)` are `useMemo`'d on `[messages]`, but `messages` gets a new reference on **every** streaming delta, so both recompute on every token — each is a full pass over the whole message list. For a long conversation that's O(n) work per token, n = message count.

**Recommendation:** Lower priority once PERF-02 lands (the recompute is cheap relative to re-rendering). If it shows up in a profile, switch to an incremental structure: only the last (streaming) turn changes during a stream, so append/patch the tail instead of rebuilding the whole turn list, or split the streaming turn into its own memoized subtree keyed off only the streaming message.

---

### PERF-04 — Unbounded canonicalize cache  **[Low]**

`file_ops.rs::canonicalize_cache()` is a process-wide `HashMap<PathBuf, PathBuf>` with **no eviction**. Over a very long-lived session that touches many distinct paths (large refactors, repeated agent runs across big trees), it grows monotonically. The per-entry cost is small, so this is a slow leak rather than a hot-path issue, but it's unbounded by design.

**Recommendation:** Cap it (a small LRU, e.g. `lru` crate or a size-bounded map with simple clear-on-threshold). Also consider invalidating entries when the watcher reports a path moved/deleted, since a stale canonical mapping for a since-moved path could mis-resolve.

---

### PERF-05 — Verify sustained terminal output event rate  **[Info]**

`spawn_output_reader` emits one Tauri `terminal-output` event per ≤4 KB read. xterm + WebGL absorbs the rendering, but each event is a full IPC serialization (`String::from_utf8_lossy` → JSON → webview). Under a sustained firehose (e.g. `yes`, a verbose build), the **event rate** itself — not the rendering — could become the bottleneck (IPC + GC pressure from many small event payloads). The agent stream path already has a `stream_coalesce.rs`; the user-terminal path does not appear to coalesce.

**Recommendation:** Profile a firehose case. If event rate is high, add a small time/size coalescing window on the Rust side (accumulate for ~8–16 ms or until a larger threshold, then emit one event) — mirroring what `stream_coalesce` does for agent streams. Likely unnecessary given xterm's buffering, hence Info — but worth a single measurement.

---

## Things checked and found well-optimized (no action)

- **Code-splitting / lazy loading** — exemplary; heavy deps are all dynamically imported.
- **Search** — capped, batched, streamed, off-thread.
- **File tree** — virtualized (react-arborist) + lazy single-level backend walk on `spawn_blocking`.
- **Terminal** — WebGL renderer, chunked output, persistent xterm instances (history survives remounts).
- **Markdown rendering** — DOMPurify result memoized per message text.
- **Rust shared state** — canonicalize cache, per-project `WorkspaceServices` dedup, background symbol index build, debounced file watcher.

---

## Prioritized order

1. **PERF-02** — add `React.memo` to list-item components (cheapest high-leverage win; helps both chat and search immediately).
2. **PERF-01** — virtualize the search-results and chat-turn lists (the two unbounded render surfaces).
3. PERF-03 — incremental turn building, only if a profile shows it after 1–2 land.
4. PERF-04 — bound the canonicalize cache.
5. PERF-05 — one measurement of terminal firehose event rate; coalesce only if warranted.

---

## Suggested validation

Before/after, measure on: (a) a search returning ≥3000 matches — time-to-first-paint and scroll FPS; (b) a 100+ turn chat mid-stream — frame time per token (React Profiler "commit" duration). These two scenarios isolate PERF-01/02 directly.
