use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{MessageRow, TaskRow};

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
            "INSERT OR REPLACE INTO messages (id, task_id, role, content_json, created_at, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![msg.id, msg.task_id, msg.role, msg.content_json, msg.created_at, msg.sort_order],
        )?;
        Ok(())
    }

    pub fn delete_messages_for_task(&self, task_id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM messages WHERE task_id = ?1", params![task_id])?;
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
            "INSERT INTO messages (id, task_id, role, content_json, created_at, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![msg.id, msg.task_id, msg.role, msg.content_json, msg.created_at, msg.sort_order],
        )?;
        Ok(())
    }

    pub fn get_messages_for_task(&self, task_id: &str) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, task_id, role, content_json, created_at, sort_order
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
}
