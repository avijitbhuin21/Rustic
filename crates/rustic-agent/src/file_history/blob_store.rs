//! Content-addressed blob store for the changed-files tracker.
//!
//! Layout: `{root}/{hash[..2]}/{hash}` where `hash` is lowercase sha256 hex.
//! All operations are best-effort idempotent — writing the same content twice
//! collapses to one blob on disk. Streaming hash + atomic rename means we
//! never hold whole-file content in memory and partially-written blobs never
//! become visible.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

const HASH_HEX_LEN: usize = 64;
/// Files larger than this are not hashed/stored. The snapshot records the path
/// with a synthetic marker so revert leaves them alone. Tunable; matches the
/// value documented in the design memo.
pub const MAX_TRACKED_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB

#[derive(Debug, Error)]
pub enum BlobStoreError {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("file too large to track: {path} ({size} bytes, limit {limit})")]
    TooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },

    #[error("invalid blob hash {0:?}")]
    InvalidHash(String),
}

pub type Result<T> = std::result::Result<T, BlobStoreError>;

/// Result of storing a blob: its content hash + size in bytes + a flag
/// telling the caller whether the blob was already on disk (so the index
/// row may already exist; the caller can skip the SQL register call if so).
#[derive(Debug, Clone)]
pub struct StoredBlob {
    pub hash: String,
    pub size: u64,
    pub already_present: bool,
}

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Create a `BlobStore` rooted at `{config_dir}/file-history/blobs/`.
    /// The root directory is created lazily on first write.
    pub fn new(config_dir: &Path) -> Self {
        Self {
            root: config_dir.join("file-history").join("blobs"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path where a blob with the given hash lives. Does not check existence.
    pub fn path_for(&self, hash: &str) -> Result<PathBuf> {
        if !is_valid_hash(hash) {
            return Err(BlobStoreError::InvalidHash(hash.to_string()));
        }
        Ok(self.root.join(&hash[..2]).join(hash))
    }

    /// True if a blob with this hash exists on disk.
    pub fn exists(&self, hash: &str) -> Result<bool> {
        Ok(self.path_for(hash)?.is_file())
    }

    /// Stream-hash `src`, store into the content-addressed location atomically,
    /// and return the (hash, size, already_present) tuple. Files exceeding
    /// `MAX_TRACKED_FILE_SIZE` are rejected with `TooLarge` — the caller is
    /// expected to record a synthetic `<too_large>` marker in the snapshot.
    ///
    /// Atomic-rename guarantee: blob is hashed into a temp file in the same
    /// shard directory, then renamed onto the final path. No partially-written
    /// blob is ever visible to readers.
    pub fn store_from_path(&self, src: &Path) -> Result<StoredBlob> {
        let metadata = fs::metadata(src)?;
        let size = metadata.len();
        if size > MAX_TRACKED_FILE_SIZE {
            return Err(BlobStoreError::TooLarge {
                path: src.to_path_buf(),
                size,
                limit: MAX_TRACKED_FILE_SIZE,
            });
        }

        // Hash-first: stream `src` through SHA-256 once, finalize the hash,
        // and only then decide whether to copy the bytes onto disk. The
        // open_snapshot pre-capture path hits the "blob already present"
        // branch for ~every unchanged file; under the previous strategy
        // (hash + write-to-temp simultaneously) those would have all paid
        // a full file-sized write that was then discarded. Skipping the
        // write halves the I/O for the common case at the cost of a second
        // file read on cache miss — acceptable because misses are rare.
        let mut hasher = Sha256::new();
        {
            let mut reader = File::open(src)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
        let hash = hex::encode(hasher.finalize());

        let final_path = self.path_for(&hash)?;
        if final_path.is_file() {
            return Ok(StoredBlob {
                hash,
                size,
                already_present: true,
            });
        }

        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let shard_root = self.ensure_shard_root_for_temp()?;
        let mut tmp = tempfile::NamedTempFile::new_in(&shard_root)?;
        {
            let mut reader = File::open(src)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                tmp.as_file_mut().write_all(&buf[..n])?;
            }
        }
        tmp.as_file_mut().flush()?;
        // persist() does an atomic rename; fall back to a copy on EXDEV (cross
        // filesystem). NamedTempFile is created in the same shard root so the
        // rename should always work, but defensive.
        match tmp.persist(&final_path) {
            Ok(_) => {}
            Err(persist_err) => {
                // PersistError holds the temp file; copy it then drop.
                let mut from = persist_err.file.reopen()?;
                let mut to = File::create(&final_path)?;
                io::copy(&mut from, &mut to)?;
            }
        }

        Ok(StoredBlob {
            hash,
            size,
            already_present: false,
        })
    }

    /// Unlink a blob file. Idempotent — missing file is not an error.
    pub fn delete(&self, hash: &str) -> Result<()> {
        let path = self.path_for(hash)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Restore a blob to a destination path. Used by the revert path.
    /// Creates parent directories as needed.
    pub fn restore_to(&self, hash: &str, dest: &Path) -> Result<()> {
        let blob_path = self.path_for(hash)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&blob_path, dest)?;
        Ok(())
    }

    /// List every blob file currently on disk (returns hashes). Used by the
    /// startup reconciliation pass to find orphans not in the index.
    pub fn list_all_hashes(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                if let Some(name) = entry.file_name().to_str() {
                    if is_valid_hash(name) {
                        out.push(name.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    /// We need the sharded subdirectory to exist before NamedTempFile can be
    /// created in it. We don't yet know the hash, so use a fixed scratch dir
    /// under the root and rename at the end.
    fn ensure_shard_root_for_temp(&self) -> Result<PathBuf> {
        let scratch = self.root.join(".tmp");
        fs::create_dir_all(&scratch)?;
        Ok(scratch)
    }
}

fn is_valid_hash(hash: &str) -> bool {
    hash.len() == HASH_HEX_LEN && hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).expect("create file");
        f.write_all(content).expect("write content");
        p
    }

    #[test]
    fn round_trip_store_then_read() {
        let cfg = fixture_dir();
        let store = BlobStore::new(cfg.path());

        let work = fixture_dir();
        let src = write_file(work.path(), "hello.txt", b"hello world");

        let stored = store.store_from_path(&src).unwrap();
        assert_eq!(stored.size, 11);
        assert!(!stored.already_present);
        assert!(store.exists(&stored.hash).unwrap());

        // Re-storing identical content is idempotent.
        let again = store.store_from_path(&src).unwrap();
        assert_eq!(again.hash, stored.hash);
        assert!(again.already_present);

        // Restore.
        let dest_dir = fixture_dir();
        let dest = dest_dir.path().join("nested").join("out.txt");
        store.restore_to(&stored.hash, &dest).unwrap();
        let got = fs::read(&dest).unwrap();
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn delete_is_idempotent() {
        let cfg = fixture_dir();
        let store = BlobStore::new(cfg.path());

        let work = fixture_dir();
        let src = write_file(work.path(), "a", b"abc");
        let stored = store.store_from_path(&src).unwrap();

        store.delete(&stored.hash).unwrap();
        assert!(!store.exists(&stored.hash).unwrap());
        // Second delete is a no-op, not an error.
        store.delete(&stored.hash).unwrap();
    }

    #[test]
    fn rejects_files_over_size_cap() {
        let cfg = fixture_dir();
        let store = BlobStore::new(cfg.path());

        // Build a file just over the cap.
        let work = fixture_dir();
        let big = work.path().join("big");
        let mut f = File::create(&big).unwrap();
        let chunk = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        while written <= MAX_TRACKED_FILE_SIZE {
            f.write_all(&chunk).unwrap();
            written += chunk.len() as u64;
        }
        drop(f);

        let err = store.store_from_path(&big).unwrap_err();
        match err {
            BlobStoreError::TooLarge { size, limit, .. } => {
                assert!(size > MAX_TRACKED_FILE_SIZE);
                assert_eq!(limit, MAX_TRACKED_FILE_SIZE);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn list_all_hashes_finds_stored_blobs() {
        let cfg = fixture_dir();
        let store = BlobStore::new(cfg.path());

        let work = fixture_dir();
        let h1 = store
            .store_from_path(&write_file(work.path(), "a", b"alpha"))
            .unwrap()
            .hash;
        let h2 = store
            .store_from_path(&write_file(work.path(), "b", b"beta"))
            .unwrap()
            .hash;

        let mut listed = store.list_all_hashes().unwrap();
        listed.sort();
        let mut expected = vec![h1, h2];
        expected.sort();
        assert_eq!(listed, expected);
    }

    #[test]
    fn invalid_hash_rejected() {
        let cfg = fixture_dir();
        let store = BlobStore::new(cfg.path());
        assert!(matches!(
            store.path_for("nope"),
            Err(BlobStoreError::InvalidHash(_))
        ));
        assert!(matches!(
            store.path_for("ABCDEF"),
            Err(BlobStoreError::InvalidHash(_))
        ));
    }
}
