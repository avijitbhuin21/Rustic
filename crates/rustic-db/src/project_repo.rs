use crate::error::Result;
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

    /// Ensure a project exists in the DB and return the actual project ID.
    /// If a project with the same root_path already exists (possibly with a
    /// different ID), return that existing ID instead of inserting a duplicate.
    pub fn ensure_project(&self, project: &ProjectRow) -> Result<String> {
        // Check by root_path first (handles ID mismatch after app restart)
        if let Some(existing) = self.get_project_by_path(&project.root_path)? {
            return Ok(existing.id);
        }
        // No existing row — insert
        self.conn().execute(
            "INSERT OR IGNORE INTO projects (id, name, root_path, created_at, settings_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project.id, project.name, project.root_path, project.created_at, project.settings_json],
        )?;
        Ok(project.id.clone())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<ProjectRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, name, root_path, created_at, settings_json FROM projects WHERE id = ?1",
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
        let mut stmt = self.conn().prepare_cached(
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
        // Archived projects are removed-from-workspace; they keep their task
        // history but must not rehydrate into the workspace on startup.
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, name, root_path, created_at, settings_json FROM projects WHERE archived = 0 ORDER BY created_at"
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
        self.conn()
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Soft-remove / restore a project. Archiving hides it from the workspace
    /// (and from `list_projects`) WITHOUT deleting its tasks/messages, so
    /// re-adding the same folder restores the history. Restoring (archived =
    /// false) is what `add_project` calls when a previously-removed folder is
    /// added back.
    pub fn set_project_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET archived = ?1 WHERE id = ?2",
            params![archived as i64, id],
        )?;
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
