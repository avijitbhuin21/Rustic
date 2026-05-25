# 004 — Why I stopped before porting rustic-git

## Decision

Pause the rustic-git migration after writing the strategy doc (003) and the
gix dep scaffolding. Do **not** start porting the modules in this session.

## Context

The migration plan from doc 001 was: file_history first, then rustic-git.
file_history is done — 56 tests passing on gix. rustic-git is the next item.

After cataloguing rustic-git in detail (repo.rs / status.rs / diff.rs /
log.rs / remote.rs / conflict.rs — 1311 LoC total) and recon-ing gix's API
against every call site, I concluded the port is **larger and higher-risk
than I can complete safely in the remaining ~5 hours of this session**.

## Why this is the right call

The file_history port had three things going for it that rustic-git doesn't:

1. **Self-contained surface.** file_history was one module (`shadow.rs`)
   with one consumer (`tracker.rs`). rustic-git has 6 interdependent modules
   that all impl methods on the same `GitRepo` type. Porting any one of them
   requires the others to be ported simultaneously, so there's no safe
   intermediate state to commit.

2. **Confined to a single gix subset.** file_history only needed object I/O
   (write_blob, find_tree, find_blob, edit_tree, diff_tree_to_tree).
   rustic-git additionally needs: revwalk, branches, references, status,
   index manipulation, commit creation, checkout, reset, ahead/behind,
   remote ops (fetch/push/clone), credentials (SSH + HTTPS), merge,
   rebase. Each API has its own quirks vs libgit2, and several aren't
   implemented in gix at all (merge, rebase).

3. **Internal scaffolding, not user-visible flows.** file_history shows up
   only in the changed-files panel and revert. A subtle bug shows up as a
   weird diff. rustic-git is the *git pane*: a subtly broken push, pull,
   merge, or rebase will silently corrupt the user's repo or lose work.
   The blast radius is much higher.

The "spawn git CLI for merge/rebase" plan from doc 003 is sound, but
implementing the rest correctly (push/fetch with credential helpers, SSH
agent integration, worktree add/prune, the merge_analysis state machine)
is multi-day work even before testing.

I cannot ask clarifying questions in your absence. Shipping half-tested VCS
plumbing that touches user repos is exactly the class of risk the
educated-guesses framework is designed to surface, not to barrel through.

## What I'm doing with the remaining time instead

Switching to the **notify integration recon** (the originally-planned next
phase after the gix migration). This is:
- Lower-risk (pure additive work, doesn't touch existing functionality)
- High-confidence (the `notify` crate is mature; design is straightforward)
- Useful to have done before you return regardless of what you decide
  about rustic-git

Recon deliverable: a design doc in `docs/educated-guesses/` covering the
watcher architecture, lost-event handling, platform-specific gotchas, and a
proposed integration shape with the existing `SweepWorker`. No code changes
to the sweep yet — just the design.

## Options for rustic-git, for your review on return

When you're back, three sane paths:

### A. Do the full rustic-git port in a focused 2–3 day session
- I do the work with you available to clarify
- We test against real repos (push to a real remote, merge a real branch)
- Final result: no C dep in the workspace
- This is what doc 001 + 003 design

### B. Keep rustic-git on git2 indefinitely
- The libgit2-sys C compile stays in the workspace
- file_history stays on gix (cleaner, faster, better-tested)
- We give up the "no C dependency" goal but at low cost — libgit2 is one
  of the most stable C libraries we could possibly depend on
- Zero further work

### C. Hybrid: port the easy modules (status, diff, log), keep git2 for the
hard ones (remote, conflict, merge/rebase parts of repo.rs)
- Half-progress; libgit2-sys still in workspace
- Slight code-quality win from gix's nicer APIs in the easy modules
- Net cost: 1 day of work for partial benefit

My recommendation: **(A) when you can give it 2–3 focused days**, or **(B)
if you decide the C dep isn't worth the engineering time**. Avoid (C) —
half-migrations have ongoing maintenance cost without delivering the goal.

## State of the working tree right now

- `crates/rustic-agent/Cargo.toml` — git2 removed, gix 0.83 added
- `crates/rustic-agent/src/file_history/shadow.rs` — fully on gix
- `crates/rustic-agent/src/file_history/tracker.rs` — Oid type updated
- `crates/rustic-agent/src/file_history/mod.rs` — re-exports Oid
- `crates/rustic-git/Cargo.toml` — git2 still there, gix added (will get
  used when the rustic-git port happens)
- `crates/rustic-git/src/**` — **untouched, still using git2**
- `Cargo.lock` — has both git2 and gix as direct deps

The workspace builds. All 307 rustic-agent tests pass. rustic-git is
unchanged from before this session.

## How to undo the rustic-git Cargo.toml change

If you don't want gix in rustic-git's deps until the port actually happens:

```
git diff crates/rustic-git/Cargo.toml  # see the gix line
# revert that one line manually, or:
git checkout HEAD~3 -- crates/rustic-git/Cargo.toml
```

Or just leave it — having gix in the deps doesn't hurt anything, it's just
unused until the port lands.
