use crate::error::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::SettingRow;

impl Database {
    pub fn set_setting(&self, key: &str, value_json: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO user_settings (key, value_json, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value_json = ?2, updated_at = datetime('now')",
            params![key, value_json],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT value_json FROM user_settings WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
        match rows.next() {
            Some(val) => Ok(Some(val?)),
            None => Ok(None),
        }
    }

    pub fn get_all_settings(&self) -> Result<Vec<SettingRow>> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT key, value_json, updated_at FROM user_settings ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok(SettingRow {
                key: row.get(0)?,
                value_json: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM user_settings WHERE key = ?1", params![key])?;
        Ok(())
    }
}
