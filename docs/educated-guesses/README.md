# Educated guesses

Decisions I made autonomously while the user was away. Each file documents one
judgment call I had to make without being able to ask. Review on return — if
any look wrong, flag and we'll revisit.

## Format

Each `.md` file in this folder:

- **Decision:** one-line summary of what I did
- **Context:** what the question was, why it came up
- **Options I considered:** alternatives, with tradeoffs
- **What I picked:** the choice, with reasoning
- **Risk if wrong:** what breaks and how to revert

## Entries from this session (2026-05-25)

- `001-scope-full-migration.md` — Scope of the gix migration. User answered:
  full migration. I delivered file_history; deferred rustic-git (see 004).
- `002-port-style.md` — Idiomatic gix rewrite (not 1:1 mirror) for shadow.rs.
- `003-rustic-git-merge-rebase-strategy.md` — How to handle gix's missing
  merge/rebase: spawn `git` CLI. Plan, not yet executed.
- `004-stopping-rustic-git-mid-session.md` — Why I stopped before porting
  rustic-git this session. Three paths forward for your review.
- `005-notify-integration-design.md` — Complete design for the FS-watcher
  acceleration of the sweep. No code written; ready for next session.

## What's done vs deferred

### ✅ Done this session
- `file_history` shadow store fully ported from libgit2 → gitoxide
- 56 file_history tests pass; 307 total rustic-agent tests pass
- Documented strategy for rustic-git port (decision needed on your end)
- Designed notify integration (ready to execute next session)

### ⏸ Deferred for your review
- rustic-git port — three options in [004](004-stopping-rustic-git-mid-session.md)
- notify integration — design ready, implementation ~3 days in [005](005-notify-integration-design.md)
