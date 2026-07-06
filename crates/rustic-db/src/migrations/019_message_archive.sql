CREATE TABLE IF NOT EXISTS archived_messages (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    slot INTEGER NOT NULL,
    role TEXT NOT NULL,
    content_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (task_id, generation, slot)
);
