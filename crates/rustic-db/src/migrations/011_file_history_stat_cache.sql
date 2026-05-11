-- Stat cache columns for the file_history_files index. Used by the
-- open_snapshot pre-capture path to skip re-hashing files whose on-disk
-- mtime + size haven't changed since the most recent snapshot recorded
-- their content. Both columns are NULLABLE so existing rows (and bash
-- sweeps that record a "did not exist" / "post-bash" entry) stay valid.

ALTER TABLE file_history_files ADD COLUMN mtime_ns INTEGER;
ALTER TABLE file_history_files ADD COLUMN size INTEGER;

CREATE INDEX IF NOT EXISTS idx_fh_files_path
    ON file_history_files(path);
