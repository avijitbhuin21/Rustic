//! Upload / download sessions for streamed, chunked sync transfers. Shared by
//! rustic-server and the desktop LAN listener (see [`super::routes`]).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Assembler, Growing, CHUNK_SIZE};

/// Consumes the (compressed) archive stream on the receiving side.
pub type ApplyFn = Box<dyn FnOnce(Box<dyn Read + Send>) -> Result<Value, String> + Send>;
/// Produces the (compressed) archive stream on the sending side.
pub type BuildFn = Box<dyn FnOnce(Box<dyn Write + Send>) -> Result<Value, String> + Send>;

/// Idle sessions are dropped after this long (the transfer can resume until then).
pub const SESSION_TTL: Duration = Duration::from_secs(60 * 60);

/// Result slot a worker thread fills once apply / build ends.
#[derive(Default)]
struct Outcome {
    result: Mutex<Option<Result<Value, String>>>,
    cv: Condvar,
}

impl Outcome {
    fn set(&self, r: Result<Value, String>) {
        *self.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
        self.cv.notify_all();
    }

    fn peek(&self) -> Option<Result<Value, String>> {
        self.result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn wait(&self, timeout: Duration) -> Option<Result<Value, String>> {
        let g = self.result.lock().unwrap_or_else(|p| p.into_inner());
        let (g, _) = self
            .cv
            .wait_timeout_while(g, timeout, |r| r.is_none())
            .unwrap_or_else(|p| p.into_inner());
        g.clone()
    }
}

struct UploadSession {
    asm: Assembler,
    outcome: Arc<Outcome>,
    touched: Mutex<Instant>,
    path: PathBuf,
}

struct DownloadSession {
    grow: Arc<Growing>,
    outcome: Arc<Outcome>,
    touched: Mutex<Instant>,
    path: PathBuf,
}

/// Reply to a chunk download request.
pub enum ChunkReply {
    /// Chunk bytes + their SHA-256.
    Data(Vec<u8>, String),
    /// The archive ended before this index.
    End,
    /// Not built yet — ask again.
    NotReady,
}

/// All live transfer sessions of one host.
pub struct SessionManager {
    dir: PathBuf,
    uploads: Mutex<HashMap<String, Arc<UploadSession>>>,
    downloads: Mutex<HashMap<String, Arc<DownloadSession>>>,
}

impl SessionManager {
    /// Sessions keep their partial files under `dir`. Starts a reaper thread
    /// that expires sessions idle for longer than [`SESSION_TTL`].
    pub fn new(dir: PathBuf) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&dir);
        let mgr = Arc::new(Self {
            dir,
            uploads: Mutex::default(),
            downloads: Mutex::default(),
        });
        let weak: Weak<Self> = Arc::downgrade(&mgr);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            match weak.upgrade() {
                Some(m) => m.expire_idle(SESSION_TTL),
                None => break,
            }
        });
        mgr
    }

    /// Drop sessions untouched for `ttl`: fail their streams and delete files.
    pub fn expire_idle(&self, ttl: Duration) {
        let stale_up: Vec<(String, Arc<UploadSession>)> = {
            let map = self.uploads.lock().unwrap_or_else(|p| p.into_inner());
            map.iter()
                .filter(|(_, s)| s.touched.lock().map(|t| t.elapsed() > ttl).unwrap_or(true))
                .map(|(k, s)| (k.clone(), Arc::clone(s)))
                .collect()
        };
        for (id, s) in stale_up {
            s.asm.fail("transfer expired (no data for too long)");
            self.uploads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            let _ = std::fs::remove_file(&s.path);
        }
        let stale_down: Vec<(String, Arc<DownloadSession>)> = {
            let map = self.downloads.lock().unwrap_or_else(|p| p.into_inner());
            map.iter()
                .filter(|(_, s)| s.touched.lock().map(|t| t.elapsed() > ttl).unwrap_or(true))
                .map(|(k, s)| (k.clone(), Arc::clone(s)))
                .collect()
        };
        for (id, s) in stale_down {
            self.downloads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            let _ = std::fs::remove_file(&s.path);
        }
    }

    fn upload(&self, id: &str) -> Result<Arc<UploadSession>, String> {
        let s = self
            .uploads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or("unknown or expired upload — start the sync again")?;
        *s.touched.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        Ok(s)
    }

    fn download(&self, id: &str) -> Result<Arc<DownloadSession>, String> {
        let s = self
            .downloads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or("unknown or expired download — start the sync again")?;
        *s.touched.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        Ok(s)
    }

    /// Start receiving an archive; `apply` runs immediately on a worker thread,
    /// reading chunks as they arrive (extraction overlaps the upload).
    pub fn begin_upload(&self, apply: ApplyFn) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.dir.join(format!("up-{id}.part"));
        let asm = Assembler::create(&path).map_err(|e| e.to_string())?;
        let reader = asm.growing().reader().map_err(|e| e.to_string())?;
        let outcome = Arc::new(Outcome::default());
        let out = Arc::clone(&outcome);
        std::thread::spawn(move || out.set(apply(Box::new(reader))));
        let session = Arc::new(UploadSession {
            asm,
            outcome,
            touched: Mutex::new(Instant::now()),
            path,
        });
        self.uploads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), session);
        Ok(id)
    }

    /// Store one uploaded chunk (idempotent). Fails fast if applying already failed.
    pub fn put_chunk(&self, id: &str, index: u64, data: &[u8], sha256: &str) -> Result<(), String> {
        let s = self.upload(id)?;
        if let Some(Err(e)) = s.outcome.peek() {
            return Err(format!("the receiving side stopped: {e}"));
        }
        s.asm.put(index, data, sha256)
    }

    /// Chunk indices already received (resume).
    pub fn upload_status(&self, id: &str) -> Result<Vec<u64>, String> {
        Ok(self.upload(id)?.asm.received())
    }

    /// Declare the final size and wait for the apply result. Errors if chunks
    /// are still missing (the client resends them and calls finish again).
    pub fn finish_upload(&self, id: &str, total_size: u64) -> Result<Value, String> {
        let s = self.upload(id)?;
        s.asm.set_total(total_size);
        let missing = s.asm.missing();
        if !missing.is_empty() {
            return Err(format!(
                "missing chunks: {:?}",
                &missing[..missing.len().min(16)]
            ));
        }
        let result = loop {
            if let Some(r) = s.outcome.wait(Duration::from_secs(5)) {
                break r;
            }
            *s.touched.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        };
        self.uploads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        let _ = std::fs::remove_file(&s.path);
        result
    }

    /// Start producing an archive; `build` runs on a worker thread and chunks
    /// can be fetched as soon as they are written (packing overlaps the download).
    pub fn begin_download(&self, build: BuildFn) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.dir.join(format!("down-{id}.part"));
        let grow = Growing::new(&path);
        let writer = grow.writer().map_err(|e| e.to_string())?;
        let outcome = Arc::new(Outcome::default());
        let (g, out) = (Arc::clone(&grow), Arc::clone(&outcome));
        std::thread::spawn(move || {
            let r = build(Box::new(writer));
            match &r {
                Ok(_) => g.finish(),
                Err(e) => g.fail(e.clone()),
            }
            out.set(r);
        });
        let session = Arc::new(DownloadSession {
            grow,
            outcome,
            touched: Mutex::new(Instant::now()),
            path,
        });
        self.downloads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), session);
        Ok(id)
    }

    /// Serve chunk `index`, waiting up to `wait` for it to be built.
    pub fn download_chunk(
        &self,
        id: &str,
        index: u64,
        wait: Duration,
    ) -> Result<ChunkReply, String> {
        let s = self.download(id)?;
        let start = index * CHUNK_SIZE;
        let end = start + CHUNK_SIZE;
        let (committed, done, error) = s.grow.wait_for(end, wait);
        if let Some(e) = error {
            return Err(e);
        }
        if committed >= end {
            let data = s
                .grow
                .read_range(start, CHUNK_SIZE)
                .map_err(|e| e.to_string())?;
            let sha = super::sha256_hex(&data);
            return Ok(ChunkReply::Data(data, sha));
        }
        if done {
            if start >= committed {
                return Ok(ChunkReply::End);
            }
            let data = s
                .grow
                .read_range(start, committed - start)
                .map_err(|e| e.to_string())?;
            let sha = super::sha256_hex(&data);
            return Ok(ChunkReply::Data(data, sha));
        }
        Ok(ChunkReply::NotReady)
    }

    /// `(bytes built so far, finished, error, build result when finished)`.
    pub fn download_status(
        &self,
        id: &str,
    ) -> Result<(u64, bool, Option<String>, Option<Value>), String> {
        let s = self.download(id)?;
        let (committed, done, error) = s.grow.snapshot();
        let result = match s.outcome.peek() {
            Some(Ok(v)) => Some(v),
            _ => None,
        };
        Ok((committed, done, error, result))
    }

    /// Client finished downloading: drop the session and its file.
    pub fn finish_download(&self, id: &str) {
        if let Some(s) = self
            .downloads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
        {
            let _ = std::fs::remove_file(&s.path);
        }
    }
}
