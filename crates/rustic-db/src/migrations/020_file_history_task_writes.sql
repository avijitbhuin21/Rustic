-- Per-task write attribution: paths a task's edit tools wrote or its bash
-- sweeps detected. Revert scoping prefers these rows over tree-diff
-- inference so whole-tree snapshots that absorb a parallel session's edits
-- can't put foreign paths in this task's blast radius.
CREATE TABLE IF NOT EXISTS file_history_task_writes (
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (task_id, path)
);
