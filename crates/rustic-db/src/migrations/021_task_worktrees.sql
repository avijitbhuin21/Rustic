-- Worktree-per-task + serialized merge queue (docs/plans/worktree-merge-queue.md).
-- One row per isolated task worktree. The merge queue is DERIVED from this
-- table (state='queued' ORDER BY queued_at) -- no separate queue table, so
-- restarts recover for free. `merging` rows found at startup are reset to
-- `queued` (the worker is idempotent up to the ff-push).
CREATE TABLE IF NOT EXISTS task_worktrees (
    task_id       TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    project_id    TEXT NOT NULL,
    project_root  TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    branch        TEXT NOT NULL,
    base_branch   TEXT NOT NULL,
    base_oid      TEXT NOT NULL,
    state         TEXT NOT NULL DEFAULT 'active',
    queued_at     TEXT,
    merged_oid    TEXT,
    last_error    TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_task_worktrees_state
    ON task_worktrees(state, queued_at);
CREATE INDEX IF NOT EXISTS idx_task_worktrees_project
    ON task_worktrees(project_id);
