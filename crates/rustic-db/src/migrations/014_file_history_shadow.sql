-- R.1 / Day 3: replace the per-file blob index with shadow-git tree oids.
-- Clean break per docs/file_tracking_decision.md §0 — pre-1.0, no migration
-- of existing data. Any rows that survive lose their revert ability (they'll
-- have a NULL tree_oid) and will be evicted by retention caps on their next
-- task.

DROP TRIGGER IF EXISTS trg_fh_files_inc;
DROP TRIGGER IF EXISTS trg_fh_files_dec;
DROP INDEX  IF EXISTS idx_fh_files_blob;
DROP INDEX  IF EXISTS idx_fh_files_path;
DROP TABLE  IF EXISTS file_history_files;
DROP TABLE  IF EXISTS file_history_blobs;

-- file_history_snapshots stays — same PK, same task FK — but gains a tree_oid
-- pointing at the shadow repo. libgit2 hashes are 40-char hex (SHA-1).
ALTER TABLE file_history_snapshots ADD COLUMN tree_oid TEXT;
