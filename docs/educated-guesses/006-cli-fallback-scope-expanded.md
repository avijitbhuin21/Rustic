# 006 — CLI fallback scope expanded for rustic-git

## Decision

Use the `git` CLI not just for merge/rebase (the original plan in doc 003)
but also for **state-mutating operations** where gix's API surface is
significantly lower-level than libgit2's. Specifically: status, stage,
unstage, commit, checkout, push, fetch, clone, pull, worktree add/remove.

Keep `gix` for pure read-side operations: refs, branches, log/revwalk,
commit metadata, tree diffs, blob reads.

## Why

When I started porting status.rs, I found that gix's status API is
significantly more involved than libgit2's flat bitflag approach:
- `repo.status(progress)?` returns a `Platform`
- You configure it via builder calls
- Then iterate `Item::TreeIndex(...)` (staged) and `Item::IndexWorktree(...)`
  (unstaged) variants
- Each variant has its own structured change payload

This is more correct than libgit2 long-term, but porting cleanly + handling
every edge case (renames, conflicts, submodules, mode changes) is a real
chunk of work.

Same pattern applies to:
- Commit creation: gix has it, but the author/committer signature path goes
  through `repo.committer().transpose()` returning `Option<SignatureRef>`
- Checkout: gix has primitives in `gix-worktree-state` but no equivalent of
  libgit2's `CheckoutBuilder`
- Index manipulation: gix has it but the API differs in non-obvious ways

Going through `git status --porcelain`, `git commit`, `git push`, `git
checkout`, etc. is:
- ✅ Pure correctness (it's literally real git's behaviour)
- ✅ Pure-Rust workspace (no `libgit2-sys`, just `gix` + subprocess calls)
- ✅ Sub-50ms overhead per call (acceptable for user-initiated VCS ops)
- ⚠️ Runtime requirement: `git` on PATH — already required for the
  merge/rebase fallback agreed in doc 003

## Trade-off accepted

- **Lose:** in-process speed for state-mutating operations (~10–50 ms
  subprocess overhead each). Imperceptible for one-off user actions like
  "commit" or "push"; would matter if we called these in a hot loop, which
  we don't.
- **Gain:** clean port without reimplementing libgit2's checkout/commit
  state machine on top of gix primitives. Estimated ~3 days saved.

## What stays in gix

These remain pure-Rust gix-native because gix's API for them is mature and
direct:

- `gix::discover`, `gix::init` — open/create repo
- `repo.head()`, head kind enum
- `repo.references()?.local_branches()` / `.remote_branches()` — branch list
- `repo.find_commit(oid)`, commit fields — log + commit metadata reading
- `repo.rev_walk()` — revwalk
- `repo.diff_tree_to_tree()` — diffs (already used by shadow store)
- `repo.find_remote()` — remote URL reading
- `repo.reference(...)` — direct ref updates (e.g. for soft reset)
- `repo.worktrees()` — list worktrees
- `repo.graph_ahead_behind` (or revwalk-based equivalent) — counts

## What goes through git CLI

- `status` — `git status --porcelain=v2`
- `stage` — `git add <paths>`
- `unstage` — `git restore --staged <paths>` (or `git reset HEAD <paths>`
  pre-2.23 fallback if needed)
- `commit` — `git commit -m <msg>`
- `discard_changes` — `git checkout -- <paths>`
- `checkout_branch` — `git checkout <name>`
- `fetch` — `git fetch origin` (with optional `-c http.extraHeader` for token)
- `push` — `git push origin <branch>`
- `pull` — `git pull origin <branch>` (which is fetch + merge)
- `clone` — `git clone <url> <path>`
- `publish_branch` — `git push --set-upstream origin <branch>`
- `rebase` (and `rebase_continue`, `rebase_abort`) — already in plan
- `merge_commit` — `git commit --no-edit` after manual resolution
- `worktree add/remove` — already in plan

## Authentication

git CLI handles all auth (SSH agent, credential helpers, HTTPS tokens) via
its own configured paths. We get this for free — no need to reimplement
`gix-credentials` callbacks.

For token-based auth (the previous `make_callbacks(token)` flow): we'll set
the token via the `-c http.extraHeader="Authorization: Bearer <token>"`
flag for one-off operations rather than baking it into git config.

## Risk if wrong

If you'd rather have native gix push/fetch (for, say, no-network-trace
auditability or for shipping to environments without git installed): the
remote-related operations can be converted to gix later. The conversion is
self-contained to `remote.rs` and won't affect the other modules.
