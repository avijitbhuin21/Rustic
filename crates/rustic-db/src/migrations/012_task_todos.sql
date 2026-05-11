-- Persistent todos for the agent's checklist (the `todo_write` tool).
-- Each task has at most one current list; the tool replaces it on every call,
-- so we store the whole list as a JSON array keyed by task_id and overwrite
-- in place. Cascades when the task is deleted.

CREATE TABLE IF NOT EXISTS task_todos (
    task_id    TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    todos_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
