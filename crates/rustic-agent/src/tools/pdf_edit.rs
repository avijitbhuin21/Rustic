//! Out-of-process PDF content editing for the PDF preview.
//!
//! The preview keeps a list of edits (move, delete, replace text). Each call
//! reloads the file, replays the whole list with `palimpsest-pdf` and returns
//! the objects of one page plus, when asked, the edited document. Nothing is
//! kept between calls, so undo is just "send the list without its last entry".
//!
//! Like text extraction (`pdf_worker`), the work runs in a re-exec of our own
//! binary: release builds set `panic = "abort"`, and a PDF parser must never be
//! able to take the application down.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use palimpsest_pdf::{Document, Rect, TextOptions};
use serde::{Deserialize, Serialize};

/// Argv flag that puts a Rustic binary into single-shot PDF edit mode.
pub const EDIT_FLAG: &str = "--rustic-pdf-edit";

const WORKER_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const EXIT_EDIT_FAILED: i32 = 20;
const EXIT_IO_FAILED: i32 = 21;
const STATE_FILE: &str = "state.json";
const PDF_FILE: &str = "out.pdf";
// Replaces the engine's wording for this case, which names its own API options.
const NEEDS_SHAPING: &str = "this text is in a script that needs shaping or right-to-left \
     layout (for example Hindi or Arabic), which the PDF editor cannot write yet";

/// One edit. Ids come from the `objects` of an earlier response for the same
/// file and the same preceding edits; coordinates are PDF points, y up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PdfEditOp {
    Delete {
        page: u32,
        id: u32,
    },
    Move {
        page: u32,
        id: u32,
        dx: f64,
        dy: f64,
    },
    Fit {
        page: u32,
        id: u32,
        rect: [f64; 4],
    },
    ReplaceLine {
        page: u32,
        id: u32,
        text: String,
    },
    ReplaceBlock {
        page: u32,
        id: u32,
        text: String,
    },
}

impl PdfEditOp {
    /// The 1-based page this edit applies to.
    fn page(&self) -> u32 {
        match self {
            PdfEditOp::Delete { page, .. }
            | PdfEditOp::Move { page, .. }
            | PdfEditOp::Fit { page, .. }
            | PdfEditOp::ReplaceLine { page, .. }
            | PdfEditOp::ReplaceBlock { page, .. } => *page,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfEditRequest {
    pub path: String,
    #[serde(default)]
    pub ops: Vec<PdfEditOp>,
    /// 1-based page whose objects are returned.
    pub page: u32,
    /// Also return the edited document.
    #[serde(default)]
    pub want_pdf: bool,
}

/// The result of replaying a request.
#[derive(Debug)]
pub struct PdfEditOutcome {
    /// JSON: `{"info":…,"objects":…,"layout":…,"notes":[…]}`, one note per edit.
    pub state: String,
    /// The edited document, when it was asked for and there were edits.
    pub pdf: Option<Vec<u8>>,
}

/// Fonts offered to the engine for characters a PDF's own fonts lack. The
/// engine bundles none; these are the metric-compatible families most
/// documents were set in. Missing files are skipped.
fn system_fonts() -> Vec<Vec<u8>> {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial Italic.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf",
        "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
        "/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf",
        "/System/Library/Fonts/Supplemental/Times New Roman Italic.ttf",
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
        "C:\\Windows\\Fonts\\ariali.ttf",
        "C:\\Windows\\Fonts\\arialbi.ttf",
        "C:\\Windows\\Fonts\\times.ttf",
        "C:\\Windows\\Fonts\\timesbd.ttf",
        "C:\\Windows\\Fonts\\timesi.ttf",
        "C:\\Windows\\Fonts\\cour.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ];
    CANDIDATES
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .collect()
}

/// Words an engine error for the user; `index` is the edit's position in the list.
fn describe(index: usize, error: &palimpsest_pdf::Error) -> String {
    match error {
        palimpsest_pdf::Error::Unsupported(reason) if reason.contains("needs shaping") => {
            NEEDS_SHAPING.to_string()
        }
        _ => format!("edit {} could not be applied: {error}", index + 1),
    }
}

/// Replays `ops` on `bytes`, in this process.
fn apply(
    bytes: &[u8],
    ops: &[PdfEditOp],
    page: u32,
    want_pdf: bool,
) -> Result<PdfEditOutcome, String> {
    let mut doc = Document::from_bytes(bytes).map_err(|e| e.to_string())?;
    let types_text = |op: &PdfEditOp| {
        matches!(
            op,
            PdfEditOp::ReplaceLine { .. } | PdfEditOp::ReplaceBlock { .. }
        )
    };
    if ops.iter().any(types_text) {
        for font in system_fonts() {
            let _ = doc.add_fallback_font(font);
        }
    }
    let placed = |p: palimpsest_pdf::Placement| {
        format!("{{\"lifted\":{},\"clipped\":{}}}", p.lifted, p.clipped)
    };
    let options = TextOptions::default();
    let mut notes = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        let note = match op {
            PdfEditOp::Delete { page, id } => doc.delete(*page, *id).map(|()| "{}".to_string()),
            PdfEditOp::Move { page, id, dx, dy } => doc.translate(*page, *id, *dx, *dy).map(placed),
            PdfEditOp::Fit { page, id, rect } => doc
                .fit(*page, *id, Rect::new(rect[0], rect[1], rect[2], rect[3]))
                .map(placed),
            PdfEditOp::ReplaceLine { page, id, text } => doc
                .replace_line(*page, *id, text, &options)
                .map(|e| e.to_json()),
            PdfEditOp::ReplaceBlock { page, id, text } => doc
                .replace_block(*page, *id, text, &options)
                .map(|e| e.to_json()),
        };
        notes.push(note.map_err(|e| describe(i, &e))?);
    }
    let objects = doc.page_objects(page).map_err(|e| e.to_string())?.to_json();
    let layout = doc.page_layout(page).map_err(|e| e.to_string())?.to_json();
    let state = format!(
        "{{\"info\":{},\"objects\":{objects},\"layout\":{layout},\"notes\":[{}]}}",
        doc.info().to_json(),
        notes.join(",")
    );
    let pdf = if want_pdf && !ops.is_empty() {
        Some(doc.save().map_err(|e| e.to_string())?)
    } else {
        None
    };
    Ok(PdfEditOutcome { state, pdf })
}

/// Run a single-shot edit and exit if this process was spawned as a PDF edit
/// worker; returns immediately otherwise.
pub fn run_worker_if_requested() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(EDIT_FLAG) {
        return;
    }
    let fail = |code: i32, message: String| -> ! {
        eprintln!("{message}");
        std::process::exit(code)
    };
    let (Some(request), Some(state_out), Some(pdf_out)) = (args.get(2), args.get(3), args.get(4))
    else {
        fail(
            EXIT_IO_FAILED,
            format!("{EDIT_FLAG} requires <request.json> <state.json> <out.pdf>"),
        )
    };
    let request: PdfEditRequest = match std::fs::read(request)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(r) => r,
        Err(e) => fail(EXIT_IO_FAILED, e),
    };
    let bytes = match std::fs::read(&request.path) {
        Ok(b) => b,
        Err(e) => fail(EXIT_IO_FAILED, e.to_string()),
    };
    let outcome = match apply(&bytes, &request.ops, request.page, request.want_pdf) {
        Ok(o) => o,
        Err(e) => fail(EXIT_EDIT_FAILED, e),
    };
    let written = outcome
        .pdf
        .map_or(Ok(()), |pdf| std::fs::write(pdf_out, pdf))
        .and_then(|()| std::fs::write(state_out, outcome.state));
    match written {
        Ok(()) => std::process::exit(0),
        Err(e) => fail(EXIT_IO_FAILED, e.to_string()),
    }
}

/// Replay a request in a child process, tolerating parser panics and hangs.
pub fn edit(request: &PdfEditRequest) -> Result<PdfEditOutcome, String> {
    // Under `cargo test` the current exe is the libtest harness, which cannot
    // serve as a worker.
    if cfg!(test) {
        let bytes = std::fs::read(&request.path).map_err(|e| e.to_string())?;
        return apply(&bytes, &request.ops, request.page, request.want_pdf);
    }
    let dir = run_worker(request)?;
    Ok(PdfEditOutcome {
        state: std::fs::read_to_string(dir.path().join(STATE_FILE)).map_err(|e| e.to_string())?,
        pdf: read_if_present(&dir.path().join(PDF_FILE))?,
    })
}

/// Replays `ops` on the PDF at `path` and writes the edited document to `dest`,
/// which may be `path` itself. Returns the number of bytes written. The
/// document goes from the worker's scratch file straight to `dest`, so its
/// size is not limited by what can be passed to the frontend and back.
pub fn edit_to_file(path: &str, ops: Vec<PdfEditOp>, dest: &Path) -> Result<u64, String> {
    let Some(page) = ops.last().map(PdfEditOp::page) else {
        return Err("there are no edits to save".to_string());
    };
    let request = PdfEditRequest {
        path: path.to_string(),
        ops,
        page,
        want_pdf: true,
    };
    if cfg!(test) {
        let bytes = std::fs::read(&request.path).map_err(|e| e.to_string())?;
        let pdf = apply(&bytes, &request.ops, request.page, true)?
            .pdf
            .ok_or("the edits produced no document")?;
        std::fs::write(dest, &pdf).map_err(|e| e.to_string())?;
        return Ok(pdf.len() as u64);
    }
    let dir = run_worker(&request)?;
    let mut from = std::fs::File::open(dir.path().join(PDF_FILE)).map_err(|e| e.to_string())?;
    // Written into the existing file (not `fs::copy`) so it keeps its permissions.
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut from, &mut to).map_err(|e| e.to_string())
}

/// Runs the worker for `request` and returns its scratch directory, which
/// holds `state.json` and, when a document was produced, `out.pdf`.
fn run_worker(request: &PdfEditRequest) -> Result<tempfile::TempDir, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let at = |name: &str| dir.path().join(name);
    let (request_path, state_path, pdf_path) = (at("request.json"), at(STATE_FILE), at(PDF_FILE));
    std::fs::write(
        &request_path,
        serde_json::to_vec(request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let mut cmd = Command::new(exe);
    cmd.arg(EDIT_FLAG)
        .args([&request_path, &state_path, &pdf_path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > WORKER_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "editing exceeded {}s and was stopped",
                    WORKER_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };
    let detail = child
        .stderr
        .take()
        .map(|mut s| {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut s, &mut buf);
            buf.trim().to_string()
        })
        .unwrap_or_default();
    match status.code() {
        Some(0) => Ok(dir),
        Some(EXIT_EDIT_FAILED) | Some(EXIT_IO_FAILED) if !detail.is_empty() => Err(detail),
        other => Err(format!(
            "the PDF editor stopped unexpectedly (exit {}){}",
            other.map_or("signal".to_string(), |c| c.to_string()),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )),
    }
}

fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FIXTURE: &[u8] = include_bytes!("testdata/pdf_edit_fixture.pdf");

    fn state(ops: &[PdfEditOp]) -> Value {
        serde_json::from_str(&apply(FIXTURE, ops, 1, false).unwrap().state).unwrap()
    }

    fn bbox(state: &Value, id: u64) -> Vec<f64> {
        let objects = state["objects"]["objects"].as_array().unwrap();
        let object = objects.iter().find(|o| o["id"] == id).unwrap();
        object["bbox"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    }

    #[test]
    fn lists_objects_without_edits() {
        let s = state(&[]);
        let kinds: Vec<&str> = s["objects"]["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["image", "text", "text"]);
        assert_eq!(bbox(&s, 1), [50.0, 600.0, 150.0, 680.0]);
        assert_eq!(s["objects"]["text_editable"], true);
        assert_eq!(s["info"]["signed"], false);
        assert_eq!(
            s["layout"]["blocks"][0]["text"],
            "First line of text Second line"
        );
        assert!(apply(FIXTURE, &[], 1, true).unwrap().pdf.is_none());
    }

    #[test]
    fn replays_edits_with_stable_ids() {
        // What the preview does: every call sends the whole list again.
        let mut ops = vec![PdfEditOp::Move {
            page: 1,
            id: 1,
            dx: 200.0,
            dy: -50.0,
        }];
        let s = state(&ops);
        assert_eq!(bbox(&s, 1), [250.0, 550.0, 350.0, 630.0]);
        // The image sat inside a clip group; the engine lifted it out so it is not cropped.
        assert_eq!(s["notes"][0]["lifted"], true);

        ops.push(PdfEditOp::ReplaceLine {
            page: 1,
            id: 2,
            text: "First line of new text".into(),
        });
        let s = state(&ops);
        assert_eq!(bbox(&s, 1), [250.0, 550.0, 350.0, 630.0]);
        assert_eq!(
            s["layout"]["blocks"][0]["lines"][0]["text"],
            "First line of new text"
        );
        assert_eq!(s["notes"][1]["missing"], "");

        ops.push(PdfEditOp::Delete { page: 1, id: 1 });
        let s = state(&ops);
        assert!(s["objects"]["objects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["kind"] == "text"));

        // Undo is the list without its last entry.
        ops.pop();
        assert_eq!(bbox(&state(&ops), 1), [250.0, 550.0, 350.0, 630.0]);

        // The saved document reopens and shows the same thing with no edits pending.
        let saved = apply(FIXTURE, &ops, 1, true).unwrap().pdf.unwrap();
        let again: Value =
            serde_json::from_str(&apply(&saved, &[], 1, false).unwrap().state).unwrap();
        assert_eq!(bbox(&again, 1), [250.0, 550.0, 350.0, 630.0]);
        assert_eq!(
            again["layout"]["blocks"][0]["lines"][0]["text"],
            "First line of new text"
        );
    }

    #[test]
    fn reports_bad_input_as_errors() {
        assert!(apply(b"not a pdf", &[], 1, false).is_err());
        assert!(apply(FIXTURE, &[], 9, false).is_err());
        let stale = [PdfEditOp::Delete { page: 1, id: 99 }];
        let err = apply(FIXTURE, &stale, 1, false).unwrap_err();
        assert!(err.contains("edit 1"), "{err}");
    }

    #[test]
    fn saves_edits_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        std::fs::write(&path, FIXTURE).unwrap();
        let source = path.to_str().unwrap();

        assert!(edit_to_file(source, Vec::new(), &path).is_err());
        let stale = vec![PdfEditOp::Delete { page: 1, id: 99 }];
        assert!(edit_to_file(source, stale, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), FIXTURE);

        let ops = vec![PdfEditOp::Move {
            page: 1,
            id: 1,
            dx: 200.0,
            dy: -50.0,
        }];
        let written = edit_to_file(source, ops, &path).unwrap();
        let saved = std::fs::read(&path).unwrap();
        assert_eq!(written, saved.len() as u64);
        let again: Value =
            serde_json::from_str(&apply(&saved, &[], 1, false).unwrap().state).unwrap();
        assert_eq!(bbox(&again, 1), [250.0, 550.0, 350.0, 630.0]);
    }

    #[test]
    fn explains_text_that_needs_shaping() {
        // The engine refuses such text; this pins the wording it is recognised by.
        let hindi = [PdfEditOp::ReplaceLine {
            page: 1,
            id: 2,
            text: "नमस्ते".into(),
        }];
        assert_eq!(apply(FIXTURE, &hindi, 1, false).unwrap_err(), NEEDS_SHAPING);
    }

    #[test]
    fn parses_the_wire_format() {
        let request: PdfEditRequest = serde_json::from_str(
            r#"{"path":"/tmp/a.pdf","page":2,"ops":[
                {"op":"move","page":2,"id":7,"dx":1.5,"dy":-2},
                {"op":"delete","page":2,"id":8},
                {"op":"fit","page":1,"id":3,"rect":[0,0,10,10]},
                {"op":"replace_line","page":1,"id":4,"text":"x"},
                {"op":"replace_block","page":1,"id":5,"text":"y"}]}"#,
        )
        .unwrap();
        assert_eq!(
            (request.ops.len(), request.page, request.want_pdf),
            (5, 2, false)
        );
    }
}
