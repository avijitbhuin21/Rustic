-- Record the worktree state captured at the end of each completed turn.
-- list_task_net_changes uses this as the "current" endpoint for idle tasks
-- instead of live disk, so external edits made after a task finishes no
-- longer contaminate the Changed Files panel.
ALTER TABLE tasks ADD COLUMN final_tree_oid TEXT;
