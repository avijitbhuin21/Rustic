-- Changed-files tracker: snapshot index + blob refcount table.
-- Blobs (file content) live on disk under {configDir}/file-history/blobs/{hash[:2]}/{hash}.
-- This DB only stores the index. Triggers maintain ref_count so cascade-deletes
-- from task removal don't leak blob references.

CREATE TABLE IF NOT EXISTS file_history_snapshots (
    message_id TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    sequence   INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_fh_snapshots_task
    ON file_history_snapshots(task_id, sequence);

CREATE TABLE IF NOT EXISTS file_history_blobs (
    hash       TEXT PRIMARY KEY,
    size       INTEGER NOT NULL,
    ref_count  INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS file_history_files (
    message_id TEXT NOT NULL REFERENCES file_history_snapshots(message_id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    blob_hash  TEXT REFERENCES file_history_blobs(hash),
    PRIMARY KEY (message_id, path)
);

CREATE INDEX IF NOT EXISTS idx_fh_files_blob
    ON file_history_files(blob_hash);

-- Triggers keep blob ref_count consistent under both explicit deletes and
-- ON DELETE CASCADE from task removal. Path-pair updates (same message_id +
-- path with new blob_hash) go through INSERT OR REPLACE which fires DELETE
-- then INSERT triggers, so the old hash is decremented and the new one
-- incremented atomically.

CREATE TRIGGER IF NOT EXISTS trg_fh_files_inc
AFTER INSERT ON file_history_files
WHEN NEW.blob_hash IS NOT NULL
BEGIN
    UPDATE file_history_blobs
    SET ref_count = ref_count + 1
    WHERE hash = NEW.blob_hash;
END;

CREATE TRIGGER IF NOT EXISTS trg_fh_files_dec
AFTER DELETE ON file_history_files
WHEN OLD.blob_hash IS NOT NULL
BEGIN
    UPDATE file_history_blobs
    SET ref_count = ref_count - 1
    WHERE hash = OLD.blob_hash;
END;
