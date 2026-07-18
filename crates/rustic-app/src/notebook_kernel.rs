//! Host-agnostic notebook execution core: a persistent per-notebook Python
//! subprocess acting as a minimal kernel. Cells are sent as JSON lines on
//! stdin; replies come back as JSON lines on stdout and are forwarded to the
//! host through an emit callback (Tauri event / WS hub). State persists
//! across cells (shared globals dict) like a real kernel; no Jupyter
//! installation is required.
//!
//! Python resolution honours project virtualenvs: `<cwd>/.venv` and
//! `<cwd>/venv` are preferred over the PATH `python`.

use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// In-process Python bridge: reads one JSON message per stdin line
/// (`{"id": cell_id, "code": source}`), executes it in a persistent globals
/// dict (last bare expression is echoed like a REPL), and replies with one
/// JSON line. Matplotlib figures are captured as base64 PNGs when available.
const PY_BRIDGE: &str = r#"
import sys, json, io, traceback, contextlib, ast, base64
g = {'__name__': '__main__'}
def _capture_figures():
    try:
        import matplotlib
        import matplotlib.pyplot as plt
    except Exception:
        return []
    images = []
    for num in plt.get_fignums():
        try:
            fig = plt.figure(num)
            buf = io.BytesIO()
            fig.savefig(buf, format='png', bbox_inches='tight')
            images.append(base64.b64encode(buf.getvalue()).decode('ascii'))
        except Exception:
            pass
    plt.close('all')
    return images
try:
    import matplotlib
    matplotlib.use('Agg')
except Exception:
    pass
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    cid = msg.get('id')
    code = msg.get('code', '')
    b64 = msg.get('code_b64')
    if b64:
        try:
            code = base64.b64decode(b64).decode('utf-8', 'replace')
        except Exception:
            code = ''
    out = io.StringIO(); err = io.StringIO()
    result_repr = None; ok = True
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            tree = ast.parse(code, mode='exec')
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                last = ast.Expression(tree.body.pop(-1).value)
                exec(compile(tree, '<cell>', 'exec'), g)
                val = eval(compile(last, '<cell>', 'eval'), g)
                if val is not None:
                    result_repr = repr(val)
            else:
                exec(compile(tree, '<cell>', 'exec'), g)
    except SystemExit:
        pass
    except Exception:
        ok = False
        err.write(traceback.format_exc())
    reply = {'id': cid, 'ok': ok, 'stdout': out.getvalue(), 'stderr': err.getvalue(), 'result': result_repr, 'images': _capture_figures()}
    sys.stdout.write(json.dumps(reply) + '\n')
    sys.stdout.flush()
"#;

/// Event forwarded to the frontend as `notebook-kernel-output`.
#[derive(Clone, Serialize)]
pub struct KernelEvent {
    pub notebook_id: String,
    /// One of "reply" (cell finished, payload = bridge JSON), "started",
    /// "exited", "error".
    pub kind: String,
    pub payload: Option<serde_json::Value>,
    pub message: Option<String>,
}

/// Host-provided event sink (Tauri emit / WS hub broadcast).
pub type KernelEmit = Arc<dyn Fn(KernelEvent) + Send + Sync>;

struct Kernel {
    child: Child,
    stdin: ChildStdin,
}

fn kernels() -> &'static Mutex<HashMap<String, Kernel>> {
    static K: OnceLock<Mutex<HashMap<String, Kernel>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pick the Python interpreter for `cwd`: project venv first, then PATH.
fn resolve_python(cwd: &Path) -> String {
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &[r".venv\Scripts\python.exe", r"venv\Scripts\python.exe"];
    #[cfg(not(target_os = "windows"))]
    const CANDIDATES: &[&str] = &[".venv/bin/python", "venv/bin/python"];
    for rel in CANDIDATES {
        let p = cwd.join(rel);
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    "python".to_string()
}

/// Start (or restart) the Python kernel for a notebook. Returns the resolved
/// Python interpreter path.
pub fn start(notebook_id: &str, cwd: &str, emit: KernelEmit) -> Result<String, String> {
    // Kill any previous kernel for this notebook first (restart semantics).
    stop(notebook_id);

    let cwd_path = PathBuf::from(cwd);
    let python = resolve_python(&cwd_path);
    let mut cmd = Command::new(&python);
    cmd.args(["-u", "-c", PY_BRIDGE])
        .current_dir(&cwd_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {python}: {e}"))?;
    let stdin = child.stdin.take().ok_or("kernel stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("kernel stdout unavailable")?;

    // Forward every bridge reply line to the frontend as an event.
    {
        let emit = Arc::clone(&emit);
        let nb = notebook_id.to_string();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => emit(KernelEvent {
                        notebook_id: nb.clone(),
                        kind: "reply".into(),
                        payload: Some(v),
                        message: None,
                    }),
                    Err(_) => emit(KernelEvent {
                        notebook_id: nb.clone(),
                        kind: "error".into(),
                        payload: None,
                        message: Some(line),
                    }),
                }
            }
            // stdout closed — the kernel process is gone.
            emit(KernelEvent {
                notebook_id: nb.clone(),
                kind: "exited".into(),
                payload: None,
                message: None,
            });
            if let Ok(mut map) = kernels().lock() {
                map.remove(&nb);
            }
        });
    }

    if let Ok(mut map) = kernels().lock() {
        map.insert(notebook_id.to_string(), Kernel { child, stdin });
    }
    emit(KernelEvent {
        notebook_id: notebook_id.to_string(),
        kind: "started".into(),
        payload: None,
        message: Some(python.clone()),
    });
    Ok(python)
}

/// Send a cell's code to the notebook's kernel. The reply arrives via the
/// emit callback with kind="reply" and payload.id = cell_id.
pub fn exec(notebook_id: &str, cell_id: &str, code: &str) -> Result<(), String> {
    use base64::Engine;
    let mut map = kernels().lock().map_err(|_| "kernel registry poisoned")?;
    let kernel = map
        .get_mut(notebook_id)
        .ok_or("Kernel not running — start it first")?;
    // Base64-armored so no quoting/escaping layer between the webview, the
    // host, and the Python process can ever mangle the source.
    let b64 = base64::engine::general_purpose::STANDARD.encode(code.as_bytes());
    let msg = serde_json::json!({ "id": cell_id, "code_b64": b64 });
    let line = format!("{}\n", msg);
    kernel
        .stdin
        .write_all(line.as_bytes())
        .and_then(|_| kernel.stdin.flush())
        .map_err(|e| format!("kernel write failed: {e}"))
}

/// Stop the notebook's kernel (used for restart and on tab close). Idempotent.
pub fn stop(notebook_id: &str) {
    if let Ok(mut map) = kernels().lock() {
        if let Some(mut k) = map.remove(notebook_id) {
            let _ = k.child.kill();
            let _ = k.child.wait();
        }
    }
}
