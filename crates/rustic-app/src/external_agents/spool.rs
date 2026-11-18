//! Consumer for the hook spool directory.
//!
//! Hook processes drop one JSON file per event (see [`super::hooks`]); this
//! module folds those files into the `external_agent_sessions` table and
//! deletes them. It is deliberately poll-based rather than filesystem-watched:
//! the volume is a handful of files per conversation, and polling can't miss an
//! event that landed while nothing was listening — a crash or restart simply
//! picks the backlog up on the next pass.

use std::path::Path;

use rustic_db::Database;

use super::activity;
use super::hooks::{pty_key_from_spool_name, title_from_prompt, HookPayload};

/// Outcome of one drain pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainOutcome {
    /// Hook events successfully folded into the database.
    pub applied: usize,
    /// Files skipped because they didn't belong to a known session yet. They
    /// are left in place for a later pass.
    pub deferred: usize,
    /// Row ids whose session id or title changed, so the host can tell the UI.
    pub touched_sessions: Vec<String>,
    /// Row ids that gained activity rows in this pass.
    pub activity_sessions: Vec<String>,
}

impl DrainOutcome {
    pub fn changed(&self) -> bool {
        !self.touched_sessions.is_empty()
    }
}

/// Fold every pending hook payload in `spool` into the database.
///
/// A payload whose `pty_key` has no matching row is *deferred*, not discarded:
/// a hook can fire before the host has finished inserting the session row, and
/// dropping it would lose the only disclosure of that conversation's id.
///
/// With `activity_dir`, events the activity list shows are also appended to the
/// session's log there.
pub fn drain_spool(db: &Database, spool: &Path, activity_dir: Option<&Path>) -> DrainOutcome {
    let mut outcome = DrainOutcome::default();
    let Ok(entries) = std::fs::read_dir(spool) else {
        return outcome;
    };

    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    for entry in files {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        let Some(pty_key) = pty_key_from_spool_name(&name) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|raw| {
                let payload = serde_json::from_value::<HookPayload>(raw.clone()).ok()?;
                Some((raw, payload))
            });
        let Some((raw, payload)) = parsed else {
            tracing::debug!(target: "rustic::external_agents", file = %name, "spool: unparseable payload discarded");
            let _ = std::fs::remove_file(&path);
            continue;
        };

        match apply_payload(db, &pty_key, &payload) {
            Ok(Some((session_id, row_changed))) => {
                outcome.applied += 1;
                if let (Some(dir), Some(event)) =
                    (activity_dir, activity::normalize(&raw, activity::now_ms()))
                {
                    match activity::append(dir, &session_id, &event) {
                        Ok(()) => {
                            if !outcome.activity_sessions.contains(&session_id) {
                                outcome.activity_sessions.push(session_id.clone());
                            }
                        }
                        Err(e) => {
                            tracing::warn!(target: "rustic::external_agents", "activity: append failed: {e}")
                        }
                    }
                }
                if row_changed && !outcome.touched_sessions.contains(&session_id) {
                    outcome.touched_sessions.push(session_id);
                }
                let _ = std::fs::remove_file(&path);
            }
            Ok(None) => {
                // Row not registered yet — retry on the next pass.
                outcome.deferred += 1;
            }
            Err(e) => {
                tracing::warn!(target: "rustic::external_agents", file = %name, "spool: apply failed: {e}");
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    outcome
}

/// Apply one payload. Returns the affected row id and whether the session list
/// needs refreshing, or `None` when no row is registered for this `pty_key` yet.
///
/// Tool and approval events arrive many times a turn and change nothing the
/// session list shows, so they must not trigger a list refresh each time.
fn apply_payload(
    db: &Database,
    pty_key: &str,
    payload: &HookPayload,
) -> rustic_db::Result<Option<(String, bool)>> {
    let before = db.get_external_agent_session_by_pty_key(pty_key)?;
    let row_id = match payload.session_id.as_deref() {
        Some(external_id) if !external_id.is_empty() => db.attach_external_agent_session_id(
            pty_key,
            external_id,
            payload.transcript_path.as_deref(),
        )?,
        // No id in the payload: still worth marking the session as alive.
        _ => match db.get_external_agent_session_by_pty_key(pty_key)? {
            Some(row) => Some(row.id),
            None => None,
        },
    };

    let Some(row_id) = row_id else {
        return Ok(None);
    };

    if let Some(title) = payload.prompt.as_deref().and_then(title_from_prompt) {
        db.set_external_agent_session_title(pty_key, &title)?;
    }
    db.touch_external_agent_session(pty_key)?;

    let session_event = matches!(
        payload.hook_event_name.as_deref(),
        None | Some("SessionStart" | "UserPromptSubmit")
    );
    let after = db.get_external_agent_session(&row_id)?;
    let row_changed = session_event
        || match (&before, &after) {
            (Some(b), Some(a)) => {
                b.id != a.id
                    || b.external_session_id != a.external_session_id
                    || b.title != a.title
                    || b.transcript_path != a.transcript_path
            }
            _ => true,
        };
    Ok(Some((row_id, row_changed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spool {
        dir: std::path::PathBuf,
    }

    impl Spool {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("rustic-spool-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn write(&self, pty_key: &str, payload: serde_json::Value) {
            let name = format!("{pty_key}-{}.json", uuid::Uuid::new_v4().simple());
            std::fs::write(self.dir.join(name), payload.to_string()).unwrap();
        }

        fn count(&self) -> usize {
            std::fs::read_dir(&self.dir).unwrap().flatten().count()
        }
    }

    impl Drop for Spool {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn db_with_session(pty_key: &str) -> Database {
        let db = Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO projects (id, name, root_path) VALUES ('p1','P','/tmp/p1')",
                [],
            )
            .unwrap();
        db.create_external_agent_session("s1", "p1", "claude", pty_key, "/tmp/p1")
            .unwrap();
        db
    }

    fn key() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    #[test]
    fn session_start_then_prompt_fills_id_title_and_transcript() {
        let k = key();
        let db = db_with_session(&k);
        let spool = Spool::new();
        spool.write(
            &k,
            serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": "ext-1",
                "transcript_path": "/t.jsonl"
            }),
        );
        // Distinct mtimes so the drain order is the emission order.
        std::thread::sleep(std::time::Duration::from_millis(20));
        spool.write(
            &k,
            serde_json::json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "ext-1",
                "prompt": "refactor the parser\nplease"
            }),
        );

        let outcome = drain_spool(&db, &spool.dir, None);
        assert_eq!(outcome.applied, 2);
        assert_eq!(outcome.deferred, 0);
        assert_eq!(outcome.touched_sessions, vec!["s1".to_string()]);
        assert_eq!(spool.count(), 0, "consumed files are deleted");

        let row = db.get_external_agent_session("s1").unwrap().unwrap();
        assert_eq!(row.external_session_id.as_deref(), Some("ext-1"));
        assert_eq!(row.transcript_path.as_deref(), Some("/t.jsonl"));
        assert_eq!(row.title.as_deref(), Some("refactor the parser"));
    }

    #[test]
    fn payload_for_unknown_pty_key_is_deferred_not_dropped() {
        let db = db_with_session(&key());
        let spool = Spool::new();
        spool.write(
            &key(),
            serde_json::json!({ "hook_event_name": "SessionStart", "session_id": "ext-9" }),
        );

        let outcome = drain_spool(&db, &spool.dir, None);
        assert_eq!(outcome.applied, 0);
        assert_eq!(outcome.deferred, 1);
        assert!(!outcome.changed());
        assert_eq!(spool.count(), 1, "kept for a later pass");
    }

    #[test]
    fn junk_files_are_discarded() {
        let db = db_with_session(&key());
        let spool = Spool::new();
        std::fs::write(spool.dir.join("garbage.json"), "not json").unwrap();
        let k = key();
        std::fs::write(spool.dir.join(format!("{k}-x.json")), "{ broken").unwrap();

        let outcome = drain_spool(&db, &spool.dir, None);
        assert_eq!(outcome.applied, 0);
        assert_eq!(spool.count(), 0);
    }

    #[test]
    fn tool_events_land_in_the_activity_log_without_refreshing_the_list() {
        let k = key();
        let db = db_with_session(&k);
        let spool = Spool::new();
        let activity_dir = spool.dir.join("activity");
        spool.write(
            &k,
            serde_json::json!({ "hook_event_name": "UserPromptSubmit", "session_id": "e", "prompt": "list files" }),
        );
        let first = drain_spool(&db, &spool.dir, Some(&activity_dir));
        assert_eq!(first.touched_sessions, vec!["s1".to_string()]);
        assert_eq!(first.activity_sessions, vec!["s1".to_string()]);

        spool.write(
            &k,
            serde_json::json!({
                "hook_event_name": "PostToolUse",
                "session_id": "e",
                "tool_name": "Bash",
                "tool_use_id": "t1",
                "tool_input": { "command": "ls" }
            }),
        );
        // Antigravity's dialect carries no session id at all.
        std::thread::sleep(std::time::Duration::from_millis(20));
        spool.write(
            &k,
            serde_json::json!({
                "hookEventName": "PostToolUse",
                "toolCall": { "id": "c1", "name": "read_file", "args": { "Path": "/tmp/p1/a.txt" } }
            }),
        );
        let second = drain_spool(&db, &spool.dir, Some(&activity_dir));
        assert_eq!(second.applied, 2);
        assert!(
            !second.changed(),
            "tool events must not refresh the session list"
        );
        assert_eq!(second.activity_sessions, vec!["s1".to_string()]);

        let rows = activity::read(&activity_dir, "s1", 50);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, activity::ActivityKind::Prompt);
        assert_eq!(rows[1].text.as_deref(), Some("ls"));
        assert_eq!(rows[2].paths, vec!["/tmp/p1/a.txt".to_string()]);
    }

    #[test]
    fn antigravity_payloads_supply_the_conversation_id_title_and_transcript() {
        let k = key();
        let db = db_with_session(&k);
        let spool = Spool::new();
        spool.write(
            &k,
            serde_json::json!({
                "hookEventName": "PreInvocation",
                "invocationNum": 0,
                "conversationId": "conv-1",
                "transcriptPath": "/brain/conv-1/transcript_full.jsonl",
                "lastUserInput": "read the notes"
            }),
        );
        let outcome = drain_spool(&db, &spool.dir, None);
        assert_eq!(outcome.touched_sessions, vec!["s1".to_string()]);
        let row = db.get_external_agent_session("s1").unwrap().unwrap();
        assert_eq!(row.external_session_id.as_deref(), Some("conv-1"));
        assert_eq!(row.title.as_deref(), Some("read the notes"));
        assert_eq!(
            row.transcript_path.as_deref(),
            Some("/brain/conv-1/transcript_full.jsonl")
        );
    }

    #[test]
    fn first_prompt_wins_as_the_title() {
        let k = key();
        let db = db_with_session(&k);
        let spool = Spool::new();
        spool.write(
            &k,
            serde_json::json!({ "hook_event_name": "UserPromptSubmit", "session_id": "e", "prompt": "first" }),
        );
        drain_spool(&db, &spool.dir, None);
        spool.write(
            &k,
            serde_json::json!({ "hook_event_name": "UserPromptSubmit", "session_id": "e", "prompt": "second" }),
        );
        drain_spool(&db, &spool.dir, None);

        let row = db.get_external_agent_session("s1").unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some("first"));
    }
}
