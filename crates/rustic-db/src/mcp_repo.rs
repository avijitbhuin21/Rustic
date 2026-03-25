use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::McpServerRow;

impl Database {
    pub fn insert_mcp_server(&self, server: &McpServerRow) -> Result<()> {
        self.conn().execute(
            "INSERT INTO mcp_servers (id, name, transport_type, config_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                server.id, server.name, server.transport_type,
                server.config_json, server.enabled, server.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<McpServerRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, transport_type, config_json, enabled, created_at
             FROM mcp_servers ORDER BY created_at"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(McpServerRow {
                id: row.get(0)?,
                name: row.get(1)?,
                transport_type: row.get(2)?,
                config_json: row.get(3)?,
                enabled: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_mcp_server(&self, id: &str) -> Result<Option<McpServerRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, transport_type, config_json, enabled, created_at
             FROM mcp_servers WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(McpServerRow {
                id: row.get(0)?,
                name: row.get(1)?,
                transport_type: row.get(2)?,
                config_json: row.get(3)?,
                enabled: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn update_mcp_server_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE mcp_servers SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(())
    }

    pub fn delete_mcp_server(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
        Ok(())
    }
}
