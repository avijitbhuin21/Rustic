-- Worktree-per-task isolation removed: tasks run directly in the project
-- checkout again. Drop the merge-queue table (mirrors 005_drop_mcp_servers).
DROP TABLE IF EXISTS task_worktrees;
