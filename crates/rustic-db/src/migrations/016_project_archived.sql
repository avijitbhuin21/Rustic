-- Soft-removal flag for projects. Removing a project from the workspace must
-- NOT delete its task/chat history (which `DELETE FROM projects` does via
-- `ON DELETE CASCADE` on tasks → messages). Instead we mark the project
-- archived: it disappears from the workspace and won't rehydrate on startup,
-- but its tasks and messages survive so re-adding the same folder restores the
-- full history.
ALTER TABLE projects ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
