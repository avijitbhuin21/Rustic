-- Resume support for harness-backed tasks (Claude Code, Codex, ...).
--
-- The CLI mints its own session id and persists conversation history under
-- `~/.claude/projects/<hash>/`. We capture that id from the `system:init`
-- envelope on first spawn and persist it here, then pass it back via
-- `claude --resume <id>` when the user reopens the same task. Native
-- API-key tasks never write this column; it stays NULL for them.
ALTER TABLE tasks ADD COLUMN harness_session_id TEXT;
