use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{MessageRow, TaskRow};

impl Database {
    pub fn insert_task(&self, task: &TaskRow) -> Result<()> {
        self.conn().execute(
            "INSERT INTO tasks (id, project_id, title, status, provider_type, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task.id, task.project_id, task.title, task.status,
                task.provider_type, task.model, task.created_at, task.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<TaskRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, title, status, provider_type, model, created_at, updated_at
             FROM tasks WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                provider_type: row.get(4)?,
                model: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_tasks_for_project(&self, project_id: &str) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, title, status, provider_type, model, created_at, updated_at
             FROM tasks WHERE project_id = ?1 ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                provider_type: row.get(4)?,
                model: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
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
