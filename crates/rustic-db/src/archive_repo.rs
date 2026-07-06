use crate::connection::Database;
use crate::error::Result;
use crate::models::ArchivedMessageRow;
use rusqlite::params;

impl Database {
    /// Append one condense generation of dropped messages to the task's archive.
    pub fn append_archived_messages(&self, task_id: &str, rows: &[(String, String)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn().unchecked_transaction()?;
        let generation: i64 = tx.query_row(
            "SELECT COALESCE(MAX(generation), 0) + 1 FROM archived_messages WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )?;
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        for (slot, (role, content_json)) in rows.iter().enumerate() {
            tx.execute(
                "INSERT INTO archived_messages (task_id, generation, slot, role, content_json, created_at)\n                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![task_id, generation, slot as i64, role, content_json, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Fetch every archived message for a task, ordered by (generation, slot).
    pub fn get_archived_messages(&self, task_id: &str) -> Result<Vec<ArchivedMessageRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT task_id, generation, slot, role, content_json, created_at\n             FROM archived_messages WHERE task_id = ?1 ORDER BY generation, slot",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(ArchivedMessageRow {
                task_id: row.get(0)?,
                generation: row.get(1)?,
                slot: row.get(2)?,
                role: row.get(3)?,
                content_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_task() -> Database {
        let db = Database::in_memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO projects (id, name, root_path) VALUES ('p1', 'p', '/tmp');\n                 INSERT INTO tasks (id, project_id, title, provider_type, model) VALUES ('t1', 'p1', 't', 'Claude', 'm');",
            )
            .unwrap();
        db
    }

    #[test]
    fn append_assigns_incrementing_generations_and_slots() {
        let db = db_with_task();
        db.append_archived_messages(
            "t1",
            &[
                ("user".into(), "[]".into()),
                ("assistant".into(), "[]".into()),
            ],
        )
        .unwrap();
        db.append_archived_messages("t1", &[("assistant".into(), "[]".into())])
            .unwrap();
        let rows = db.get_archived_messages("t1").unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.generation, r.slot))
                .collect::<Vec<_>>(),
            vec![(1, 0), (1, 1), (2, 0)]
        );
    }

    #[test]
    fn empty_append_is_noop_and_cascade_deletes_with_task() {
        let db = db_with_task();
        db.append_archived_messages("t1", &[]).unwrap();
        assert!(db.get_archived_messages("t1").unwrap().is_empty());
        db.append_archived_messages("t1", &[("user".into(), "[]".into())])
            .unwrap();
        db.delete_task("t1").unwrap();
        assert!(db.get_archived_messages("t1").unwrap().is_empty());
    }
}
