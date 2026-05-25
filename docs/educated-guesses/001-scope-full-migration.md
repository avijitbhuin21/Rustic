# 001 — Scope: full migration off libgit2

## Decision

Migrate **both** `file_history` (in `rustic-agent`) **and** `rustic-git` off
`git2` (libgit2 bindings) onto `gix` (gitoxide, pure Rust).

## Context

User asked whether to migrate just `file_history` or both crates. The answering
question was: "if `rustic-git` keeps `git2`, does the C dependency stay?"
Answer: **yes** — `git2` 0.19 pulls in `libgit2-sys` 0.17, which compiles
libgit2 from C. As long as any workspace member depends on `git2`, the C
toolchain requirement and FFI surface stays.

So the only way to deliver the actual user-visible benefit of the migration
("no C dependency") is to remove `git2` from every Cargo.toml in the workspace.

## Surface to port

### `crates/rustic-agent` (file_history)

- `file_history/shadow.rs` — heavy use. Tree storage, blob writes, tree diff,
  unified diff, restore, GC. ~600 LoC.
- `file_history/tracker.rs` — minor. Two `git2::Oid::from_str` calls only.

### `crates/rustic-git` (regular VCS features)

- `repo.rs` — `Repository`, branches, reset, worktree add/prune
- `status.rs` — `git status`, checkout (`CheckoutBuilder`)
- `log.rs` — commit log iteration, per-commit diff stats
- `diff.rs` — diff parsing helpers
- `remote.rs` — clone, fetch, push with SSH + HTTPS credentials  ⚠️ **RISK**
- `conflict.rs` — merge head handling                            ⚠️ **RISK**

## Risk assessment

`gix`'s API coverage is uneven by area:

| Area | `gix` status | Confidence |
|---|---|---|
| Bare repo init/open | mature | high |
| Blob/tree object I/O | mature | high |
| Tree diff (entry-level) | mature | high |
| Unified diff text rendering | mature (via `gix-diff`) | high |
| Index manipulation | usable but different shape vs libgit2 | medium |
| Worktree walk (`gix::status`) | mature | high |
| Branch ops | mature | high |
| Reset (soft/hard/mixed) | available | medium |
| Worktree add/prune | partial — `gix-worktree` exists but is lower-level | medium-low |
| Clone | works (`gix::prepare_clone`) | medium |
| Fetch/push with HTTPS | works | medium |
| Fetch/push with SSH credentials | works but credential-callback shape differs | medium-low |
| Merge / conflict markers | gix doesn't fully implement merge yet | **low** |

The `file_history` migration is high-confidence — every op used in `shadow.rs`
maps to a stable `gix` API.

The `rustic-git` migration has unknowns in **remote ops (SSH credentials)**,
**worktree add/prune**, and **merge conflict handling**. If any of these turn
out to be genuinely missing in `gix`, I'll document the gap and pick the least
bad workaround:

1. Keep `git2` for *just that operation* and document why — defeats the
   "no C dep" goal entirely; only acceptable if literally one op is missing
2. Spawn the `git` CLI for that operation — adds a runtime dep on git being
   on PATH, which the project doesn't currently require
3. Reimplement the missing piece on top of `gix`'s lower-level crates
   (`gix-protocol`, `gix-credentials`, etc.) — more work but no C dep, no
   runtime git dependency

Default to (3) where feasible, fall back to (2) where (3) would take days, only
fall back to (1) as a last resort with explicit user sign-off on return.

## Order of execution

1. `file_history` first (easy, high confidence, exercises the shadow store
   which is the highest-stakes piece for correctness)
2. `rustic-git` second, one module at a time, committing after each
3. Remove `git2` from both Cargo.tomls last (so partial state still builds)

## Risk if wrong

If the scope answer is wrong and you actually only wanted `file_history`:
revert the `rustic-git` commits via `git revert` — they'll be isolated. The
`file_history` commits stay valid either way.
