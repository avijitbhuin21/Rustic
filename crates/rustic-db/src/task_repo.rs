use crate::error::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{MessageRow, SubagentRecord, TaskRow};

const TASK_COLUMNS: &str =
    "id, project_id, title, status, provider_type, model, created_at, updated_at, \
     total_input_tokens, total_output_tokens, total_cache_read_tokens, estimated_cost_usd, turn_count, \
     harness_session_id, cost_json, thinking_tier, pinned, goal, \
     goal_turns, goal_status, goal_reason";

/// `TASK_COLUMNS` for reads: `updated_at` is the later of the task's own stamp
/// and its newest sub-agent record, so sub-agent activity counts as activity in
/// the thread. Derived at read time because sub-agent text is appended per
/// streamed delta, and stamping the task row on each one would double that
/// write path.
const TASK_SELECT: &str =
    "id, project_id, title, status, provider_type, model, created_at, \
     COALESCE((SELECT MAX(s.updated_at) FROM subagent_records s \
               WHERE s.task_id = tasks.id AND s.updated_at > datetime(tasks.updated_at)), \
              tasks.updated_at) AS updated_at, \
     total_input_tokens, total_output_tokens, total_cache_read_tokens, estimated_cost_usd, turn_count, \
     harness_session_id, cost_json, thinking_tier, pinned, goal, \
     goal_turns, goal_status, goal_reason";

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        status: row.get(3)?,
        provider_type: row.get(4)?,
        model: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        total_input_tokens: row.get(8)?,
        total_output_tokens: row.get(9)?,
        total_cache_read_tokens: row.get(10)?,
        estimated_cost_usd: row.get(11)?,
        turn_count: row.get(12)?,
        harness_session_id: row.get(13)?,
        cost_json: row.get(14)?,
        thinking_tier: row.get(15)?,
        pinned: row.get(16)?,
        goal: row.get(17)?,
        goal_turns: row.get(18)?,
        goal_status: row.get(19)?,
        goal_reason: row.get(20)?,
    })
}

impl Database {
    pub fn insert_task(&self, task: &TaskRow) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT OR IGNORE INTO tasks ({TASK_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)"
            ),
            params![
                task.id, task.project_id, task.title, task.status,
                task.provider_type, task.model, task.created_at, task.updated_at,
                task.total_input_tokens, task.total_output_tokens, task.total_cache_read_tokens,
                task.estimated_cost_usd, task.turn_count, task.harness_session_id, task.cost_json,
                task.thinking_tier, task.pinned, task.goal,
                task.goal_turns, task.goal_status, task.goal_reason
            ],
        )?;
        Ok(())
    }

    /// Persist the harness CLI's reported session id so a future reopen can
    /// pass `--resume <id>` and restore the conversation. Idempotent — calling
    /// it again with the same value is a no-op for the user.
    pub fn update_task_harness_session_id(&self, id: &str, session_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET harness_session_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![session_id, id],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskRow>> {
        let mut stmt = self
            .conn()
            .prepare_cached(&format!("SELECT {TASK_SELECT} FROM tasks WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], row_to_task)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_tasks_for_project(&self, project_id: &str) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {TASK_SELECT} FROM tasks WHERE project_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![project_id], row_to_task)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// List every task across all projects, newest first. Used by the
    /// orchestrator's `list_tasks_across_projects` tool.
    pub fn list_all_tasks(&self) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {TASK_SELECT} FROM tasks ORDER BY updated_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_task)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    /// Correct a status left behind by a crashed session without moving `updated_at` (nothing happened in the thread).
    pub fn reset_stale_task_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    // Rename and model switch leave updated_at alone: the chat list shows it
    // as the time of the last entry in the thread, and neither is one.
    pub fn update_task_title(&self, id: &str, title: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    pub fn update_task_model(&self, id: &str, provider_type: &str, model: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET provider_type = ?1, model = ?2 WHERE id = ?3",
            params![provider_type, model, id],
        )?;
        Ok(())
    }

    /// Persist the reasoning-effort tier the user last used with this task.
    pub fn update_task_thinking_tier(&self, id: &str, tier: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET thinking_tier = ?1 WHERE id = ?2",
            params![tier, id],
        )?;
        Ok(())
    }

    /// Persist a task's sticky-note pin state without touching updated_at.
    pub fn update_task_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )?;
        Ok(())
    }

    /// Persist or clear a task's /goal completion condition. Either way the
    /// loop's progress starts over.
    pub fn update_task_goal(&self, id: &str, goal: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET goal = ?1, goal_turns = 0, goal_status = NULL, goal_reason = NULL \
             WHERE id = ?2",
            params![goal, id],
        )?;
        Ok(())
    }

    /// Persist where a task's /goal loop stands: its forced-turn count and,
    /// while it is blocked, the status and reason. No-op once the goal is gone.
    pub fn update_task_goal_progress(
        &self,
        id: &str,
        turns: u32,
        status: Option<&str>,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET goal_turns = ?1, goal_status = ?2, goal_reason = ?3 \
             WHERE id = ?4 AND goal IS NOT NULL",
            params![turns, status, reason, id],
        )?;
        Ok(())
    }

    /// Persist the latest cost data for a task.
    pub fn update_task_cost(
        &self,
        id: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cost_usd: f64,
        turn_count: i64,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET \
             total_input_tokens = ?1, total_output_tokens = ?2, total_cache_read_tokens = ?3, \
             estimated_cost_usd = ?4, turn_count = ?5, updated_at = datetime('now') \
             WHERE id = ?6",
            params![
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cost_usd,
                turn_count,
                id
            ],
        )?;
        Ok(())
    }

    /// Persist the full TaskCost snapshot (incl. per-model breakdown) as JSON.
    pub fn update_task_cost_json(&self, id: &str, cost_json: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET cost_json = ?1 WHERE id = ?2",
            params![cost_json, id],
        )?;
        Ok(())
    }

    pub fn upsert_message(&self, msg: &MessageRow) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO messages (id, task_id, role, content_json, created_at, sort_order, turn_usage_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![msg.id, msg.task_id, msg.role, msg.content_json, msg.created_at, msg.sort_order, msg.turn_usage_json],
        )?;
        Ok(())
    }

    pub fn delete_messages_for_task(&self, task_id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM messages WHERE task_id = ?1", params![task_id])?;
        Ok(())
    }

    /// Delete messages from `from_sort_order` onwards (inclusive).
    /// Used to truncate a task's chat history back to a checkpoint.
    pub fn truncate_messages_from(&self, task_id: &str, from_sort_order: i64) -> Result<()> {
        self.conn().execute(
            "DELETE FROM messages WHERE task_id = ?1 AND sort_order >= ?2",
            params![task_id, from_sort_order],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_tasks_for_project(&self, project_id: &str) -> Result<()> {
        // Messages are deleted by ON DELETE CASCADE from the tasks FK
        self.conn().execute(
            "DELETE FROM tasks WHERE project_id = ?1",
            params![project_id],
        )?;
        Ok(())
    }

    pub fn insert_message(&self, msg: &MessageRow) -> Result<()> {
        self.conn().execute(
            "INSERT INTO messages (id, task_id, role, content_json, created_at, sort_order, turn_usage_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![msg.id, msg.task_id, msg.role, msg.content_json, msg.created_at, msg.sort_order, msg.turn_usage_json],
        )?;
        Ok(())
    }

    /// Atomically replace the entire message history for a task: deletes all
    /// existing rows and inserts the supplied set inside a single SQLite
    /// transaction. The previous implementation called `delete_messages_for_task`
    /// followed by a loop of `insert_message`s without a transaction, which is
    /// not crash-safe — if the worker thread died between the DELETE and the
    /// inserts (e.g. the user clicks Stop and immediately closes the app),
    /// the messages table was left empty even though prior turns had been
    /// committed earlier. With the transaction the DELETE is rolled back if
    /// any insert fails or the process exits before COMMIT, so the previous
    /// turn's messages survive.
    pub fn replace_messages_for_task(&self, task_id: &str, messages: &[MessageRow]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        // Message history is append-only per index, so `sort_order` identifies
        // the same message across turns. Carry the first-seen `created_at`
        // forward instead of re-stamping every row with "now" — otherwise the
        // whole transcript's timestamps reset on each turn and the UI's
        // per-message / per-tool-call elapsed labels lose their anchor.
        let existing: std::collections::HashMap<i64, String> = {
            let mut stmt =
                tx.prepare("SELECT sort_order, created_at FROM messages WHERE task_id = ?1")?;
            let rows = stmt.query_map(params![task_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()?
        };
        tx.execute("DELETE FROM messages WHERE task_id = ?1", params![task_id])?;
        for msg in messages {
            let created_at = existing
                .get(&msg.sort_order)
                .cloned()
                .unwrap_or_else(|| msg.created_at.clone());
            tx.execute(
                "INSERT INTO messages (id, task_id, role, content_json, created_at, sort_order, turn_usage_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![msg.id, msg.task_id, msg.role, msg.content_json, created_at, msg.sort_order, msg.turn_usage_json],
            )?;
        }
        tx.commit()?;
        // Caller (rustic-agent / src-tauri) logs around this call site;
        // rustic-db keeps no tracing dependency to stay leaf-pure.
        Ok(())
    }

    /// Incremental persist: upserts rows by id, rewriting only those whose role/content changed, and deletes trailing rows past the new length (history shrank after condense). Preserves first-seen `created_at` and any existing `turn_usage_json`. When anything was written, stamps `tasks.updated_at` — the chat list's "last entry in the thread" time.
    pub fn upsert_messages_for_task(&self, task_id: &str, messages: &[MessageRow]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        let mut written = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (id, task_id, role, content_json, created_at, sort_order, turn_usage_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                     role = excluded.role,
                     content_json = excluded.content_json,
                     sort_order = excluded.sort_order,
                     turn_usage_json = COALESCE(excluded.turn_usage_json, messages.turn_usage_json)
                 WHERE messages.role IS NOT excluded.role
                    OR messages.content_json IS NOT excluded.content_json
                    OR messages.sort_order IS NOT excluded.sort_order
                    OR (excluded.turn_usage_json IS NOT NULL
                        AND messages.turn_usage_json IS NOT excluded.turn_usage_json)",
            )?;
            for msg in messages {
                written += stmt.execute(params![
                    msg.id,
                    msg.task_id,
                    msg.role,
                    msg.content_json,
                    msg.created_at,
                    msg.sort_order,
                    msg.turn_usage_json
                ])?;
            }
        }
        written += tx.execute(
            "DELETE FROM messages WHERE task_id = ?1 AND sort_order >= ?2",
            params![task_id, messages.len() as i64],
        )?;
        if written > 0 {
            tx.execute(
                "UPDATE tasks SET updated_at = datetime('now') WHERE id = ?1",
                params![task_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_messages_for_task(&self, task_id: &str) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, task_id, role, content_json, created_at, sort_order, turn_usage_json
             FROM messages WHERE task_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                role: row.get(2)?,
                content_json: row.get(3)?,
                created_at: row.get(4)?,
                sort_order: row.get(5)?,
                turn_usage_json: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Per-message `created_at` keyed by `sort_order`, for callers that serve
    /// message history from an in-memory cache but still need the persisted
    /// timestamps.
    pub fn get_message_times(&self, task_id: &str) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT sort_order, created_at FROM messages WHERE task_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt.query_map(params![task_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Raw `content_json` for one message, addressed by `sort_order`. Lets the
    /// UI fetch a single full block (a truncated tool result, an image payload)
    /// without loading — and deserializing — the task's whole history.
    pub fn get_message_content(&self, task_id: &str, sort_order: i64) -> Result<Option<String>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT content_json FROM messages WHERE task_id = ?1 AND sort_order = ?2",
        )?;
        let mut rows = stmt.query(params![task_id, sort_order])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// `(id, content_json)` for every message carrying an image block, paged by
    /// `after_id` so a 460 MB database is never loaded at once. Serves both
    /// media-store jobs: converting legacy inline payloads, and collecting the
    /// set of payloads still referenced.
    pub fn messages_with_images(
        &self,
        after_id: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, content_json FROM messages
             WHERE id > ?1 AND content_json LIKE '%\"type\":\"image\"%'
             ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_id, limit], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Replace one message's serialized content in place.
    pub fn set_message_content(&self, id: &str, content_json: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE messages SET content_json = ?1 WHERE id = ?2",
            params![content_json, id],
        )?;
        Ok(())
    }

    pub fn get_next_sort_order(&self, task_id: &str) -> Result<i64> {
        let count: i64 = self.conn().query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM messages WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    // ── Sub-agent records ────────────────────────────────────────────────
    // Persisted per (task_id, agent_id) so the chat view can hydrate
    // sub-agent cards on reload — prompt, final summary, status, and
    // rolled-up cost/tokens. Without this the spawn_subagent tool_use's
    // tool_result (a brief "spawned" acknowledgement) is the only thing
    // that survives reload, leaving cards empty.

    pub fn upsert_subagent_spawn(
        &self,
        task_id: &str,
        agent_id: &str,
        model: &str,
        prompt: &str,
        name: &str,
    ) -> Result<()> {
        // Preserves summary/cost/status if this row already exists (e.g. the
        // spawn event arrives after a CostUpdate for the same agent — unlikely
        // but defensive).
        self.conn().execute(
            "INSERT INTO subagent_records (task_id, agent_id, model, prompt, status, name)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5)
             ON CONFLICT(task_id, agent_id) DO UPDATE SET
               model = excluded.model,
               prompt = excluded.prompt,
               name = excluded.name,
               updated_at = datetime('now')",
            params![task_id, agent_id, model, prompt, name],
        )?;
        Ok(())
    }

    pub fn update_subagent_cost(
        &self,
        task_id: &str,
        agent_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cost_usd: f64,
    ) -> Result<()> {
        // INSERT OR IGNORE first so the row exists if the cost update races
        // ahead of the spawn event (the executor emits CostUpdate from the
        // sub-agent's own turn, which can arrive before the spawn marker in
        // some orderings).
        self.conn().execute(
            "INSERT OR IGNORE INTO subagent_records (task_id, agent_id) VALUES (?1, ?2)",
            params![task_id, agent_id],
        )?;
        self.conn().execute(
            "UPDATE subagent_records SET
               input_tokens = ?3, output_tokens = ?4,
               cache_read_tokens = ?5, cost_usd = ?6,
               updated_at = datetime('now')
             WHERE task_id = ?1 AND agent_id = ?2",
            params![
                task_id,
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cost_usd
            ],
        )?;
        Ok(())
    }

    pub fn update_subagent_summary(
        &self,
        task_id: &str,
        agent_id: &str,
        summary: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO subagent_records (task_id, agent_id) VALUES (?1, ?2)",
            params![task_id, agent_id],
        )?;
        self.conn().execute(
            "UPDATE subagent_records SET
               summary = ?3, status = 'completed', updated_at = datetime('now')
             WHERE task_id = ?1 AND agent_id = ?2",
            params![task_id, agent_id, summary],
        )?;
        Ok(())
    }

    pub fn update_subagent_error(&self, task_id: &str, agent_id: &str, error: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO subagent_records (task_id, agent_id) VALUES (?1, ?2)",
            params![task_id, agent_id],
        )?;
        self.conn().execute(
            "UPDATE subagent_records SET
               error = ?3, status = 'failed', updated_at = datetime('now')
             WHERE task_id = ?1 AND agent_id = ?2",
            params![task_id, agent_id, error],
        )?;
        Ok(())
    }

    /// Append assistant text to a sub-agent's running output buffer. Called
    /// from the host's `agent-subagent-text-delta` event handler so the
    /// streamed transcript is durable as it arrives — without this, closing
    /// the app mid-run loses every word the sub-agent produced.
    pub fn append_subagent_output(&self, task_id: &str, agent_id: &str, delta: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO subagent_records (task_id, agent_id) VALUES (?1, ?2)",
            params![task_id, agent_id],
        )?;
        self.conn().execute(
            "UPDATE subagent_records SET
               output_text = output_text || ?3,
               updated_at = datetime('now')
             WHERE task_id = ?1 AND agent_id = ?2",
            params![task_id, agent_id, delta],
        )?;
        Ok(())
    }

    /// Replace a sub-agent's persisted tool-calls array with `json` (a
    /// JSON-encoded array). Called from the `tool_use` / `tool_result`
    /// event handlers with the latest snapshot from the in-memory store.
    pub fn set_subagent_tool_calls(&self, task_id: &str, agent_id: &str, json: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO subagent_records (task_id, agent_id) VALUES (?1, ?2)",
            params![task_id, agent_id],
        )?;
        self.conn().execute(
            "UPDATE subagent_records SET
               tool_calls_json = ?3,
               updated_at = datetime('now')
             WHERE task_id = ?1 AND agent_id = ?2",
            params![task_id, agent_id, json],
        )?;
        Ok(())
    }

    pub fn get_subagent_records_for_task(&self, task_id: &str) -> Result<Vec<SubagentRecord>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT task_id, agent_id, model, prompt, summary, status,
                    input_tokens, output_tokens, cache_read_tokens, cost_usd,
                    error, created_at, updated_at, output_text, tool_calls_json, name
             FROM subagent_records
             WHERE task_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(SubagentRecord {
                task_id: row.get(0)?,
                agent_id: row.get(1)?,
                model: row.get(2)?,
                prompt: row.get(3)?,
                summary: row.get(4)?,
                status: row.get(5)?,
                input_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                cache_read_tokens: row.get(8)?,
                cost_usd: row.get(9)?,
                error: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                output_text: row.get(13)?,
                tool_calls_json: row.get(14)?,
                name: row.get(15)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// One sub-agent's untrimmed replay payload: `(output_text,
    /// tool_calls_json)`. The list query returns these cut down, so the UI pulls
    /// the full pair only for the child the user actually opens.
    pub fn get_subagent_replay(
        &self,
        task_id: &str,
        agent_id: &str,
    ) -> Result<Option<(String, String)>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT output_text, tool_calls_json FROM subagent_records
             WHERE task_id = ?1 AND agent_id = ?2",
        )?;
        let mut rows = stmt.query(params![task_id, agent_id])?;
        match rows.next()? {
            Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::Database;
    use crate::models::{MessageRow, ProjectRow, TaskRow};

    const STARTED: &str = "2020-01-01 00:00:00";

    fn db_with_task() -> Database {
        let db = Database::in_memory().expect("init");
        db.insert_project(&ProjectRow {
            id: "p".into(),
            name: "p".into(),
            root_path: "/tmp/p".into(),
            created_at: STARTED.into(),
            settings_json: None,
            sort_order: 0,
        })
        .expect("project");
        db.insert_task(&TaskRow {
            id: "t".into(),
            project_id: "p".into(),
            title: "old".into(),
            status: "Completed".into(),
            provider_type: "Claude".into(),
            model: "m".into(),
            created_at: STARTED.into(),
            updated_at: STARTED.into(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            estimated_cost_usd: 0.0,
            turn_count: 0,
            harness_session_id: None,
            cost_json: None,
            thinking_tier: None,
            pinned: false,
            goal: None,
            goal_turns: 0,
            goal_status: None,
            goal_reason: None,
        })
        .expect("task");
        db
    }

    fn updated_at(db: &Database) -> String {
        db.get_task("t").expect("get").expect("row").updated_at
    }

    fn rewind(db: &Database) {
        db.conn()
            .execute("UPDATE tasks SET updated_at = ?1 WHERE id = 't'", [STARTED])
            .expect("reset");
    }

    fn msg(i: i64, role: &str, content: &str) -> MessageRow {
        MessageRow {
            id: format!("t-{i}"),
            task_id: "t".into(),
            role: role.into(),
            content_json: content.into(),
            created_at: "2026-01-01 00:00:00".into(),
            sort_order: i,
            turn_usage_json: None,
        }
    }

    #[test]
    fn goal_progress_survives_a_reload_and_resets_with_the_goal() {
        let db = db_with_task();
        let goal = |db: &Database| {
            let t = db.get_task("t").expect("get").expect("row");
            (t.goal, t.goal_turns, t.goal_status, t.goal_reason)
        };

        // No goal: progress is not recorded.
        db.update_task_goal_progress("t", 3, Some("blocked"), Some("x"))
            .expect("progress");
        assert_eq!(goal(&db), (None, 0, None, None));

        db.update_task_goal("t", Some("ship it")).expect("set");
        db.update_task_goal_progress("t", 4, None, None)
            .expect("continuing");
        assert_eq!(goal(&db), (Some("ship it".into()), 4, None, None));

        db.update_task_goal_progress("t", 4, Some("blocked"), Some("needs a key"))
            .expect("blocked");
        assert_eq!(
            goal(&db),
            (
                Some("ship it".into()),
                4,
                Some("blocked".into()),
                Some("needs a key".into())
            )
        );
        assert_eq!(
            updated_at(&db),
            STARTED,
            "goal bookkeeping is not a thread entry"
        );

        // The loop moving again clears the block and keeps counting.
        db.update_task_goal_progress("t", 5, None, None)
            .expect("resumed");
        assert_eq!(goal(&db), (Some("ship it".into()), 5, None, None));

        // A new /goal and a clear both start over.
        db.update_task_goal_progress("t", 5, Some("blocked"), Some("again"))
            .expect("blocked again");
        db.update_task_goal("t", Some("other")).expect("replace");
        assert_eq!(goal(&db), (Some("other".into()), 0, None, None));
        db.update_task_goal_progress("t", 2, None, None)
            .expect("progress");
        db.update_task_goal("t", None).expect("clear");
        assert_eq!(goal(&db), (None, 0, None, None));
    }

    #[test]
    fn updated_at_follows_the_last_thread_entry() {
        let db = db_with_task();
        let user = msg(0, "user", "[\"hi\"]");
        let tool_call = msg(1, "assistant", "[\"tool_use\"]");
        let tool_result = msg(2, "user", "[\"tool_result\"]");

        db.upsert_messages_for_task("t", std::slice::from_ref(&user))
            .expect("user");
        assert!(updated_at(&db).as_str() > STARTED, "user message");

        rewind(&db);
        db.upsert_messages_for_task("t", &[user.clone(), tool_call.clone()])
            .expect("tool call");
        assert!(updated_at(&db).as_str() > STARTED, "tool call");

        rewind(&db);
        let thread = [user.clone(), tool_call.clone(), tool_result];
        db.upsert_messages_for_task("t", &thread)
            .expect("tool result");
        assert!(updated_at(&db).as_str() > STARTED, "tool result");

        rewind(&db);
        db.upsert_messages_for_task("t", &thread).expect("no-op");
        assert_eq!(updated_at(&db), STARTED, "re-saving an unchanged thread");

        db.reset_stale_task_status("t", "Completed").expect("stale");
        assert_eq!(updated_at(&db), STARTED, "crash-leftover status reset");

        db.upsert_messages_for_task("t", &[user, tool_call])
            .expect("shrink");
        assert!(updated_at(&db).as_str() > STARTED, "thread shrank");
    }

    fn set_subagent_time(db: &Database, at: &str) {
        db.conn()
            .execute(
                "UPDATE subagent_records SET updated_at = ?1 WHERE task_id = 't'",
                [at],
            )
            .expect("subagent time");
    }

    #[test]
    fn updated_at_includes_subagent_activity() {
        let db = db_with_task();
        db.upsert_subagent_spawn("t", "a1", "m", "do it", "worker")
            .expect("spawn");
        db.upsert_subagent_spawn("t", "a2", "m", "do it", "worker")
            .expect("spawn");
        db.append_subagent_output("t", "a2", "hello")
            .expect("output");

        let listed = db.list_tasks_for_project("p").expect("list");
        assert!(
            listed[0].updated_at.as_str() > STARTED,
            "list sees sub-agent activity"
        );
        assert_eq!(listed[0].updated_at, updated_at(&db), "list and get agree");
        assert_eq!(
            db.list_all_tasks().expect("all")[0].updated_at,
            listed[0].updated_at
        );
        let raw: String = db
            .conn()
            .query_row("SELECT updated_at FROM tasks WHERE id = 't'", [], |r| {
                r.get(0)
            })
            .expect("raw");
        assert_eq!(raw, STARTED, "sub-agent writes do not touch the task row");

        set_subagent_time(&db, "2019-06-01 00:00:00");
        assert_eq!(updated_at(&db), STARTED, "the task's own later stamp wins");

        // In-memory stamps are RFC 3339; they must compare by instant, not text.
        db.conn()
            .execute(
                "UPDATE tasks SET updated_at = '2020-01-01T05:30:00+05:30' WHERE id = 't'",
                [],
            )
            .expect("rfc3339");
        set_subagent_time(&db, "2020-01-01 00:00:00");
        assert_eq!(
            updated_at(&db),
            "2020-01-01T05:30:00+05:30",
            "same instant keeps the task stamp"
        );
        set_subagent_time(&db, "2020-01-01 00:00:01");
        assert_eq!(
            updated_at(&db),
            "2020-01-01 00:00:01",
            "one second later wins"
        );
    }

    #[test]
    fn updated_at_tracks_conversation_activity_only() {
        let db = db_with_task();

        db.update_task_title("t", "renamed").expect("title");
        db.update_task_model("t", "OpenAI", "gpt").expect("model");
        db.update_task_pinned("t", true).expect("pin");
        let row = db.get_task("t").expect("get").expect("row");
        assert_eq!(row.title, "renamed");
        assert_eq!(row.model, "gpt");
        assert_eq!(row.updated_at, STARTED, "metadata edits are not activity");

        db.update_task_cost("t", 1, 1, 0, 0.01, 1).expect("cost");
        let after_turn = updated_at(&db);
        assert!(after_turn.as_str() > STARTED, "a turn moves updated_at");

        db.conn()
            .execute("UPDATE tasks SET updated_at = ?1 WHERE id = 't'", [STARTED])
            .expect("reset");
        db.update_task_status("t", "Completed").expect("status");
        assert!(
            updated_at(&db).as_str() > STARTED,
            "a run ending moves updated_at"
        );
    }
}
