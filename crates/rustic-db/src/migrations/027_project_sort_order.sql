-- Explicit user-defined ordering for projects in the workspace explorer.
-- Previously projects were listed by `created_at`; this lets the user
-- drag-reorder them and have the arrangement persist across restarts.
-- Backfill by rowid so the existing (creation-order) arrangement is
-- preserved as the initial sort order.
ALTER TABLE projects ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
UPDATE projects SET sort_order = rowid;
