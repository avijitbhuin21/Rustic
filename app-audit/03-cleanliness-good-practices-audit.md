# Rustic — Code Cleanliness & Good-Practices Audit

**Date:** 2026-06-01
**Branch:** `rebuilt-ui`
**Scope:** Whole repo. Focus per request: stray/unrequired files, oversized docstrings/comments, dead code, robustness practices, module size, and general hygiene.
**Method:** Read-only static review + repo-wide metrics. No changes made.

---

## Executive summary

**This is a clean codebase.** The hygiene metrics are well above average for a project of this size (~88k LOC):

- **Only 3 TODO/FIXME/HACK markers** in the entire source tree.
- **No `todo!()` / `unimplemented!()` stubs** — no half-finished code paths shipped.
- **~23 lines of commented-out code** total — essentially none.
- **Comment ratio ~13%** in Rust — proportionate, not bloated.
- **Only 8 `#[allow(dead_code/unused)]`** escape hatches.
- **Only 6 stray `console.log/debug/info`** in the frontend (the rest of console usage is legitimate `warn`/`error`).

The verbose, rationale-heavy comment style (multi-paragraph "why" blocks) is, on balance, a **strength** — it documents hard-won decisions (ConPTY EOF deadlocks, DNS-rebinding, panel-tree keying). The findings below are mostly small hygiene items. The two that genuinely warrant action are **CLN-01** (a committed junk file) and **CLN-02** (the systemic `lock().unwrap()` panic pattern on the IPC surface).

| ID | Severity | Title |
|----|----------|-------|
| CLN-01 | **Medium** | `src-tauri/Pasted text #1` — unrelated chat transcript committed to git |
| CLN-02 | Medium | 103 `lock().unwrap()` on the IPC command surface — one poisoned mutex cascades into repeated panics |
| CLN-03 | Low | 16 source files exceed 1000 lines; several exceed 2400–3600 (god-modules) |
| CLN-04 | Low | `build_output.log` (18 KB) left in `src-tauri/` working tree |
| CLN-05 | Low | Hardcoded app version in source (`status-bar.jsx`) — also flagged in UI/UX audit |
| CLN-06 | Low | A few 25+ line block comments could be trimmed to the essential "why" |
| CLN-07 | Info | 461 non-lock `.unwrap()` across crates — mostly tests, a few real (e.g. `Runtime::new().unwrap()`) |

---

## Findings

### CLN-01 — Junk file committed to the repo  **[Medium]**

`src-tauri/Pasted text #1` (1.5 KB) is **tracked by git** and contains an **unrelated chat transcript** — a conversation about training an ML model on heart-murmur sounds and generating a PDF report. It has nothing to do with Rustic. Beyond being clutter in a security-sensitive directory (right next to `tauri.conf.json` and `capabilities/`), it leaks unrelated personal/project context into the repository history.

**Recommendation:** `git rm "src-tauri/Pasted text #1"` and commit. The filename (with a space and `#`) also suggests an accidental editor paste-to-file. Consider adding a pre-commit guard or a `.gitignore` entry for `Pasted text*` to prevent recurrence.

---

### CLN-02 — `lock().unwrap()` is the default on the IPC surface  **[Medium, Robustness]**  — ✅ RESOLVED 2026-06-01

> **Resolved:** Added `src-tauri/src/sync_ext.rs` (`MutexExt::lock_safe()`), which recovers the guard from a poisoned mutex (via `PoisonError::into_inner`) and logs once at the recovery site — the same no-poison behaviour `parking_lot` gives, without a cross-crate type change. All 117 production `state.*.lock().unwrap()` sites across 13 files in `src-tauri/src` were converted to `lock_safe()`. A panic inside one command can no longer cascade-panic the whole subsystem. Covered by unit tests in `sync_ext.rs`. Original finding follows for context.


Across `src-tauri/src/commands/` there are **107 `.unwrap()` calls, 103 of which are `lock().unwrap()`** on shared `Mutex` state (`terminal_manager`, `workspace`, `git_token`, etc.). Repo-wide that pattern appears **169** times.

```rust
let mut manager = state.terminal_manager.lock().unwrap();   // terminal.rs, many sites
let workspace = state.workspace.lock().unwrap();            // git.rs
```

**Why it matters:** Rust poisons a `Mutex` if *any* thread panics while holding it. Once poisoned, **every** subsequent `lock().unwrap()` on that mutex panics too. So a single panic inside one terminal/git/workspace operation doesn't fail gracefully — it permanently bricks that subsystem for the rest of the process lifetime, with each later command thread panicking on lock. For a long-lived desktop app, this turns a recoverable one-off error into a "restart the app" situation.

**Recommendation:** This is a deliberate-looking convention, so fix it conventionally:
- Prefer `lock().map_err(|_| "…state lock poisoned…".to_string())?` in command handlers (they already return `Result<_, String>`), so a poisoned lock surfaces as a normal error the UI can toast — not a panic.
- Or adopt `parking_lot::Mutex` (no poisoning, and faster), which removes the failure mode entirely and lets you drop the `.unwrap()` on every lock.
- The `tokio::runtime::Runtime::new().unwrap()` at `agent/mod.rs:1239` is a genuine non-lock unwrap — handle the error or document why it can't fail there.

---

### CLN-03 — Several god-modules over 1000 lines  **[Low, Maintainability]**

16 files exceed 1000 lines. The heaviest:

| Lines | File |
|------:|------|
| 3568 | `crates/rustic-agent/src/tools/file_ops.rs` |
| 3440 | `src-tauri/src/commands/agent/mod.rs` |
| 2574 | `crates/rustic-agent/src/task/executor.rs` |
| 2439 | `src/state/agent.js` |
| 2270 | `src/components/settings/agent-settings.jsx` |
| 1944 | `src/components/agent/prompt-box.jsx` |
| 1901 | `crates/rustic-agent/src/tools/subagent_tools.rs` |

These are the parts of the system most likely to be touched concurrently and hardest to review. `file_ops.rs`, for instance, mixes quote-normalization helpers, path-scope resolution, sensitive-file tiering, and the actual tool implementations in one file.

**Recommendation:** No need to chase a line-count target, but the top offenders would benefit from splitting along the seams that already exist in them: e.g. `file_ops.rs` → `path_scope` / `sensitive` / `edit` / `read` submodules; `agent/mod.rs` → split the command groups; `agent-settings.jsx` → per-section components. Do it opportunistically when next editing each, not as a big-bang refactor.

---

### CLN-04 — Build artifact in the working tree  **[Low]**

`src-tauri/build_output.log` (18 KB) sits in the working tree. It is correctly **gitignored** (`*.log`), so it won't be committed — but it's stale clutter in a hand-edited directory and can be confusing.

**Recommendation:** Delete it; it's regenerated by builds anyway. Consider directing build logs to `target/` or a `logs/` dir that's outside hand-edited source folders.

---

### CLN-05 — Hardcoded version string  **[Low]**

`src/components/shell/status-bar.jsx` hardcodes `Rustic v0.3.1` while the app is at `0.3.4`. (Cross-referenced as UX-03 in the UI/UX audit — listed here too because it's fundamentally a "single source of truth" hygiene issue.)

**Recommendation:** Inject the version from `package.json`/`tauri.conf.json` at build time.

---

### CLN-06 — A few oversized block comments  **[Low]**

The longest consecutive comment runs are 27 lines (`provider/claude.rs`) and 26 lines (`task/executor.rs`). Most of the verbose commenting is *good* (it documents non-obvious decisions), but a 25+ line uninterrupted prose block in the middle of a function is more than is needed and tends to drift out of date.

**Recommendation:** Keep the rationale, but move the longest blocks to a module-level doc comment (`//!`) or a `docs/decisions/` note (the repo already has `docs/decisions/` and `docs/educated-guesses/` — good practice), leaving a one-line pointer at the code site. This is a nit, not a problem; the comment culture overall is an asset.

---

### CLN-07 — Non-lock `.unwrap()` distribution  **[Info]**

461 non-lock `.unwrap()` calls exist across the crates. Sampling shows the majority are in `#[cfg(test)]` modules (where `.unwrap()` is idiomatic and fine) or on infallible conversions. A focused pass to confirm no *production* hot path unwraps on attacker- or user-influenced data is worthwhile, but this is lower priority than CLN-02 (which is the concentrated, user-facing subset).

**Recommendation:** Optionally add a clippy lint budget: `#![warn(clippy::unwrap_used)]` at the crate level for `rustic-agent`/`src-tauri` (with `#[allow]` on test modules) so new production unwraps are caught in review.

---

## Things checked and found clean (no action)

- **No stub code** (`todo!`/`unimplemented!` = 0).
- **No meaningful commented-out code** (~23 lines repo-wide).
- **Almost no debug logging** left in the frontend (6 `console.log/info/debug`).
- **Few dead-code suppressions** (8 `#[allow]`).
- **Decision docs exist** (`docs/decisions/`, `docs/educated-guesses/`, `docs/plans/`) — design rationale is captured outside code where it belongs.
- **Comment quality is high** — comments explain *why*, not *what*, and document real platform gotchas.

---

## Prioritized order

1. **CLN-01** — remove `Pasted text #1` from git (30 seconds, removes repo noise + stray context leak).
2. **CLN-02** — convert IPC-surface `lock().unwrap()` to graceful errors or `parking_lot` (real robustness win for a long-lived app).
3. CLN-04 / CLN-05 — delete the build log; de-hardcode the version.
4. CLN-03 — split god-modules opportunistically.
5. CLN-06 / CLN-07 — comment trims + a clippy lint budget.
