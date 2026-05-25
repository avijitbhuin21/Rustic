# 002 — Port style: idiomatic gix rewrite

## Decision

Rewrite each module using `gix`'s natural patterns rather than mechanically
substituting `gix` calls for `git2` calls one-for-one.

## Context

User picked "idiomatic gix rewrite". Recording the consequences here so the
review knows what changed shape vs what only changed crate names.

## Concrete shape changes

### Tree construction (shadow.rs `track()`)

**libgit2 today:** uses the bare repo's `Index` as a tree builder. Each file:
`index.add_frombuffer(IndexEntry, content)` writes a blob + adds an index entry.
Then `index.write_tree()` produces the tree oid.

**gix idiomatic:** no index involved. For each file: `repo.write_blob(content)`
returns the blob oid directly. Trees are built explicitly via
`gix_object::Tree { entries: Vec<tree::Entry> }` and persisted with
`repo.write_object(tree)`. Subtrees built bottom-up (a tree references its
direct children's oids; nested paths require building inner trees first).

Why this is better: no synthetic index step (libgit2 only synthesises one for
bare repos as a workaround), no in-memory `IndexEntry` struct with 12 zeroed
fields, deterministic across platforms by construction (no mtime/ctime fields
to ignore).

### Tree diff

**libgit2 today:** `repo.diff_tree_to_tree(from, to, opts)` returns a
`git2::Diff` whose deltas you iterate, then `Patch::from_diff` per index for
per-file unified diff + line stats.

**gix idiomatic:** `gix::diff::tree_with_rewrites(from, to, ..)` or
`gix::diff::tree(from, to, ..)` for structural diffs; `gix::diff::blob` for
unified text diffs. Binary detection lives on the blob side. Two passes vs
libgit2's combined object, but each pass is faster and the API is more typed.

### Index/checkout (rustic-git status.rs)

**libgit2:** `git2::build::CheckoutBuilder` with options + `repo.checkout_index`.

**gix:** `gix::worktree::state::checkout` against an in-memory index. Slightly
lower-level — you build the index from a tree, then check it out. The
options surface is different (force/safe is encoded differently).

### Status (rustic-git status.rs)

**libgit2:** `git2::StatusOptions` + `repo.statuses(Some(&mut opts))`.

**gix:** `repo.status(progress)` returns an iterator. The iterator yields typed
entries (added/modified/deleted/etc.) directly, no `Status` bitflag decoding.

### Reset (rustic-git repo.rs)

**libgit2:** `repo.reset(target, ResetType::Soft|Mixed|Hard, None)`.

**gix:** No single `reset` method. Each reset mode is built from primitives:
- Soft: just move HEAD (`repo.reference("HEAD")` → `set_target`)
- Mixed: soft + rewrite index to target's tree
- Hard: mixed + checkout the new index

I'll wrap this in a `git2::ResetType` shim so callers don't change.

## Why not 1:1 mirror

A 1:1 mirror produces a `shadow.rs` that calls `gix` functions but is
structured around `libgit2`'s idioms (an in-memory index that exists only to
synthesise a tree). It would compile but read like a bad translation, and would
hit edge cases where the libgit2 idiom relies on behaviour `gix` doesn't
replicate (specifically: the synthetic index for bare repos).

The idiomatic version is a bigger code diff but converges on `gix`'s natural
shape, which is what the swap is *for*.

## Behavioural compatibility commitment

Public API of each crate **must not change** during this migration:

- `ShadowSnapshot::for_worktree`, `track`, `patch`, `diff`, `diff_paths`,
  `diff_full`, `unified_diff_for_path`, `read_path`, `list_paths`,
  `restore_path`, `restore_paths_from`, `cleanup` — same signatures, same
  semantics, same error types (just the inner `Git` variant wraps a different
  error type).
- `Oid` type stays exposed; if the user code refers to it I'll re-export
  `gix_hash::ObjectId` aliased as `Oid` so callers don't change.
- All existing tests must pass. New tests welcome; existing tests must stay.

## Risk if wrong

If "idiomatic" turns out to mean "too different from before to safely review":
each commit is scoped to one module, so individual commits can be reverted and
re-done in mirror style without losing the others.
