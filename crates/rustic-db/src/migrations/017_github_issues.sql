-- GitHub auto-issue-resolve (rustic-server only feature; the tables live in
-- the shared DB so desktop builds simply never write them).
--
-- `github_issues` is the durable register of every issue we track: one row
-- per (repo, issue number), carrying the local task binding, the lifecycle
-- status, and the pending ask_user suspension when the agent is waiting for
-- the reporter's reply on GitHub.
--
-- status values:
--   queued        — webhook received, not yet picked up by the worker
--   working       — the fixer task is currently running
--   waiting_reply — agent asked a question; posted as an issue comment,
--                   turn suspended until a reply comment arrives
--   done          — fix committed locally, ✓ reaction posted
--   failed        — task failed (provider error, cost cap, …)
--   manual        — a human took over the chat from the Rustic UI
CREATE TABLE IF NOT EXISTS github_issues (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id             TEXT    NOT NULL,
    repo_full_name         TEXT    NOT NULL,
    issue_number           INTEGER NOT NULL,
    title                  TEXT    NOT NULL DEFAULT '',
    issue_url              TEXT    NOT NULL DEFAULT '',
    -- Bound chat/task id; NULL until the worker first picks the issue up.
    task_id                TEXT,
    status                 TEXT    NOT NULL DEFAULT 'queued',
    -- ask_user suspension: the dangling tool_use id awaiting an answer and
    -- the questions JSON that was posted to the issue.
    pending_tool_use_id    TEXT,
    pending_questions_json TEXT,
    -- Per-issue cost cap snapshot (USD) taken from project config at enqueue
    -- time; NULL = uncapped.
    cost_cap_usd           REAL,
    error                  TEXT    NOT NULL DEFAULT '',
    created_at             TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at             TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (repo_full_name, issue_number)
);

CREATE INDEX IF NOT EXISTS idx_github_issues_task ON github_issues(task_id);
CREATE INDEX IF NOT EXISTS idx_github_issues_status ON github_issues(status);

-- FIFO work queue feeding the single-threaded issue worker. One row per
-- webhook delivery we decided to act on. kinds:
--   new_issue    — issue opened (or first seen via labeled/reopened)
--   issue_update — body edited / reopened on an already-tracked issue
--   comment      — a (non-bot) comment arrived; resumes a waiting_reply
--                  suspension or rides along as extra context otherwise
CREATE TABLE IF NOT EXISTS github_events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id     INTEGER NOT NULL REFERENCES github_issues(id) ON DELETE CASCADE,
    kind         TEXT    NOT NULL,
    payload_json TEXT    NOT NULL DEFAULT '{}',
    created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_github_events_issue ON github_events(issue_id);
