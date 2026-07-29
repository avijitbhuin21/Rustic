//! Session capture for the Antigravity CLI (`agy`).
//!
//! Unlike Claude Code and Codex, `agy` has no hook system, so there is nothing
//! to register and nothing that calls back. What it does have is a per
//! conversation SQLite file under `~/.gemini/antigravity-cli/conversations/`,
//! whose stem is the conversation id accepted by `agy --conversation <id>`.
//!
//! Capture therefore works by difference: snapshot the directory before
//! spawning, then watch for the file that appears. Everything here is
//! best-effort and read-only — Rustic never writes to Antigravity's store, and a
//! failed read just leaves the session without a title.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// Antigravity step kind whose payload holds the user's prompt.
const STEP_TYPE_USER_PROMPT: i64 = 14;

/// A conversation discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// `cascade_id` — the handle `agy --conversation` expects.
    pub id: String,
    pub path: PathBuf,
    pub title: Option<String>,
}

/// Default store location: `~/.gemini/antigravity-cli/conversations`.
pub fn conversations_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(
        PathBuf::from(home)
            .join(".gemini")
            .join("antigravity-cli")
            .join("conversations"),
    )
}

/// Conversation ids currently present in `dir` (file stems of every `.db`).
pub fn snapshot_ids(dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "db"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .collect()
}

/// The newest conversation in `dir` that wasn't in `before`.
pub fn find_new_conversation(dir: &Path, before: &HashSet<String>) -> Option<Conversation> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .filter(|p| {
            p.file_stem()
                .map(|s| !before.contains(s.to_string_lossy().as_ref()))
                .unwrap_or(false)
        })
        .map(|p| {
            let mtime = p
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (mtime, p)
        })
        .collect();
    candidates.sort_by_key(|(mtime, _)| *mtime);
    let (_, newest) = candidates.pop()?;
    read_conversation(&newest)
}

/// Read a conversation's id and (if the first turn has landed) its title.
///
/// Falls back to the file stem when `trajectory_meta` can't be read — the stem
/// *is* the cascade id, so a resume handle survives even a schema change.
pub fn read_conversation(path: &Path) -> Option<Conversation> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let conn = open_read_only(path);

    let id = conn
        .as_ref()
        .and_then(|c| {
            c.query_row("SELECT cascade_id FROM trajectory_meta LIMIT 1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or(stem);

    let title = conn.as_ref().and_then(read_title);

    Some(Conversation {
        id,
        path: path.to_path_buf(),
        title,
    })
}

/// Open an external SQLite file without disturbing its owner. When `agy` holds
/// locks that block even a read-only open, fall back to reading a copy.
fn open_read_only(path: &Path) -> Option<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    if let Ok(conn) = Connection::open_with_flags(path, flags) {
        // Touch the schema so a half-written file is rejected here rather than
        // surfacing as a confusing error later.
        if conn
            .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_ok()
        {
            return Some(conn);
        }
    }
    let tmp = std::env::temp_dir().join(format!("rustic-agy-{}.db", uuid::Uuid::new_v4().simple()));
    std::fs::copy(path, &tmp).ok()?;
    let conn = Connection::open_with_flags(&tmp, flags).ok();
    let _ = std::fs::remove_file(&tmp);
    conn
}

/// Extract the first user prompt from the `steps` table.
///
/// `step_payload` is an opaque protobuf blob; the prompt is the human text
/// inside it, so we take the longest printable run. This is a heuristic on an
/// undocumented format by design — it only ever affects the displayed title, and
/// returning `None` is a supported outcome.
fn read_title(conn: &Connection) -> Option<String> {
    let payload: Vec<u8> = conn
        .query_row(
            "SELECT step_payload FROM steps WHERE step_type = ?1 ORDER BY rowid LIMIT 1",
            [STEP_TYPE_USER_PROMPT],
            |r| r.get(0),
        )
        .ok()?;
    let text = longest_printable_run(&payload)?;
    super::hooks::title_from_prompt(&text)
}

/// Longest run of printable ASCII (plus tabs/newlines) in a byte blob.
fn longest_printable_run(bytes: &[u8]) -> Option<String> {
    let mut best: &[u8] = &[];
    let mut start = 0usize;
    let printable = |b: u8| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t';

    for i in 0..=bytes.len() {
        let is_end = i == bytes.len() || !printable(bytes[i]);
        if is_end {
            if i - start > best.len() {
                best = &bytes[start..i];
            }
            start = i + 1;
        }
    }
    let text = String::from_utf8_lossy(best).trim().to_string();
    (text.chars().count() >= 2).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let p =
                std::env::temp_dir().join(format!("rustic-agy-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Build a stand-in for an Antigravity conversation file: the two tables and
    /// columns this module reads, with the prompt embedded in a protobuf-like
    /// blob (length-prefix bytes and a trailing field tag around the text).
    fn write_conversation(dir: &Path, id: &str, prompt: Option<&str>) -> PathBuf {
        let path = dir.join(format!("{id}.db"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE trajectory_meta (cascade_id TEXT);
             CREATE TABLE steps (step_type INTEGER, step_payload BLOB);",
        )
        .unwrap();
        conn.execute("INSERT INTO trajectory_meta VALUES (?1)", [id])
            .unwrap();
        if let Some(prompt) = prompt {
            let mut blob: Vec<u8> = vec![0x0a, 0x08, 0x01, 0x12];
            blob.extend_from_slice(prompt.as_bytes());
            blob.extend_from_slice(&[0x00, 0x18, 0x02]);
            conn.execute(
                "INSERT INTO steps VALUES (?1, ?2)",
                rusqlite::params![STEP_TYPE_USER_PROMPT, blob],
            )
            .unwrap();
        }
        drop(conn);
        path
    }

    #[test]
    fn reads_cascade_id_and_prompt() {
        let dir = Dir::new();
        let path = write_conversation(&dir.0, "conv-1", Some("wire up the parser"));
        let c = read_conversation(&path).unwrap();
        assert_eq!(c.id, "conv-1");
        assert_eq!(c.title.as_deref(), Some("wire up the parser"));
    }

    #[test]
    fn missing_prompt_yields_no_title_but_still_an_id() {
        let dir = Dir::new();
        let path = write_conversation(&dir.0, "conv-2", None);
        let c = read_conversation(&path).unwrap();
        assert_eq!(c.id, "conv-2");
        assert!(c.title.is_none());
    }

    #[test]
    fn unreadable_file_falls_back_to_the_file_stem() {
        let dir = Dir::new();
        let path = dir.0.join("conv-3.db");
        std::fs::write(&path, b"not a database").unwrap();
        let c = read_conversation(&path).unwrap();
        assert_eq!(c.id, "conv-3");
        assert!(c.title.is_none());
    }

    #[test]
    fn only_conversations_absent_from_the_snapshot_are_new() {
        let dir = Dir::new();
        write_conversation(&dir.0, "old", Some("old prompt"));
        let before = snapshot_ids(&dir.0);
        assert!(before.contains("old"));
        assert!(find_new_conversation(&dir.0, &before).is_none());

        write_conversation(&dir.0, "fresh", Some("new prompt"));
        let found = find_new_conversation(&dir.0, &before).unwrap();
        assert_eq!(found.id, "fresh");
        assert_eq!(found.title.as_deref(), Some("new prompt"));
    }

    #[test]
    fn printable_run_picks_the_human_text() {
        let mut blob = vec![0x08, 0x96, 0x01, 0x12];
        blob.extend_from_slice(b"hello there world");
        blob.extend_from_slice(&[0x00, 0xff, 0x01]);
        assert_eq!(
            longest_printable_run(&blob).as_deref(),
            Some("hello there world")
        );
        assert!(longest_printable_run(&[0x00, 0x01, 0x02]).is_none());
    }
}
