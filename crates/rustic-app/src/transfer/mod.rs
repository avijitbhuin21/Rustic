//! Streamed, chunked, parallel and resumable sync transfers.
//!
//! The archive is split into fixed-size chunks, each carrying a SHA-256.
//! * The SENDER writes the archive into a [`Growing`] file while it is being
//!   built; chunks go out as soon as they are complete (pack ‖ send).
//! * The RECEIVER writes each verified chunk at its offset ([`Assembler`], any
//!   order) and exposes the contiguous prefix as a [`Growing`] file that the
//!   extractor reads while chunks are still arriving (receive ‖ extract).
//! * Several chunks travel concurrently; a failed chunk is retried alone, and
//!   the receiver reports which chunks it already has so nothing is re-sent.

pub mod client;
pub mod routes;
pub mod session;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Chunk size on the wire. Small enough that sending starts quickly while the
/// archive is still being packed; big enough that per-request overhead is noise.
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Number of chunks for an archive of `total` bytes.
pub fn chunk_count(total: u64) -> u64 {
    total.div_ceil(CHUNK_SIZE)
}

#[derive(Default)]
struct GrowState {
    committed: u64,
    done: bool,
    error: Option<String>,
}

/// A file that is appended to while other threads read it. `committed` is how
/// many leading bytes are final and safe to read.
pub struct Growing {
    path: PathBuf,
    state: Mutex<GrowState>,
    cv: Condvar,
}

impl Growing {
    /// Track `path` (the caller creates / writes the file).
    pub fn new(path: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_path_buf(),
            state: Mutex::new(GrowState::default()),
            cv: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GrowState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Path of the underlying file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Advance the readable prefix to `len` bytes (never moves backwards).
    pub fn commit(&self, len: u64) {
        let mut s = self.lock();
        if len > s.committed {
            s.committed = len;
            self.cv.notify_all();
        }
    }

    /// No more bytes will be added.
    pub fn finish(&self) {
        let mut s = self.lock();
        s.done = true;
        self.cv.notify_all();
    }

    /// Abort: readers get `msg` as an I/O error.
    pub fn fail(&self, msg: impl Into<String>) {
        let mut s = self.lock();
        if s.error.is_none() {
            s.error = Some(msg.into());
        }
        self.cv.notify_all();
    }

    /// `(committed, done, error)`.
    pub fn snapshot(&self) -> (u64, bool, Option<String>) {
        let s = self.lock();
        (s.committed, s.done, s.error.clone())
    }

    /// Block until at least `need` bytes are committed, the file is done, it
    /// failed, or `timeout` passes. Returns the latest snapshot.
    pub fn wait_for(&self, need: u64, timeout: Duration) -> (u64, bool, Option<String>) {
        let mut s = self.lock();
        let deadline = std::time::Instant::now() + timeout;
        while s.committed < need && !s.done && s.error.is_none() {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            s = self
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        (s.committed, s.done, s.error.clone())
    }

    /// Create the file and return a writer that commits as it writes.
    pub fn writer(self: &Arc<Self>) -> std::io::Result<GrowingWriter> {
        Ok(GrowingWriter {
            file: File::create(&self.path)?,
            grow: Arc::clone(self),
            written: 0,
        })
    }

    /// Reader over the committed prefix that blocks until more arrives.
    pub fn reader(self: &Arc<Self>) -> std::io::Result<GrowingReader> {
        Ok(GrowingReader {
            file: File::open(&self.path)?,
            grow: Arc::clone(self),
            pos: 0,
        })
    }

    /// Read `[offset, offset+len)` (caller ensures it is committed).
    pub fn read_range(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Sequential writer; every write extends the committed prefix.
pub struct GrowingWriter {
    file: File,
    grow: Arc<Growing>,
    written: u64,
}

impl Write for GrowingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(buf)?;
        self.written += n as u64;
        self.grow.commit(self.written);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Sequential reader over a [`Growing`] file.
pub struct GrowingReader {
    file: File,
    grow: Arc<Growing>,
    pos: u64,
}

impl Read for GrowingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let (committed, done, error) = self.grow.wait_for(self.pos + 1, Duration::from_secs(5));
            if self.pos < committed {
                let want = (committed - self.pos).min(buf.len() as u64) as usize;
                let n = self.file.read(&mut buf[..want])?;
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(e) = error {
                return Err(std::io::Error::other(e));
            }
            if done {
                return Ok(0);
            }
        }
    }
}

/// Receiver side: places verified chunks at their offsets in any order and
/// commits the contiguous prefix to its [`Growing`] file.
pub struct Assembler {
    grow: Arc<Growing>,
    file: Mutex<File>,
    inner: Mutex<AsmState>,
}

#[derive(Default)]
struct AsmState {
    received: BTreeSet<u64>,
    /// Byte length of each received chunk (only the last may be short).
    lens: std::collections::HashMap<u64, u64>,
    /// First chunk index not yet part of the contiguous prefix.
    next_contiguous: u64,
    contiguous_bytes: u64,
    total_chunks: Option<u64>,
    total_size: Option<u64>,
}

impl Assembler {
    /// Create the target file at `path`.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(Self {
            grow: Growing::new(path),
            file: Mutex::new(file),
            inner: Mutex::new(AsmState::default()),
        })
    }

    /// The growing view of the contiguous prefix (hand its reader to the extractor).
    pub fn growing(&self) -> Arc<Growing> {
        Arc::clone(&self.grow)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, AsmState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Verify and store chunk `index`. Re-sending an already stored chunk is a no-op.
    pub fn put(&self, index: u64, data: &[u8], sha256: &str) -> Result<(), String> {
        if sha256_hex(data) != sha256.to_ascii_lowercase() {
            return Err(format!("chunk {index} failed its checksum — resend it"));
        }
        if data.len() as u64 > CHUNK_SIZE || data.is_empty() {
            return Err(format!(
                "chunk {index} has an invalid size ({} bytes)",
                data.len()
            ));
        }
        {
            let st = self.state();
            if st.received.contains(&index) {
                return Ok(());
            }
            if let Some(total) = st.total_chunks {
                if index >= total {
                    return Err(format!("chunk {index} is past the end ({total} chunks)"));
                }
            }
        }
        {
            let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
            f.seek(SeekFrom::Start(index * CHUNK_SIZE))
                .map_err(|e| e.to_string())?;
            f.write_all(data).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
        let mut st = self.state();
        st.received.insert(index);
        st.lens.insert(index, data.len() as u64);
        self.advance(&mut st);
        Ok(())
    }

    /// Extend the contiguous prefix over newly completed chunks.
    fn advance(&self, st: &mut AsmState) {
        while st.received.contains(&st.next_contiguous) {
            let len = st.lens[&st.next_contiguous];
            st.contiguous_bytes += len;
            st.next_contiguous += 1;
            if len < CHUNK_SIZE {
                break; // a short chunk can only be the last one
            }
        }
        self.grow.commit(st.contiguous_bytes);
        if let (Some(total), Some(size)) = (st.total_chunks, st.total_size) {
            if st.next_contiguous >= total && st.contiguous_bytes == size {
                self.grow.finish();
            }
        }
    }

    /// The sender declared the final size. Completes once every chunk is in.
    pub fn set_total(&self, total_size: u64) {
        let mut st = self.state();
        st.total_size = Some(total_size);
        st.total_chunks = Some(chunk_count(total_size));
        self.advance(&mut st);
    }

    /// Chunk indices already stored (for resume).
    pub fn received(&self) -> Vec<u64> {
        self.state().received.iter().copied().collect()
    }

    /// Chunk indices still missing once the total is known.
    pub fn missing(&self) -> Vec<u64> {
        let st = self.state();
        match st.total_chunks {
            Some(total) => (0..total).filter(|i| !st.received.contains(i)).collect(),
            None => Vec::new(),
        }
    }

    /// Whether every chunk has arrived and the prefix covers the whole file.
    pub fn is_complete(&self) -> bool {
        self.grow.snapshot().1
    }

    /// Abort the transfer (the extractor sees an error).
    pub fn fail(&self, msg: impl Into<String>) {
        self.grow.fail(msg);
    }
}
