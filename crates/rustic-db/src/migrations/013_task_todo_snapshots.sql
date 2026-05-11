-- Per-turn todo-list snapshots. Pairs with file_history_snapshots (010): when a
-- new user turn opens a file snapshot under message_id M, we also record the
-- todo list AS IT EXISTED BEFORE that turn started. Reverting to M then
-- restores both the worktree AND the todo list to the same point in time.
--
-- Storage shape mirrors task_todos (012) — full list per row as JSON. Keyed by
-- message_id (one snapshot per turn). Cascades when the task is deleted so
-- snapshots for removed tasks don't leak.

CREATE TABLE IF NOT EXISTS task_todo_snapshots (
    message_id TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    todos_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Look up the earliest snapshot in a task — used by revert_task to restore
-- the todo list to its pre-task state.
CREATE INDEX IF NOT EXISTS idx_task_todo_snapshots_task_created
    ON task_todo_snapshots(task_id, created_at);
