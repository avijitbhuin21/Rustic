CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    message_index INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS file_snapshots (
    id TEXT PRIMARY KEY,
    checkpoint_id TEXT NOT NULL REFERENCES checkpoints(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    content BLOB NOT NULL,
    was_new INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_snapshots_checkpoint ON file_snapshots(checkpoint_id);
CREATE INDEX IF NOT EXISTS idx_checkpoints_task ON checkpoints(task_id);
CREATE INDEX IF NOT EXISTS idx_messages_task ON messages(task_id, sort_order);
