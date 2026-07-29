//! One-time migration and periodic upkeep for the image media store.
//!
//! Databases created before the store existed keep image payloads as inline
//! base64 inside `messages.content_json` — on the machine this was built for
//! that was 203 MB of a 462 MB file, and every task open paid for it. [`backfill`]
//! rewrites those rows to `path` references; [`prune`] drops store files no
//! message references any more.
//!
//! Both walk the whole `messages` table, so hosts run them once in the
//! background at startup, never on a UI path.

use crate::media_store;
use crate::provider::ContentBlock;
use rustic_db::Database;

/// Rows scanned per query. Keeps peak memory to a few MB even when a single
/// message carries several 5 MB screenshots.
const BATCH: i64 = 64;

#[derive(Debug, Default, Clone, Copy)]
pub struct BackfillStats {
    pub messages_rewritten: usize,
    pub images_moved: usize,
    /// How much smaller the rewritten `content_json` rows are in total. The
    /// file itself only shrinks once the pages are reclaimed by a VACUUM.
    pub json_bytes_saved: u64,
}

/// Move every inline image payload in the database into the media store.
/// Idempotent: rows already holding a `path` are skipped, so this can run on
/// every startup and does nothing once converted.
pub fn backfill(db: &Database) -> BackfillStats {
    let mut stats = BackfillStats::default();
    if media_store::root().is_none() {
        return stats;
    }
    let mut after = String::new();
    loop {
        let rows = match db.messages_with_images(&after, BATCH) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("[media_maintenance] scan failed: {e}");
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for (id, content_json) in &rows {
            after = id.clone();
            let Ok(mut content) = serde_json::from_str::<Vec<ContentBlock>>(content_json) else {
                // A row the current enum can't parse (older shape, or the
                // fallback raw-text row) is left exactly as it is.
                continue;
            };
            let moved = media_store::dehydrate(&mut content);
            if moved == 0 {
                continue;
            }
            let Ok(rewritten) = serde_json::to_string(&content) else {
                continue;
            };
            if let Err(e) = db.set_message_content(id, &rewritten) {
                tracing::warn!("[media_maintenance] rewrite failed for {id}: {e}");
                continue;
            }
            stats.messages_rewritten += 1;
            stats.images_moved += moved;
            stats.json_bytes_saved += content_json
                .len()
                .saturating_sub(rewritten.len())
                .try_into()
                .unwrap_or(0);
        }
        if (rows.len() as i64) < BATCH {
            break;
        }
    }
    if stats.messages_rewritten > 0 {
        tracing::info!(
            messages = stats.messages_rewritten,
            images = stats.images_moved,
            saved_mb = stats.json_bytes_saved / (1024 * 1024),
            "[media_maintenance] moved inline image payloads to the media store"
        );
    }
    stats
}

/// Delete store files no message references. Runs after [`backfill`] so newly
/// written payloads are already visible in the table.
///
/// Skipped entirely when a scan error prevents building a complete reference
/// set — deleting on a partial set would destroy live payloads.
pub fn prune(db: &Database) -> (usize, u64) {
    if media_store::root().is_none() {
        return (0, 0);
    }
    let mut keep = std::collections::HashSet::new();
    let mut after = String::new();
    loop {
        let rows = match db.messages_with_images(&after, 512) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("[media_maintenance] prune aborted — scan failed: {e}");
                return (0, 0);
            }
        };
        if rows.is_empty() {
            break;
        }
        let count = rows.len();
        for (id, content_json) in rows {
            after = id;
            if let Ok(content) = serde_json::from_str::<Vec<ContentBlock>>(&content_json) {
                keep.extend(media_store::referenced_names(&[content]));
            } else {
                // Can't tell what this row references — refuse to prune rather
                // than risk deleting a payload it still points at.
                tracing::warn!("[media_maintenance] prune aborted — unparseable message content");
                return (0, 0);
            }
        }
        if count < 512 {
            break;
        }
    }
    let (files, bytes) = media_store::prune_orphans(&keep);
    if files > 0 {
        tracing::info!(
            files,
            mb = bytes / (1024 * 1024),
            "[media_maintenance] pruned orphaned media payloads"
        );
    }
    (files, bytes)
}

/// Backfill, prune, then reclaim the freed pages. VACUUM needs exclusive access
/// and rewrites the whole file, so it only runs when the backfill actually
/// moved something — otherwise every startup would rewrite a 460 MB file.
pub fn run_startup_maintenance(db: &Database) {
    let stats = backfill(db);
    prune(db);
    if stats.messages_rewritten == 0 {
        return;
    }
    match db.vacuum() {
        Ok(()) => tracing::info!("[media_maintenance] vacuum complete"),
        Err(e) => tracing::warn!("[media_maintenance] vacuum failed: {e}"),
    }
}
