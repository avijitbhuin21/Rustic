# File-tracking architecture: decision

**Status:** Draft v1, 2026-05-13.
**Question:** Should Rustic replace its current `file_history` system with a shadow-git system modelled on CLAURST's, keep the current system, or go hybrid?
**TL;DR recommendation:** **Hybrid.** Replace the storage core with libgit2-backed shadow trees; keep our SQLite layer of `(task_id, message_id) → tree_hash` metadata on top.

---

## 1. What we currently have

Source: [crates/rustic-agent/src/file_history/](../crates/rustic-agent/src/file_history/).

- **Storage:** SQLite index (`snapshots`/`files`/`blobs` tables in `rustic-db`) + content-addressed blob store at `{configDir}/file-history/blobs/{hash[:2]}/{hash}`.
- **Hashing:** Streaming SHA-256. File-size cap **5 MiB**; oversized files recorded with a `TooLarge` marker (unrevertable).
- **Capture path (synchronous):** Edit / Write / NotebookEdit tools call `tracker::capture(message_id, abs_path)` **before** mutating a file. ~5 ms on the agent path. Returns `CaptureOutcome::{Captured, DidNotExist, AlreadyTracked, TooLarge}`.
- **Sweep path (background):** Bash tools push `SweepJob` onto an mpsc channel. A single-consumer worker drains it, coalesces bursts within a 50 ms debounce window, and runs each sweep on `spawn_blocking`. The sweep walks the worktree and records changes vs. the snapshot.
- **Scoping:** Per-task (`task_id`), per-message (`message_id`). Snapshots are sequenced; capping at `max_snapshots = 100` per task with FIFO eviction. Trigger-driven blob refcounting for GC.
- **Cumulative view:** `TaskNetChange` computes "what did this task change overall" by diffing the **earliest** snapshot's stored blob against current disk content.
- **Revert:** Plan preview (`RevertPlanEntry`) showing per-path actions (`restore` / `delete` / `keep`), then apply.

## 2. What CLAURST has

Source: [references/claurst/src-rust/crates/core/src/snapshot/](../references/claurst/src-rust/crates/core/src/snapshot/).

- **Storage:** A **shadow bare git repository** stored *outside* the user's repo at `~/.claurst/data/snapshot/<project_hash>/<worktree_hash>/`. The user's actual worktree is reused via `GIT_WORK_TREE`; the shadow gitdir gets `GIT_DIR`. **No commits**, just tree objects.
- **Hashing:** Whatever git uses internally (SHA-1, content-addressed via packfiles with delta compression). File-size cap **2 MiB**; oversized untracked files are added to the shadow's `info/exclude`.
- **Track operation:** `track()` stages all tracked + untracked files (respecting user's gitignore + `info/exclude`), calls `git write-tree`, returns the **tree hash** as the checkpoint id.
- **Operations** (all serialised via `Arc<Mutex<()>>` per shadow repo):
  - `patch(hash)` → files changed since `hash`
  - `diff(hash)` → unified diff vs `hash`
  - `diff_full(from, to)` → per-file `FileDiff` (status + line counts + patch text)
  - `restore(hash)` → restore *entire worktree* to that tree via `read-tree` + `checkout-index`
  - `revert(patches)` → restore individual files (batched per-tree via `ls-tree` + `checkout`)
  - `cleanup()` → `git gc --prune=7.days`
- **Scoping:** None at the snapshot layer — pure tree hashes. Per-session callers track the tree hash they care about as application state.
- **Lifecycle:** Process-global registry keyed by canonical worktree path → `Arc<ShadowSnapshot>`.
- **Dependency:** External `git` binary on PATH (spawned via `tokio::process::Command`). Degrades gracefully (`for_session` returns `None`) when git is missing or the directory isn't in a git repo.

## 3. Head-to-head comparison

| Dimension | Rustic `file_history` | CLAURST shadow-git |
|---|---|---|
| Storage model | Custom blob store, one blob per (path, content) | Git tree objects + packfiles with delta compression |
| External dependency | None — pure Rust | `git` binary on PATH (or libgit2 if we adopt) |
| Works on non-git projects | Yes | **No** — worktree must be inside a git repo |
| Hash algorithm | SHA-256 streaming | SHA-1 (git internal) |
| Per-file size cap | 5 MiB | 2 MiB |
| Disk efficiency for repeated edits | **Poor** — N edits of the same file = N full blobs | **Excellent** — delta compression in packfiles dedupes near-identical content |
| Concurrency | SQLite Mutex; per-file blob writes atomic via rename | Single async Mutex per shadow repo; ops serialised |
| Per-task/per-message metadata | **Built in** (task_id, message_id, sequence) | **Absent** — just tree hashes; caller must store metadata separately |
| Pre-mutation capture guarantee | Yes — synchronous capture before write | No — `track()` is called after the fact; relies on next track() catching the change |
| Out-of-band edit detection (e.g. `cargo build`) | Yes — bash-triggered sweep | Yes — next `track()` will see whatever's on disk |
| Symlinks / CRLF / case-sensitive paths / submodules | Custom handling (incomplete) | Inherited from git (battle-tested) |
| .gitignore handling | None (we track everything in the worktree, large files excluded) | Respects user .gitignore + shadow `info/exclude` for oversized |
| Inspectability | Need to query SQLite | `cd ~/.claurst/data/snapshot/...` and use `git log`/`git show`/`git diff` directly |
| Cleanup / GC | DB-trigger refcounting (custom, bug-prone) | `git gc --prune=7.days` (battle-tested) |
| Subprocess overhead per op | None | 10–50 ms per `git` invocation on Windows |
| Implementation surface (Rust) | ~5 files, ~1500+ LoC, all ours to maintain | ~3 files, ~600 LoC, most logic delegated to git |

## 4. Likely sources of the current bugs

The user reports "a lot of bugs and glitches in file references" but doesn't have specific repros documented. The bugs are most plausibly in the **core mechanics**, not the agent-layer scoping. Best candidates:

1. **Concurrent task interference.** With 3–4 tasks running in the same project, the file_history isn't worktree-isolated. Two tasks editing the same file race on capture vs mutation; the "pre-state" captured by task B can be task A's partially-written content if timing is unlucky. Shadow-git wouldn't fix this directly (single Mutex still serialises) but the symptoms would be different and easier to spot in `git log` of the shadow repo.

2. **Sweep-vs-capture races.** The bash sweep walks the tree on a `spawn_blocking` thread. If another tool is mid-write when the walk hits that file, the sweep captures partial content as the "current state." Subsequent operations then diff against this corrupted record. CLAURST has this problem too but their Mutex covers more ground.

3. **Blob refcounting bugs.** Trigger-driven GC is hard. The most common bug: pruning a blob still referenced by a snapshot in another task because the trigger's join was wrong. Git's reachability-based GC is bulletproof here.

4. **Path canonicalization on Windows.** Same logical file under `C:\Projects\Foo\src\a.rs` and `c:\projects\foo\src\a.rs` (case) or via a junction/symlink can produce two `rel_path` rows. Git deals with this via `core.ignorecase` and normalization on its side.

5. **Snapshot eviction at `max_snapshots = 100`.** If a revert-preview is open when the 101st snapshot is created, the snapshot referenced by the preview can get evicted mid-flow.

6. **5 MiB cap is too low for some real files.** Lock files (`Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`) regularly exceed 5 MiB. Edits to those become unrevertable. Git's 2 MiB applies only to **untracked** files; tracked files have no such limit.

7. **SQLite + blob store consistency.** Two storage systems must stay in sync. A crash mid-write can leave either an index row pointing to a missing blob, or an orphan blob with no index row. Git's "object + index in one place" makes this impossible.

Most of these bugs disappear or transform into well-understood git bugs if we adopt shadow-git.

## 5. Three options

### Option A — Keep current, fix bugs in place

**Pro:** No external dependency. Already works on non-git projects. Per-task/per-message metadata is rich and useful.
**Con:** Custom code surface keeps growing. Each bug class above needs its own fix. Disk inefficiency is structural — won't improve. Edge cases (symlinks, line endings, case-sensitivity, submodules) keep biting.
**Effort:** Ongoing. Estimated 1–2 weeks to fix the seven bug classes listed, with the risk of new ones emerging.

### Option B — Full migration to CLAURST-style shadow-git

**Pro:** Inherits 20 years of git battle-testing. Pack-file disk efficiency. Trivial cleanup (`git gc`). Inspectable with stock git tools.
**Con:** **Loses our per-task/per-message scoping** — tree hashes have no notion of "which task changed this." We'd have to lift that into a side table anyway. External git dependency. Doesn't work on non-git projects. Subprocess overhead per op (10–50 ms).
**Effort:** ~1 week implementation + ~2 days migration of existing user data.

### Option C — Hybrid: libgit2 shadow trees + our SQLite metadata layer

**Pro:** Best of both:
- Battle-tested git internals (delta compression, gitignore, symlinks, CRLF, etc.) for the **storage** layer
- Our per-task/per-message scoping preserved as a thin metadata table that just maps `(task_id, message_id) → tree_hash`
- **No subprocess overhead** — use [`git2` crate](https://crates.io/crates/git2) (Rust binding for libgit2) so ops are in-process function calls, not 10–50 ms `git` invocations
- Inspectable: `git2`-created repos are just normal git repos; user can still shell out for debugging
- Pack-file disk savings are real — a 1 MB file edited 50 times goes from ~50 MB in current system to ~1.5 MB

**Con:** libgit2 adds ~2 MB to the binary. Doesn't natively support non-git projects (but we can init the shadow gitdir against *any* directory — see Section 6.3).
**Effort:** ~1.5 weeks total. See plan in Section 6.

## 6. Recommendation: Option C (hybrid)

### 6.1 New architecture

```
crates/rustic-agent/src/file_history/
  shadow.rs         — libgit2-backed shadow trees (new)
  registry.rs       — process-global Arc<ShadowSnapshot> per worktree (new)
  metadata.rs       — SQLite layer: (task_id, message_id) → tree_hash (refactored from tracker.rs)
  tracker.rs        — high-level API; orchestrates shadow + metadata
  walk.rs           — keep for sweep-driven out-of-band detection
  sweep.rs          — keep, just calls shadow.track() instead of custom capture
```

### 6.2 What stays the same (public API)

The trait surface that edit tools and Bash use stays nearly identical:

- `FileHistory::capture(task_id, message_id, path)` → still synchronous, still ~5 ms
- `FileHistory::open_snapshot(task_id, message_id)` → still anchored to messages
- `RevertPlanEntry`, `FileChangeStats`, `TaskNetChange`, `FileDiff` → all preserved
- Sweep worker contract unchanged

What changes is the **implementation**: instead of streaming SHA-256 → blob file → SQLite row, we call `repo.index().add_path(rel) + write_tree()` and store the resulting tree hash in our metadata table.

### 6.3 Non-git projects

CLAURST's shadow snapshot requires the worktree to be inside a git repo (their `find_repo_root`). We don't have to inherit that limitation. **libgit2 can manage an external gitdir pointing at any directory** — we don't need a `.git/` inside the user's project. We just need to skip the gitignore-of-user-repo step in those cases. So:

- **In a git repo:** Shadow repo at `{configDir}/file-history/shadow/{project_hash}/`; respects user's `.gitignore`.
- **Not in a git repo:** Same shadow repo location; falls back to a default exclude list (`target/`, `node_modules/`, `dist/`, `.next/`, `__pycache__/`, etc.) and no user gitignore.

### 6.4 Concurrency

Keep CLAURST's per-shadow-repo Mutex pattern. With our `WorkspaceServices` design (already planned as P1.3), the shadow snapshot can be owned by `WorkspaceServices` and shared across concurrent tasks in the same project — same as the symbol index. Single-writer-many-readers because tree writes are short.

### 6.5 Per-file size limit

Raise to **50 MiB** for tracked files (lockfiles, generated bundles, large notebooks all fit). libgit2 handles this fine. Keep the 2 MiB cap for **untracked** files (CLAURST's policy) so we don't shadow huge generated artefacts.

### 6.6 Migration of existing user data

Existing file_history blobs and snapshots are user-visible only via revert-preview. Options:

- **Clean break:** existing tasks lose revert ability for changes made before the migration. New tasks work with the new system. Acceptable since this is pre-1.0.
- **Best-effort import:** Walk the existing SQLite, replay each snapshot into a shadow repo as a tree. ~1 day extra. Worth doing if any user has serious in-flight work.

Recommend clean break, with a one-time UI notice on first launch after upgrade.

### 6.7 Risk register

| Risk | Mitigation |
|---|---|
| libgit2 binary-size bloat (~2 MB) | Acceptable; we already ship Tauri WebView (~80 MB) |
| Cross-platform path edge cases | libgit2 handles these on every major OS; CLAURST users report no issues |
| Performance on huge projects (chromium-scale) | libgit2 is what GitHub Desktop uses on 1M-file repos. Not a real concern at our scale. |
| In-process git crashes us | libgit2 returns `Result` from every fallible op; no native panics |
| Multiple Rustic instances on the same shadow repo | Same Mutex-per-canonical-worktree pattern as CLAURST; the second instance shares the Arc |

## 7. Implementation plan (1.5 weeks)

| Day | Work |
|---|---|
| 1 | Add `git2` dep. Create `shadow.rs` with `ShadowSnapshot::for_worktree`, `track()`, `patch(hash)`, `diff(from, to)`. Unit tests against tempdir. |
| 2 | Add `restore(hash)` and `revert(patches)`. Test with concurrent reads. |
| 3 | Refactor `tracker.rs` to delegate storage to `shadow.rs`. Keep SQLite metadata table for `(task_id, message_id) → tree_hash`. Update `CaptureOutcome` mapping. |
| 4 | Wire sweep worker to call `shadow.track()` instead of `apply_sweep`. Burst coalescing logic preserved. |
| 5 | Refactor `FileChangeStats`, `TaskNetChange`, `FileDiff` to be computed from `shadow.diff_full()` instead of blob comparisons. |
| 6 | Migration UI notice + best-effort import path for existing tasks (optional). |
| 7 | Integration test: spin up 3 concurrent tasks editing overlapping files; assert revert correctness. Run against a 50k-file repo for perf. |
| 8 | Dogfood on 2–3 of the user's real projects. Fix surfaced edge cases. |

## 8. Decision criteria

Adopt Option C if:
- ✅ User reports current bugs are slowing daily work (they do)
- ✅ Disk usage from file_history is noticeable (likely — no delta compression)
- ✅ Adding libgit2 dependency is acceptable (yes — Tauri already bundles much more)

Reconsider if:
- ❌ A platform we ship to lacks libgit2 support (none currently — Windows, macOS, Linux all supported)
- ❌ We need to track files >50 MiB (we don't — those are build artefacts)
- ❌ The user has so much in-flight task state that a clean-break migration is unacceptable (verify before starting)

## 9. Open questions for the user

1. **Migrate or clean-break?** Recommend clean-break with a one-time notice. Are there active tasks whose revert-history you'd be sad to lose?
2. **Default exclude list for non-git projects?** Recommend `target/`, `node_modules/`, `dist/`, `.next/`, `build/`, `out/`, `__pycache__/`, `.venv/`, `.cargo/`, `.gradle/`. Add anything specific to your workflow?
3. **Per-file size cap for tracked files** — recommend 50 MiB. Comfortable with that?
4. **GC schedule** — CLAURST uses `git gc --prune=7.days`. Sensible default; want it tunable in Settings?
