# 003 — rustic-git: how to handle gix's missing merge/rebase

## Decision

For the rustic-git migration, port **everything possible** to gix; for the
specific operations that gix does not yet implement (merge, rebase), **spawn
the `git` CLI as a subprocess**. This is the only realistic path that keeps
the workspace free of libgit2-sys (the original goal) while also delivering
correctness on merge/rebase in the timeframe available.

## Context

After cataloguing the full rustic-git surface (1311 LoC across 6 files), the
operations fall into three buckets:

### Cleanly available in gix (≈ 90% of surface)

- Repository open/init/discover
- HEAD / branch reading + enumeration
- revwalk / log
- commit reading (metadata + tree)
- diff_tree_to_tree, diff_tree_to_index, diff_index_to_workdir
- status (working tree vs index, index vs HEAD)
- index manipulation (add_path, remove_path, write, write_tree)
- commit creation (single-parent and merge-parent)
- checkout (worktree-state checkout)
- soft reset
- ahead_behind (via revwalk)
- find_reference / set_target / create reference
- clone (`gix::prepare_clone`)
- fetch with credentials (`gix-credentials`)
- push with credentials (newer in gix, but available in 0.83)
- worktree listing (`repo.worktrees()`)

### Available but lower-level — wrappers needed (≈ 5%)

- Worktree add (`gix-worktree` has primitives, `Repository::worktree(...)` does
  not. Build it from: create branch ref → create `.git/worktrees/<name>` admin
  files → checkout that branch into the target dir.)
- Worktree prune (`gix-worktree`'s prune is lower-level; safe to write our own
  removal logic over `gix::worktree::Proxy::prune` plus filesystem cleanup.)

### NOT available in gix as of 0.83 (≈ 5%)

- `merge` (any flavour — fast-forward is buildable, three-way is not)
- `merge_analysis` (FF / up-to-date / normal classification)
- `rebase` (start / next-op / commit / finish / abort)
- `cleanup_state` (removes MERGE_HEAD / REBASE_HEAD marker files)

These aren't on gix's near-term roadmap as production-ready. Attempting to
implement three-way merge on top of `gix-merge` or `gix-diff` primitives is a
multi-week project with high correctness risk (we'd be reimplementing what
real git already does).

## Options considered

### A. Stop the migration at file_history, leave rustic-git on git2

Pros: zero risk in user's absence; no questionable judgment calls land.
Cons: libgit2-sys stays in the workspace, defeating the user's stated goal.
User explicitly chose full migration when shown that very tradeoff.

### B. Port the doable 90% to gix; keep `git2` as a dep just for merge/rebase

Pros: minimal code churn for merge/rebase.
Cons: libgit2-sys still compiled into the workspace. The "no C dependency"
goal is only achieved when zero workspace members reference git2.

### C. Port everything to gix; spawn the `git` CLI for merge/rebase  ← **picked**

Pros:
- Workspace has zero git2 / libgit2-sys references
- merge/rebase semantics are *literally git's* — no risk of subtle deviation
- gix maintainers explicitly recommend this pattern for ops gix doesn't cover
- This codebase is a code editor — `git` CLI is reasonable to require

Cons:
- New runtime requirement: `git` must be on PATH (very common; user already
  has it for shell use)
- Subprocess spawn overhead (~10–50 ms) on merge/rebase operations. These are
  user-initiated, not hot paths — acceptable.
- Different error messages to surface to the UI (parse stderr).

### D. Reimplement merge / rebase on top of gix primitives

Pros: pure-Rust, no runtime deps.
Cons: weeks of work; high correctness risk. Not viable in this session.

## How the git CLI fallback will work

A tiny helper in `rustic-git/src/git_cli.rs`:

```rust
fn run_git(repo_path: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C").arg(repo_path)
        .args(args)
        .output()
        .context("failed to spawn `git` — is it installed and on PATH?")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
```

Used only by:
- `pull` (replaces merge_analysis + fast_forward + merge)
- `rebase`, `rebase_continue`, `rebase_abort`

Everything else goes through gix.

## Risk if wrong

If the user prefers option A (stop) or B (keep git2 just for merge/rebase) on
review, the worst-case cleanup is:

- Option A: revert all rustic-git commits via `git revert`; file_history commit
  stays. Maybe ~30 min of churn.
- Option B: add `git2 = "0.19"` back to rustic-git/Cargo.toml, swap out the
  git-CLI fallback for libgit2 merge/rebase calls. Maybe ~1 hour of work.

## How I'll commit

One commit per module so individual modules can be reverted without taking
down the rest:

1. `rustic-git: add gix dep + scaffolding`
2. `rustic-git: port repo.rs to gix`
3. `rustic-git: port status.rs to gix`
4. `rustic-git: port diff.rs to gix`
5. `rustic-git: port log.rs to gix`
6. `rustic-git: port remote.rs to gix (push/fetch/clone), git CLI for merge`
7. `rustic-git: port conflict.rs to gix`
8. `rustic-git: drop git2 dep entirely`
