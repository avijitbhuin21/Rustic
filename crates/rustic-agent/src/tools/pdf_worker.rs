//! Out-of-process PDF text extraction.
//!
//! `pdf-extract` (and its `cff-parser` font dependency) panics rather than
//! returning `Err` on some malformed embedded fonts. Release builds set
//! `panic = "abort"`, so such a panic takes the whole application down — this
//! module isolates extraction in a re-exec of our own binary so a crash costs
//! one child process and surfaces as a normal tool error.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Argv flag that puts a Rustic binary into single-shot PDF extraction mode.
pub const WORKER_FLAG: &str = "--rustic-pdf-extract";

const WORKER_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const EXIT_PARSE_FAILED: i32 = 20;
const EXIT_READ_FAILED: i32 = 21;

/// Why an out-of-process extraction attempt produced no text.
#[derive(Debug)]
pub enum PdfExtractError {
    /// The PDF itself was rejected by the parser.
    Parse(String),
    /// The child died on a panic/abort or was killed for running too long.
    WorkerCrashed(String),
    /// We could not even get a child process going.
    WorkerUnavailable(String),
}

/// Run single-shot extraction and exit if this process was spawned as a PDF
/// worker; returns immediately otherwise. Call first thing in `main`.
pub fn run_worker_if_requested() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = match args.get(1).map(String::as_str) {
        Some(WORKER_FLAG) => match (args.get(2), args.get(3)) {
            (Some(i), Some(o)) => (i.clone(), o.clone()),
            _ => {
                eprintln!("{WORKER_FLAG} requires <input-pdf> <output-txt>");
                std::process::exit(EXIT_READ_FAILED);
            }
        },
        _ => return,
    };

    let bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(EXIT_READ_FAILED);
        }
    };

    match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => {
            let write = std::fs::File::create(&output)
                .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.flush()));
            match write {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(EXIT_READ_FAILED);
                }
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(EXIT_PARSE_FAILED);
        }
    }
}

/// Extract all text from a PDF in a child process, tolerating parser panics.
pub fn extract_text(pdf_path: &Path) -> Result<String, PdfExtractError> {
    // Under `cargo test` the current exe is the libtest harness, which cannot
    // serve as a worker, so error-mapping is exercised in-process instead.
    if cfg!(test) {
        return extract_in_process(pdf_path);
    }

    let exe = std::env::current_exe()
        .map_err(|e| PdfExtractError::WorkerUnavailable(format!("current_exe failed: {e}")))?;

    let out_path = std::env::temp_dir().join(format!(
        "rustic-pdf-{}-{}.txt",
        std::process::id(),
        unique_token()
    ));
    let _guard = TempFile(out_path.clone());

    let mut cmd = Command::new(exe);
    cmd.arg(WORKER_FLAG)
        .arg(pdf_path)
        .arg(&out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| PdfExtractError::WorkerUnavailable(format!("spawn failed: {e}")))?;

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if started.elapsed() > WORKER_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PdfExtractError::WorkerCrashed(format!(
                        "extraction exceeded {}s and was terminated",
                        WORKER_TIMEOUT.as_secs()
                    )));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                return Err(PdfExtractError::WorkerUnavailable(format!(
                    "wait failed: {e}"
                )))
            }
        }
    };

    let stderr = child
        .stderr
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            use std::io::Read;
            let _ = s.read_to_string(&mut buf);
            buf
        })
        .unwrap_or_default();
    let detail = stderr.trim().to_string();

    match status.code() {
        Some(0) => std::fs::read_to_string(&out_path).map_err(|e| {
            PdfExtractError::WorkerCrashed(format!("worker reported success but output unreadable: {e}"))
        }),
        Some(EXIT_PARSE_FAILED) => Err(PdfExtractError::Parse(if detail.is_empty() {
            "unsupported or corrupt PDF structure".to_string()
        } else {
            detail
        })),
        Some(EXIT_READ_FAILED) => Err(PdfExtractError::WorkerUnavailable(if detail.is_empty() {
            "worker could not read the file".to_string()
        } else {
            detail
        })),
        other => Err(PdfExtractError::WorkerCrashed(format!(
            "worker terminated abnormally (exit {}){}",
            other
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))),
    }
}

/// Same extraction, in this process — test-only fallback for the worker path.
fn extract_in_process(pdf_path: &Path) -> Result<String, PdfExtractError> {
    let bytes = std::fs::read(pdf_path)
        .map_err(|e| PdfExtractError::WorkerUnavailable(format!("read failed: {e}")))?;
    pdf_extract::extract_text_from_mem(&bytes).map_err(|e| PdfExtractError::Parse(e.to_string()))
}

/// Cheap unique token so concurrent extractions never share a temp path.
fn unique_token() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64),
    );
    h.finish()
}

struct TempFile(std::path::PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}
