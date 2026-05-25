# 005 — notify integration: design doc for the next session

## What this is

A complete design for replacing the "walk the entire worktree on every bash"
strategy in [crates/rustic-agent/src/file_history/sweep.rs](../../crates/rustic-agent/src/file_history/sweep.rs)
with a filesystem watcher that tells us *exactly* which paths changed.

I did the recon but **wrote no code** — that's the next session's job. This
doc is meant to make that session executable in 1–2 days.

## The win, in numbers

Current sweep cost = O(worktree size). On a 50k-file repo with `ignore` crate
filtering, `walk_for_sweep` is ~150–250 ms. Sweep fires after every bash tool
invocation, so a model that runs `cargo check` then `cargo test` then `cargo
fmt` pays 3× that.

Target cost with FS watcher = O(actually-changed-files). Same 50k repo, 5
files edited → ~5 ms. **Best case: 30–50× faster sweep.** Worst case: same as
today (when we have to fall back to full walk on lost events).

There's also a UX win: the changed-files panel can update in real-time as
files change, not in a batch when the bash tool returns. Files appear
instantly, including external edits (user opens a file in another editor).

## Library choice: `notify-debouncer-full`

- `notify` 8.2.0 — raw FS events; needs custom coalescing
- `notify-debouncer-mini` — basic debouncing, no rename matching
- `notify-debouncer-full` ← **picked**

**Why full:** it handles three things we'd otherwise reinvent badly:

1. **Coalescing.** Editors generate 3–5 events per save (write to `.tmp`,
   rename, modified). full collapses them to one event per path per debounce
   window.
2. **Rename matching.** A `mv old.rs new.rs` is two events at the OS level
   (`Remove old.rs`, `Create new.rs`). full matches them via inode tracking
   and emits a single `Rename` event.
3. **Lost-event surfacing.** When the kernel buffer overflows (very real on
   Windows during `cargo build` writing thousands of files), `full` reports
   it as an error result so we can fall back to a full walk.

Cost: pulls in a small extra dep tree. Worth it.

## Architecture

```
   ┌──────────────────────────────────────────────────────────┐
   │  notify-debouncer-full                                   │
   │   - RecommendedWatcher (auto-picks per-OS backend)       │
   │   - 50–100 ms debounce window                            │
   │   - filters: .gitignore + HARD_DENY_DIRS (registration   │
   │     time, not per-event — see "Filtering" below)         │
   └──────────────────────┬───────────────────────────────────┘
                          │ DebouncedEvent { path, kind } / Err
                          ▼
   ┌──────────────────────────────────────────────────────────┐
   │  DirtyPathAccumulator (new, ~150 LoC)                    │
   │   - HashMap<(task_id, message_id), DirtySet>             │
   │   - DirtySet { paths: HashSet<PathBuf>, lost: bool }     │
   │   - on event: add path to active task/message            │
   │   - on lost-event error: set lost flag                   │
   └──────────────────────┬───────────────────────────────────┘
                          │ flushed by SweepWorker, or by
                          │ FileTracked event for edit-tool path
                          ▼
   ┌──────────────────────────────────────────────────────────┐
   │  SweepWorker (modified, ~50 LoC change)                  │
   │   - bash-tool-end enqueues SweepJob as today             │
   │   - on SweepJob: drain accumulator for that (task, msg)  │
   │   - if `lost` flag OR set is empty:                      │
   │        full walk via shadow.track() (current behaviour)  │
   │     else:                                                │
   │        targeted track via shadow.track_paths(paths)      │
   │        (new shadow method, see "Shadow changes" below)   │
   └──────────────────────────────────────────────────────────┘
```

### Who owns the watcher

One `RecommendedWatcher` per *project* (not per task, not per message). It
lives in the same place `FileHistoryHandle` already does — see
[src-tauri/src/commands/file_history.rs:143](../../src-tauri/src/commands/file_history.rs#L143)
where the per-project registry already exists. The watcher gets registered
when the project's FileHistory is created and torn down on project close.

The watcher's lifetime is tied to the project, but events get routed to the
*currently active* task+message. The accumulator needs to know which
`(task_id, message_id)` is "live" at any moment. Two options:

**Option 1: routing table inside the accumulator.** Tracker tells accumulator
"I'm opening snapshot X for task Y, message Z" — the accumulator routes
incoming events to that pair until the next `open_snapshot` call.

**Option 2: event broadcast.** Accumulator stores events globally with a
timestamp; SweepWorker queries `paths_dirty_since(bash_start)` when running
a sweep.

**Recommend Option 1.** Cleaner ownership, no time-window queries.
`SweepJob` already carries `task_id` and `message_id` — pass those into
`open_snapshot` so the accumulator's routing table updates atomically.

## Shadow store changes

`shadow.rs` needs a new method:

```rust
pub fn track_paths(&self, paths: &[String]) -> Result<TrackResult> {
    // Like track() but only re-reads + re-hashes the listed paths. The rest
    // of the tree is carried forward from the previous track() result.
    //
    // Implementation: open the previous tree via `find_tree(prev_tree_oid)`,
    // run `edit_tree` from there, upsert/remove only the listed paths.
    // ...
}
```

This is the surgical equivalent of the current full walk. It needs to know
the "previous" tree oid to seed the editor from — which we already store in
`file_history_snapshots.tree_oid` for the active message.

### Subtlety: deletions

Watcher events for deletions arrive as `EventKind::Remove(...)`. The dirty
set needs to track them separately from modifications so `track_paths` knows
to *remove* the entry from the tree rather than re-hash it (the file's not
on disk anymore).

Cleanest:

```rust
struct DirtySet {
    modified: HashSet<PathBuf>,  // upsert via shadow
    removed: HashSet<PathBuf>,   // remove from tree
    lost: bool,                  // → full walk
}
```

## Filtering

Two layers — same as the current sweep — but applied at the *watcher*
registration level, not per-event:

1. **HARD_DENY_DIRS** from
   [walk.rs:21](../../crates/rustic-agent/src/file_history/walk.rs#L21):
   `node_modules`, `target`, `dist`, `.git`, etc. Don't register these for
   watching at all. On Windows, watching `target/` recursively on a Rust
   project pegs the CPU and floods events during builds.

2. **.gitignore.** Tricky — gitignore is per-directory and dynamic. Options:
   - (a) Re-load gitignore rules on every event and filter — adds latency
   - (b) Watch everything, filter in the accumulator using the same `ignore`
     crate setup we use in `walk_for_sweep` — simpler, slight per-event cost
   - (c) Watch nothing inside hard-denied dirs; let everything else through
     and trust the next `track_paths` call to respect gitignore on read

   **Recommend (c).** The shadow store already respects gitignore on read; if
   we get a watcher event for a `.gitignored` file but never read it, no
   harm done. The accumulator just carries a slightly larger dirty set.

## Platform-specific gotchas

### Windows (ReadDirectoryChangesW)

- **Kernel buffer overflow.** Default buffer is 64 KiB; a `cargo build`
  writing 10,000 files generates many MB of events. Overflow → events
  silently lost. notify-debouncer-full surfaces this as a `notify::Error`
  with `kind == ErrorKind::WatchNotFound` or similar — the accumulator
  catches it, sets the `lost` flag, and the next sweep does a full walk.
- **Long paths.** Paths longer than 260 chars need the `\\?\` prefix.
  notify 8.x handles this internally.
- **Junction/symlink behaviour.** Recursive watch doesn't follow symlinks
  consistently. Not a problem for us — we don't watch worktrees with
  external symlink targets anyway.

### Linux (inotify)

- **Watch limit.** `/proc/sys/fs/inotify/max_user_watches` defaults to ~8192
  on most distros. A monorepo with 50k+ directories blows past it and
  `RecommendedWatcher::watch` returns `Error::Generic` (or similar).
- **Mitigation:** on `ENOSPC`-class errors, log a clear warning telling the
  user how to raise the limit (`echo fs.inotify.max_user_watches=524288 |
  sudo tee /etc/sysctl.d/40-watches.conf && sudo sysctl --system`) and
  fall back to PollWatcher for that project. JetBrains does exactly this.
- **fanotify alternative.** Requires CAP_SYS_ADMIN — non-starter for a
  user-space app.

### macOS (FSEvents)

- **Mostly fine.** FSEvents handles large worktrees gracefully.
- **Caveat:** FSEvents coalesces events at the directory level, not the
  file level. notify normalizes this but with slightly less precise paths
  on some bursts. Acceptable for our use.
- **Sandboxed builds.** If we ever ship through the Mac App Store, FSEvents
  needs specific entitlements. Not a near-term concern.

### Network filesystems (SMB, WSL `\\wsl$\`, network mounts)

- Native watchers don't work on most non-local filesystems.
- notify provides `PollWatcher` as a fallback — polls stats on a configurable
  interval (default 30s). Use it when the native watcher fails to register.
- Detect at startup: try `RecommendedWatcher` first; on `Error::Generic`
  with "operation not supported" or similar, fall back to `PollWatcher`
  with a 2s interval (more aggressive than its default because users notice
  30s lag).

## Editor save patterns

Most modern editors save by:

1. Write to `<filename>.<editor>-tmp-<random>`
2. Rename `<tmp>` → `<filename>` (atomic on POSIX, near-atomic on Windows)

This shows up as: `Create(.tmp) + Modify(.tmp) + Remove(.tmp) + Create(target)`
or sometimes `Create(.tmp) + Rename(.tmp → target)`.

`notify-debouncer-full` already collapses these to a single `DebouncedEvent`
on the final target path, so we don't have to write the matching ourselves.
This is one of the main reasons to use full over mini.

**Catch:** some editors (vim with `backupcopy=no`) write to a backup, delete
the original, rename backup. This produces `Remove(original) + Create(target)`
with no rename event. full handles it via inode tracking *if* the editor
preserves the inode — vim usually does, VSCode usually doesn't. The fallback
is just two events (one Remove, one Create) and the accumulator handles
both correctly: the path is in the dirty set either way.

## Lost-event handling

The single most important correctness invariant: if the watcher loses an
event, we *must* fall back to a full walk. Otherwise we'd report a stale
"no changes" sweep and the changed-files panel would lie.

```rust
match result {
    Ok(events) => for ev in events { accumulator.record(ev); }
    Err(errors) => {
        for e in errors {
            tracing::warn!(?e, "fs watcher dropped events");
        }
        accumulator.mark_lost();  // forces next sweep to full walk
    }
}
```

The `lost` flag is per-(task, message). Once set, it sticks until the next
`open_snapshot` for that pair (which does a full walk anyway).

## Lifecycle

```
project open
  → FileHistoryHandle::new(project_root)
    → spawn watcher (notify-debouncer-full)
    → watcher.watch(project_root, RecursiveMode::Recursive)
    → exclude HARD_DENY_DIRS via .ignore-style filter
    → events → accumulator

user starts task / sends message
  → tracker.open_snapshot(task_id, message_id)
    → accumulator.set_active(task_id, message_id)
    → shadow.track() (full walk; this is the baseline)

bash tool runs
  → terminal.rs enqueues SweepJob
  → SweepWorker picks it up, debounces 50ms
  → on flush: read accumulator.drain(task_id, message_id)
              if drained.lost or drained.empty → shadow.track() (full)
              else → shadow.track_paths(drained.modified, drained.removed)

edit tool runs (Write/Edit/NotebookEdit)
  → file_ops.rs calls history.capture()
  → capture stays synchronous (it's a UI hook, see today's behaviour)
  → BUT: the watcher will also see the write, generating an event that
    lands in the accumulator. That's fine — the next sweep will track it
    via track_paths; the edit-tool capture and watcher event are
    idempotent w.r.t. the shadow.

project close
  → FileHistoryHandle dropped
  → watcher dropped (notify cleans up backends automatically)
```

## What does NOT change

- Public API of `FileHistory` — same methods, same signatures
- Public API of `ShadowSnapshot` — same, plus the one new `track_paths`
- Per-message snapshot model — still tree oids in SQLite
- Retention caps — unchanged
- `FileTracked` event semantics — unchanged
- Tests — existing 56 file_history tests must still pass

The watcher is purely an *acceleration* of the sweep, not a replacement for
any existing concept.

## Implementation order (next session, ~1–2 days)

1. **Add deps:** `notify = "8"`, `notify-debouncer-full = "0.7"`
2. **Add `shadow.rs::track_paths(modified, removed)`** + tests. ~150 LoC.
   This is independent of the watcher; can be merged first.
3. **Build `DirtyPathAccumulator`** in a new module
   `file_history/accumulator.rs`. Pure data structure + small API. ~200 LoC
   incl. tests.
4. **Build `FileWatcher`** wrapper that owns the debouncer and feeds the
   accumulator. New module `file_history/watcher.rs`. Handles registration,
   filtering, lost-event flag, platform fallback. ~300 LoC.
5. **Wire into `FileHistoryHandle`** in `src-tauri/src/commands/file_history.rs`.
   Spawn watcher when handle is created.
6. **Modify `SweepWorker`** to call `track_paths` when the accumulator has
   a non-empty, non-lost set; full walk otherwise. ~50 LoC delta.
7. **Modify `tracker.rs::open_snapshot`** to call
   `accumulator.set_active(task_id, message_id)`. Tiny change.
8. **Tests:**
   - Unit: accumulator behaviour with mocked events
   - Integration: real watcher + tempdir + file mutations
   - Stress: Windows kernel buffer overflow simulation (write 10k files,
     verify lost flag → full walk → consistent shadow state)
   - Linux: simulate `ENOSPC` on watch registration, verify PollWatcher
     fallback
9. **Performance test:** existing
   [tracker.rs:1205](../../crates/rustic-agent/src/file_history/tracker.rs#L1205)
   has a "track_thousand_small_files_in_under_5s" test. Add a sibling that
   exercises `track_paths` on a small subset of a large worktree and asserts
   it's 10× faster.

## Things I'm not sure about — open for review

1. **Should `open_snapshot` always do a full walk, or trust the accumulator
   if the previous message has a known tree oid?**
   First message in a task: definitely full walk. Subsequent messages:
   probably also full walk to be safe (cheap insurance vs. accumulated
   bugs). But this is a design tradeoff worth discussing.

2. **PollWatcher fallback interval.** Default 30s is too slow for our use.
   2s feels right. Want your call.

3. **Should we expose the lost-event signal in the UI?** A small badge
   "watcher fell behind, files may be stale" when `lost` is set, until the
   next sweep recovers. Maybe overkill — the sweep recovers automatically
   within ~50 ms. Suggest: no, don't surface it. Just log.

4. **Cleanup test for Linux inotify limit.** Hard to test in CI without
   sudo. Maybe gate behind an env var (`RUSTIC_TEST_INOTIFY_LIMIT=1`)
   that callers set explicitly.

## Risk if wrong

`notify-debouncer-full` is well-trodden and used by many production Rust
projects. The watcher itself is unlikely to be the source of bugs.

The accumulator's routing table (which (task, message) is "active") is the
subtle bit. If we route an event to the wrong message, the changed-files
panel for that message gets a path it shouldn't. Mitigated by:
- Accumulator is per-project, not global → no cross-project leakage
- Routing changes atomically at `open_snapshot` time → no in-flight events
  land on a stale routing target
- The eventual `track_paths` reads the actual disk state → even if a path
  is over-attributed, the diff against the previous tree is still correct

Worst case fix: if integration testing shows the routing is racey, fall back
to a time-window query (Option 2 from the architecture section).

## Estimated effort

- Design (this doc): ✅ done
- Implementation: **1–2 focused days**
- Testing (unit + integration + stress): **0.5 days**
- Polish + UI wiring: **0.5 days**

**Total: ~3 days for a confident merge.** Less if we accept the open
questions as-is without bikeshedding.

Sources for design:
- [notify docs](https://docs.rs/notify/latest/notify/)
- [notify-debouncer-full docs](https://docs.rs/notify-debouncer-full/latest/notify_debouncer_full/)
- Current sweep implementation: [sweep.rs](../../crates/rustic-agent/src/file_history/sweep.rs)
- Current walk implementation: [walk.rs](../../crates/rustic-agent/src/file_history/walk.rs)
