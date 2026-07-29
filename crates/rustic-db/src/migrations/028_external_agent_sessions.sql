-- External CLI agent sessions: Claude Code (`claude`), Codex (`codex`) and
-- Antigravity (`agy`) spawned as PTY tabs inside Rustic.
--
-- One row per *conversation*, not per spawn. Resuming an existing conversation
-- updates the row in place (including `pty_key`) rather than inserting.
--
-- `pty_key` is the RUSTIC_PTY_ID token injected into the PTY env at spawn. The
-- CLIs' hooks inherit it and echo it back, which is how a hook callback is
-- correlated to the tab that produced it. A fresh token is minted on every
-- spawn and only ever lives on one row, so it is globally unique.
--
-- `external_session_id` is the CLI's own id, and the ONLY supported way to
-- resume (`claude --resume`, `codex resume`, `agy --conversation`). It is NULL
-- between spawn and the first hook/watcher callback that reveals it, so
-- callers must tolerate a row that is not yet resumable.

CREATE TABLE IF NOT EXISTS external_agent_sessions (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    agent               TEXT NOT NULL,
    pty_key             TEXT NOT NULL UNIQUE,
    external_session_id TEXT,
    title               TEXT,
    transcript_path     TEXT,
    cwd                 TEXT NOT NULL,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    last_active_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_external_agent_sessions_project
    ON external_agent_sessions(project_id, agent, last_active_at DESC);

-- A conversation id is unique per CLI. Partial so the many not-yet-known
-- (NULL) ids don't collide with each other.
CREATE UNIQUE INDEX IF NOT EXISTS idx_external_agent_sessions_external_id
    ON external_agent_sessions(agent, external_session_id)
    WHERE external_session_id IS NOT NULL;
