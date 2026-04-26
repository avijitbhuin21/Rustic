use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{MessageRow, SubagentRecord, TaskRow};

const TASK_COLUMNS: &str =
    "id, project_id, title, status, provider_type, model, created_at, updated_at, \
     total_input_tokens, total_output_tokens, total_cache_read_tokens, estimated_cost_usd, turn_count";

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
    })
}

impl Database {
    pub fn insert_task(&self, task: &TaskRow) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT OR IGNORE INTO tasks ({TASK_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            params![
                task.id, task.project_id, task.title, task.status,
                task.provider_type, task.model, task.created_at, task.updated_at,
                task.total_input_tokens, task.total_output_tokens, task.total_cache_read_tokens,
                task.estimated_cost_usd, task.turn_count
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskRow>> {
        let mut stmt = self.conn().prepare(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1")
        )?;
        let mut rows = stmt.query_map(params![id], |row| row_to_task(row))?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_tasks_for_project(&self, project_id: &str) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn().prepare(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE project_id = ?1 ORDER BY created_at DESC")
        )?;
        let rows = stmt.query_map(params![project_id], |row| row_to_task(row))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// List every task across all projects, newest first. Used by the
    /// orchestrator's `list_tasks_across_projects` tool.
    pub fn list_all_tasks(&self) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn().prepare(
            &format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY updated_at DESC")
        )?;
        let rows = stmt.query_map([], |row| row_to_task(row))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    pub fn update_task_title(&self, id: &str, title: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    pub fn update_task_model(&self, id: &str, provider_type: &str, model: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET provider_type = ?1, model = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![provider_type, model, id],
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
            params![input_tokens, output_tokens, cache_read_tokens, cost_usd, turn_count, id],
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
        self.conn().execute("DELETE FROM messages WHERE task_id = ?1", params![task_id])?;
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
        self.conn().execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_tasks_for_project(&self, project_id: &str) -> Result<()> {
        // Messages are deleted by ON DELETE CASCADE from the tasks FK
        self.conn().execute("DELETE FROM tasks WHERE project_id = ?1", params![project_id])?;
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

    pub fn get_messages_for_task(&self, task_id: &str) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, task_id, role, content_json, created_at, sort_order, turn_usage_json
             FROM messages WHERE task_id = ?1 ORDER BY sort_order"
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
    ) -> Result<()> {
        // Preserves summary/cost/status if this row already exists (e.g. the
        // spawn event arrives after a CostUpdate for the same agent — unlikely
        // but defensive).
        self.conn().execute(
            "INSERT INTO subagent_records (task_id, agent_id, model, prompt, status)
             VALUES (?1, ?2, ?3, ?4, 'running')
             ON CONFLICT(task_id, agent_id) DO UPDATE SET
               model = excluded.model,
               prompt = excluded.prompt,
               updated_at = datetime('now')",
            params![task_id, agent_id, model, prompt],
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
            params![task_id, agent_id, input_tokens, output_tokens, cache_read_tokens, cost_usd],
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

    pub fn update_subagent_error(
        &self,
        task_id: &str,
        agent_id: &str,
        error: &str,
    ) -> Result<()> {
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

    pub fn get_subagent_records_for_task(&self, task_id: &str) -> Result<Vec<SubagentRecord>> {
        let mut stmt = self.conn().prepare(
            "SELECT task_id, agent_id, model, prompt, summary, status,
                    input_tokens, output_tokens, cache_read_tokens, cost_usd,
                    error, created_at, updated_at
             FROM subagent_records
             WHERE task_id = ?1
             ORDER BY created_at ASC"
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
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
