use crate::error::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::ProjectRow;

impl Database {
    pub fn insert_project(&self, project: &ProjectRow) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO projects (id, name, root_path, created_at, settings_json, sort_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project.id, project.name, project.root_path, project.created_at, project.settings_json, project.sort_order],
        )?;
        Ok(())
    }

    /// Rewrite a project's on-disk root path (used by cloud sync when an
    /// imported environment's projects land at different local paths).
    pub fn update_project_root(&self, id: &str, root_path: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET root_path = ?2 WHERE id = ?1",
            params![id, root_path],
        )?;
        Ok(())
    }

    /// Best-effort prefix rewrite of file-history index paths after a project
    /// root moved (cloud sync import). Separators are normalized to `/` for
    /// the comparison so Windows- and Unix-recorded paths both match.
    pub fn rewrite_file_history_prefix(&self, old_prefix: &str, new_prefix: &str) -> Result<()> {
        let old_norm = old_prefix.replace('\\', "/");
        let new_norm = new_prefix.replace('\\', "/");
        self.conn().execute(
            "UPDATE OR IGNORE file_history_files
             SET path = REPLACE(REPLACE(path, '\\', '/'), ?1, ?2)
             WHERE REPLACE(path, '\\', '/') LIKE ?3",
            params![old_norm, new_norm, format!("{old_norm}%")],
        )?;
        Ok(())
    }

    /// Next `sort_order` value to append a project after all existing ones.
    pub fn next_project_sort_order(&self) -> Result<i64> {
        let next = self.conn().query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(next)
    }

    /// Persist a full drag-drop reordering: `ordered_ids` is the new order of
    /// project ids, written back as 0-based `sort_order` values in one
    /// transaction. Ids not present are left untouched.
    pub fn reorder_projects(&self, ordered_ids: &[String]) -> Result<()> {
        let conn = self.conn();
        conn.execute_batch("BEGIN")?;
        for (i, id) in ordered_ids.iter().enumerate() {
            conn.execute(
                "UPDATE projects SET sort_order = ?1 WHERE id = ?2",
                params![i as i64, id],
            )?;
        }
        conn.execute_batch("COMMIT")?;
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
            "INSERT OR IGNORE INTO projects (id, name, root_path, created_at, settings_json, sort_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project.id, project.name, project.root_path, project.created_at, project.settings_json, project.sort_order],
        )?;
        Ok(project.id.clone())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<ProjectRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, name, root_path, created_at, settings_json, sort_order FROM projects WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
                sort_order: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_project_by_path(&self, root_path: &str) -> Result<Option<ProjectRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT id, name, root_path, created_at, settings_json, sort_order FROM projects WHERE root_path = ?1"
        )?;
        let mut rows = stmt.query_map(params![root_path], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
                sort_order: row.get(5)?,
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
            "SELECT id, name, root_path, created_at, settings_json, sort_order FROM projects WHERE archived = 0 ORDER BY sort_order, created_at"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                created_at: row.get(3)?,
                settings_json: row.get(4)?,
                sort_order: row.get(5)?,
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
