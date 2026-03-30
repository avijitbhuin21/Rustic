use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::ProjectRow;

impl Database {
    pub fn insert_project(&self, project: &ProjectRow) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO projects (id, name, root_path, created_at, settings_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project.id, project.name, project.root_path, project.created_at, project.settings_json],
        )?;
        Ok(())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<ProjectRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, root_path, created_at, settings_json FROM projects WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_project_by_path(&self, root_path: &str) -> Result<Option<ProjectRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, root_path, created_at, settings_json FROM projects WHERE root_path = ?1"
        )?;
        let mut rows = stmt.query_map(params![root_path], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, root_path, created_at, settings_json FROM projects ORDER BY created_at"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_project_settings(&self, id: &str, settings_json: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET settings_json = ?1 WHERE id = ?2",
            params![settings_json, id],
        )?;
        Ok(())
    }
}
