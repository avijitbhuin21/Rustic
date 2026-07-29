//! Content-addressed on-disk store for the image payloads carried by chat
//! messages.
//!
//! Image blocks used to be persisted as inline base64 inside
//! `messages.content_json`. That made images 62% of a 460 MB database and every
//! task open had to read, JSON-parse and re-serialize the whole blob before the
//! transcript could render. Payloads now live in files named by the SHA-256 of
//! their decoded bytes; the message keeps `media_type` + `path`.
//!
//! Storing decoded bytes (not the base64 text) halves the on-disk size, and
//! content addressing dedupes the same screenshot pasted into several tasks.
//!
//! The store is a process-global because the persist and request-build paths
//! that need it are far apart and neither carries host configuration. Hosts call
//! [`init`] once at startup; when it is never called every function degrades to
//! a no-op and blocks keep their inline base64, so the agent still works with no
//! store configured (tests, embedded uses).

use crate::provider::{ContentBlock, Message};
use base64::Engine as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Point the media store at `root` (conventionally `<app_data>/media`).
/// First call wins; later calls are ignored so a second host in the same
/// process can't repoint it mid-flight.
pub fn init(root: PathBuf) {
    if let Err(existing) = ROOT.set(root) {
        tracing::debug!(
            "[media_store] init ignored — already rooted at {}",
            existing.display()
        );
    }
}

/// The configured root, or `None` when no host has called [`init`].
pub fn root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

/// A store file name is exactly a 64-char lowercase hex digest. Validating
/// before touching the filesystem keeps a crafted `path` from a tampered DB
/// row (or a malicious cloud-sync archive) from escaping the store directory.
fn is_valid_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn path_for(name: &str) -> Option<PathBuf> {
    if !is_valid_name(name) {
        tracing::warn!("[media_store] rejecting malformed media name: {name:?}");
        return None;
    }
    root().map(|r| r.join(name))
}

/// Write `b64`'s decoded bytes into the store, returning the content-addressed
/// file name. `None` means "keep the payload inline" — no root configured, or
/// the base64 / write failed.
pub fn store_base64(b64: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .ok()?;
    store_bytes(&bytes)
}

/// Write raw payload bytes into the store, returning the content-addressed file
/// name. `None` means "keep the payload inline" — no root configured, or the
/// write failed.
pub fn store_bytes(bytes: &[u8]) -> Option<String> {
    let root = root()?;
    if bytes.is_empty() {
        return None;
    }

    use sha2::{Digest, Sha256};
    let name = hex::encode(Sha256::digest(bytes));
    let path = root.join(&name);

    // Content-addressed: identical bytes always produce the same name, so an
    // existing file is already the payload we were about to write.
    if path.exists() {
        return Some(name);
    }
    if let Err(e) = std::fs::create_dir_all(root) {
        tracing::warn!("[media_store] cannot create {}: {e}", root.display());
        return None;
    }
    // Write to a temp file then rename so a crash mid-write can't leave a
    // truncated file sitting at a name that claims to hold those bytes.
    let tmp = root.join(format!("{name}.tmp"));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        tracing::warn!("[media_store] write failed for {name}: {e}");
        return None;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        tracing::warn!("[media_store] rename failed for {name}: {e}");
        return None;
    }
    Some(name)
}

/// Build an `Image` block from base64, moving the payload into the store when
/// one is configured. Ingestion points use this so base64 never enters the
/// in-memory history or `content_json`; `hydrate_messages` refills it for each
/// provider request.
pub fn image_block(media_type: String, b64: &str) -> ContentBlock {
    match store_base64(b64) {
        Some(name) => ContentBlock::Image {
            media_type,
            data: String::new(),
            path: Some(name),
        },
        None => ContentBlock::Image {
            media_type,
            data: b64.to_string(),
            path: None,
        },
    }
}

/// Serialize a message's content for `messages.content_json`, moving any
/// still-inline image payload into the store first.
///
/// Ingestion already stores payloads, so the inline case only arises for
/// history loaded from rows written before the store existed. The scan is a
/// cheap `is_empty` check per block and the clone only happens when there is
/// something to move, so this stays free on the hot persist path.
pub fn content_json(content: &[ContentBlock]) -> serde_json::Result<String> {
    let has_inline = content.iter().any(|b| {
        matches!(b, ContentBlock::Image { data, path, .. } if !data.is_empty() && path.is_none())
    });
    if !has_inline {
        return serde_json::to_string(content);
    }
    let mut owned = content.to_vec();
    dehydrate(&mut owned);
    serde_json::to_string(&owned)
}

/// Read a stored payload back as base64, or `None` if it's missing.
pub fn load_base64(name: &str) -> Option<String> {
    let path = path_for(name)?;
    match std::fs::read(&path) {
        Ok(bytes) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        Err(e) => {
            tracing::warn!("[media_store] missing payload {name}: {e}");
            None
        }
    }
}

/// Byte size of a stored payload, for UI stubs that show the size without
/// loading the image.
pub fn payload_len(name: &str) -> Option<u64> {
    let path = path_for(name)?;
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Move every inline image payload in `content` into the store, replacing
/// `data` with a `path`. Returns the number of blocks moved. Blocks whose
/// payload can't be stored are left untouched, so this is always safe to call
/// before persisting.
pub fn dehydrate(content: &mut [ContentBlock]) -> usize {
    let mut moved = 0;
    for block in content.iter_mut() {
        if let ContentBlock::Image {
            data,
            path,
            media_type: _,
        } = block
        {
            if data.is_empty() || path.is_some() {
                continue;
            }
            if let Some(name) = store_base64(data) {
                *path = Some(name);
                data.clear();
                moved += 1;
            }
        }
    }
    moved
}

/// Refill inline `data` for every image block that only carries a `path`.
/// Returns the number of blocks hydrated. A payload that has gone missing from
/// disk leaves `data` empty rather than failing the whole turn — providers skip
/// empty image blocks and the rest of the conversation still goes through.
pub fn hydrate(content: &mut [ContentBlock]) -> usize {
    let mut filled = 0;
    for block in content.iter_mut() {
        if let ContentBlock::Image { data, path, .. } = block {
            if !data.is_empty() {
                continue;
            }
            let Some(name) = path.as_deref() else {
                continue;
            };
            if let Some(b64) = load_base64(name) {
                *data = b64;
                filled += 1;
            }
        }
    }
    filled
}

/// [`hydrate`] across a whole conversation. Called once per turn before the
/// messages reach a provider.
pub fn hydrate_messages(messages: &mut [Message]) -> usize {
    messages.iter_mut().map(|m| hydrate(&mut m.content)).sum()
}

/// [`dehydrate`] across a whole conversation.
pub fn dehydrate_messages(messages: &mut [Message]) -> usize {
    messages.iter_mut().map(|m| dehydrate(&mut m.content)).sum()
}

/// Every payload name currently referenced by any image block in `contents`.
/// Used by the maintenance sweep to tell live payloads from orphans.
pub fn referenced_names(contents: &[Vec<ContentBlock>]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for content in contents {
        for block in content {
            if let ContentBlock::Image { path: Some(p), .. } = block {
                out.insert(p.clone());
            }
        }
    }
    out
}

/// Delete store files not present in `keep`. Returns (files removed, bytes
/// reclaimed). Only well-formed names are considered, so unrelated files
/// dropped in the directory are never touched.
pub fn prune_orphans(keep: &std::collections::HashSet<String>) -> (usize, u64) {
    let Some(root) = root() else {
        return (0, 0);
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return (0, 0);
    };
    let mut files = 0;
    let mut bytes = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_valid_name(&name) || keep.contains(&name) {
            continue;
        }
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(entry.path()).is_ok() {
            files += 1;
            bytes += len;
        }
    }
    (files, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_b64() -> String {
        base64::engine::general_purpose::STANDARD.encode([137, 80, 78, 71, 13, 10, 26, 10])
    }

    #[test]
    fn rejects_names_that_could_escape_the_store() {
        assert!(!is_valid_name("../../secret"));
        assert!(!is_valid_name("abc"));
        assert!(!is_valid_name(&"A".repeat(64)));
        assert!(is_valid_name(&"a1".repeat(32)));
    }

    #[test]
    fn dehydrate_is_a_noop_without_a_root() {
        // ROOT is process-global and other tests may have set it; this asserts
        // the shape that matters either way — a block is never left with both
        // an empty `data` and no `path`.
        let mut content = vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: png_b64(),
            path: None,
        }];
        dehydrate(&mut content);
        match &content[0] {
            ContentBlock::Image { data, path, .. } => {
                assert!(!data.is_empty() || path.is_some());
            }
            _ => panic!("expected image block"),
        }
    }

    #[test]
    fn round_trips_through_a_temp_root() {
        let dir = tempfile::tempdir().unwrap();
        // Bypass the OnceLock so this test is independent of ordering.
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        let bytes = [137u8, 80, 78, 71, 13, 10, 26, 10];
        use sha2::{Digest, Sha256};
        let name = hex::encode(Sha256::digest(bytes));
        std::fs::write(root.join(&name), bytes).unwrap();
        assert!(is_valid_name(&name));
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(b64, png_b64());
    }
}
