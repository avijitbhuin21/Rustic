use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path` atomically: write to a sibling temp file, fsync,
/// then rename over the destination.
///
/// On Windows std::fs::rename uses MoveFileEx with MOVEFILE_REPLACE_EXISTING,
/// so this works cross-platform as long as the temp lives on the same filesystem
/// (we always place it in the same parent directory).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(
        ".{}.tmp.{}.{}",
        file_name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writing to a fresh path creates the file with the exact bytes.
    #[test]
    fn atomic_write_creates_file() {
        let dir = std::env::temp_dir().join(format!("rustic-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        atomic_write(&p, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Overwriting an existing file replaces its contents (atomic rename).
    #[test]
    fn atomic_write_overwrites() {
        let dir = std::env::temp_dir().join(format!("rustic-tests-overwrite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.txt");
        std::fs::write(&p, "old").unwrap();
        atomic_write(&p, b"new content").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new content");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// No leftover .tmp files remain after a successful write. The temp file
    /// is always renamed onto the target.
    #[test]
    fn atomic_write_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("rustic-tests-tmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.txt");
        atomic_write(&p, b"data").unwrap();
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftover.is_empty(), "found leftover tmp files: {:?}", leftover);
        std::fs::remove_dir_all(&dir).ok();
    }
}
