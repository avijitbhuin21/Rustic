//! Drag & drop between remote-backend windows.
//!
//! Remote windows load an external rustic-server origin, so they get no Tauri
//! IPC (exposing it to a remote page would hand that page every desktop
//! command). Instead an injected init script talks to the desktop by starting
//! a navigation to a sentinel host, which `handle_navigation` intercepts and
//! cancels. The only things a page can ask for:
//!
//! * `drag?token&path` — "I (this window) started dragging `path`".
//! * `drop?token&dir`  — "a drag with `token` was dropped on folder `dir` here".
//!
//! A drop is relayed only when the token was registered by a *different*
//! remote window within [`DRAG_TTL`], so a page can never choose which file is
//! read from another backend — the user has to have dragged it there. The
//! desktop then downloads it from the source server and uploads it to the
//! target server with each backend's keychain password.
//!
//! OS → backend drops need none of this: remote windows are built with the
//! native drag-drop handler disabled, so the web UI's own HTML5 drop → HTTP
//! upload path handles them (with the exact folder under the cursor).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Url};

/// Host the init script navigates to; never resolves, always cancelled.
pub const SENTINEL_HOST: &str = "rustic-dnd.invalid";

/// How long a registered drag stays droppable.
const DRAG_TTL: Duration = Duration::from_secs(120);

/// Upload chunk size (matches the web client's order of magnitude and stays
/// under typical proxy body limits).
const UPLOAD_CHUNK: usize = 32 * 1024 * 1024;

struct PendingDrag {
    label: String,
    path: String,
    at: Instant,
}

/// window label → backend base URL, for windows opened by `remote_backend_open`.
static WINDOWS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// drag token → where the drag came from.
static DRAGS: Mutex<Option<HashMap<String, PendingDrag>>> = Mutex::new(None);

/// Remember which backend a remote window shows (used to log in for relays).
pub fn register_window(label: &str, base: &str) {
    let mut g = WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashMap::new)
        .insert(label.to_string(), base.to_string());
}

fn window_base(label: &str) -> Option<String> {
    WINDOWS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(label).cloned())
}

/// A request decoded from a sentinel navigation.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Drag { token: String, path: String },
    Drop { token: String, dir: String },
}

fn valid_token(t: &str) -> bool {
    (8..=64).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Decode a sentinel URL. `None` for anything malformed.
fn parse_action(url: &Url) -> Option<Action> {
    if url.host_str() != Some(SENTINEL_HOST) {
        return None;
    }
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let token = q.get("token").filter(|t| valid_token(t))?.clone();
    let field = |k: &str| {
        q.get(k)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .cloned()
    };
    match url.path().trim_matches('/') {
        "drag" => Some(Action::Drag { token, path: field("path")? }),
        "drop" => Some(Action::Drop { token, dir: field("dir")? }),
        _ => None,
    }
}

/// Last path component, accepting `/` and `\` separators.
fn base_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string()
}

/// JS that shows a status toast inside a remote window (handled by the init script).
fn status_js(message: &str, error: bool) -> String {
    let detail = serde_json::json!({ "message": message, "error": error });
    format!("window.dispatchEvent(new CustomEvent('rustic-dnd',{{detail:{detail}}}));")
}

/// `on_navigation` hook for remote windows. Returns whether to allow the
/// navigation; sentinel navigations are always swallowed.
pub fn handle_navigation(app: &AppHandle, label: &str, url: &Url) -> bool {
    if url.host_str() != Some(SENTINEL_HOST) {
        return true;
    }
    match parse_action(url) {
        Some(Action::Drag { token, path }) => {
            let mut g = DRAGS.lock().unwrap_or_else(|p| p.into_inner());
            let m = g.get_or_insert_with(HashMap::new);
            m.retain(|_, d| d.at.elapsed() < DRAG_TTL);
            m.insert(
                token,
                PendingDrag {
                    label: label.to_string(),
                    path,
                    at: Instant::now(),
                },
            );
        }
        Some(Action::Drop { token, dir }) => {
            let drag = DRAGS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
                .and_then(|m| m.remove(&token));
            let Some(drag) = drag.filter(|d| d.at.elapsed() < DRAG_TTL && d.label != label)
            else {
                return false;
            };
            let app = app.clone();
            let dst_label = label.to_string();
            tauri::async_runtime::spawn(async move {
                let name = base_name(&drag.path);
                notify(&app, &dst_label, &format!("Copying {name}…"), false);
                match relay(&app, &drag.label, &drag.path, &dst_label, &dir).await {
                    Ok(saved) => notify(&app, &dst_label, &format!("Copied {}", base_name(&saved)), false),
                    Err(e) => notify(&app, &dst_label, &format!("Copy failed: {e}"), true),
                }
            });
        }
        None => {}
    }
    false
}

fn notify(app: &AppHandle, label: &str, message: &str, error: bool) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.eval(&status_js(message, error));
    }
}

/// Saved password for `base`: the per-backend keychain account, else the
/// legacy single account. Mirrors `cloud_sync::remembered_password` (private
/// there) — swap to that once it's `pub(crate)`.
fn saved_password(base: &str) -> Result<String, String> {
    super::cloud_sync::remembered_password(base)
}

async fn login(client: &reqwest::Client, base: &str) -> Result<String, String> {
    let password = saved_password(base)?;
    let resp = client
        .post(format!("{base}/login"))
        .json(&serde_json::json!({ "password": password }))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Could not reach {base}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{base} rejected the saved password (HTTP {})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{base}: login response carried no token"))
}

/// Download `path` from the source window's backend into a temp file and
/// upload it into `dir` on the target window's backend. Returns the saved path.
async fn relay(
    app: &AppHandle,
    src_label: &str,
    path: &str,
    dst_label: &str,
    dir: &str,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for l in [src_label, dst_label] {
        if app.get_webview_window(l).is_none() {
            return Err("the other backend window was closed".into());
        }
    }
    let src = window_base(src_label).ok_or("unknown source backend")?;
    let dst = window_base(dst_label).ok_or("unknown target backend")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let src_token = login(&client, &src).await?;
    let dst_token = login(&client, &dst).await?;

    let mut resp = client
        .get(format!("{src}/api/download"))
        .query(&[("path", path)])
        .bearer_auth(&src_token)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("download failed: {msg}"));
    }
    let mut name = base_name(path);
    let is_zip = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("zip"));
    if is_zip && !name.to_ascii_lowercase().ends_with(".zip") {
        name.push_str(".zip"); // folders arrive zipped
    }

    let tmp_dir = std::env::temp_dir().join("rustic-dnd");
    tokio::fs::create_dir_all(&tmp_dir).await.map_err(|e| e.to_string())?;
    let tmp = tmp_dir.join(uuid::Uuid::new_v4().to_string());
    let result = async {
        let mut f = tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("download failed: {e}"))? {
            f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())?;
        drop(f);

        let mut f = tokio::fs::File::open(&tmp).await.map_err(|e| e.to_string())?;
        let mut offset: u64 = 0;
        let mut saved: Option<String> = None;
        loop {
            let mut buf = vec![0u8; UPLOAD_CHUNK];
            let mut n = 0;
            while n < buf.len() {
                let r = f.read(&mut buf[n..]).await.map_err(|e| e.to_string())?;
                if r == 0 {
                    break;
                }
                n += r;
            }
            buf.truncate(n);
            if n == 0 && saved.is_some() {
                break;
            }
            let off = offset.to_string();
            let mut q: Vec<(&str, &str)> = vec![("offset", off.as_str())];
            match saved.as_deref() {
                Some(p) => q.push(("path", p)),
                None => {
                    q.push(("dstDir", dir));
                    q.push(("name", name.as_str()));
                }
            }
            let r = client
                .post(format!("{dst}/api/upload_stream"))
                .query(&q)
                .bearer_auth(&dst_token)
                .body(buf)
                .send()
                .await
                .map_err(|e| format!("upload failed: {e}"))?;
            let ok = r.status().is_success();
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if !ok {
                let msg = body.get("error").and_then(|v| v.as_str()).unwrap_or("upload rejected");
                return Err(format!("upload failed: {msg}"));
            }
            saved = body.get("path").and_then(|v| v.as_str()).map(str::to_string);
            if saved.is_none() {
                return Err("upload failed: server returned no path".into());
            }
            offset += n as u64;
            if n < UPLOAD_CHUNK {
                break;
            }
        }
        saved.ok_or_else(|| "upload failed".to_string())
    }
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    result
}

/// Script injected into every remote-backend window. See the module docs.
pub const INIT_SCRIPT: &str = r#"(function () {
  if (window.__rusticDnd) return;
  window.__rusticDnd = true;
  var HOST = 'https://rustic-dnd.invalid/';
  var TYPE = 'application/x-rustic-dnd';
  var mine = new Set();
  var localDrag = false;
  function send(action, params) {
    try { window.location.assign(HOST + action + '?' + new URLSearchParams(params).toString()); } catch (_) {}
  }
  function token() {
    return (crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2));
  }
  function dirAt(el) {
    var n = el && el.closest ? el.closest('[data-explorer-dir]') : null;
    return n ? n.getAttribute('data-explorer-dir') : null;
  }
  function foreign(e) {
    return !localDrag && e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], TYPE) >= 0;
  }
  var box = null, timer = null;
  function toast(msg, isError) {
    if (!document.body) return;
    if (!box) {
      box = document.createElement('div');
      box.style.cssText = 'position:fixed;bottom:16px;right:16px;z-index:2147483647;max-width:420px;padding:8px 12px;border-radius:8px;font:12px system-ui,sans-serif;box-shadow:0 4px 16px rgba(0,0,0,.3);pointer-events:none';
      document.body.appendChild(box);
    }
    box.textContent = msg;
    box.style.background = isError ? '#7f1d1d' : '#1f2937';
    box.style.color = '#fff';
    box.style.display = 'block';
    clearTimeout(timer);
    timer = setTimeout(function () { box.style.display = 'none'; }, isError ? 8000 : 3000);
  }
  window.addEventListener('rustic-dnd', function (e) { toast(e.detail.message, e.detail.error); });
  // Bubble phase on window runs after React's root handler set the path.
  window.addEventListener('dragstart', function (e) {
    var dt = e.dataTransfer;
    if (!dt) return;
    var path = '';
    try { path = dt.getData('application/x-rustic-file'); } catch (_) {}
    if (!path) return;
    var t = token();
    try { dt.setData(TYPE, t); } catch (_) { return; }
    mine.add(t);
    localDrag = true;
    setTimeout(function () { send('drag', { token: t, path: path }); }, 0);
  });
  window.addEventListener('dragend', function () { localDrag = false; }, true);
  // Capture phase: a drag from another backend window must not reach the
  // explorer's own handlers (they'd treat its path as a local move).
  window.addEventListener('dragover', function (e) {
    if (!foreign(e)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = dirAt(e.target) ? 'copy' : 'none';
  }, true);
  window.addEventListener('drop', function (e) {
    if (!foreign(e)) return;
    e.preventDefault();
    e.stopPropagation();
    var dir = dirAt(e.target);
    if (!dir) { toast('Drop onto a folder or project in the explorer to copy it here', true); return; }
    var t = e.dataTransfer.getData(TYPE);
    if (!t || mine.has(t)) return;
    send('drop', { token: t, dir: dir });
  }, true);
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn parses_drag_and_drop() {
        assert_eq!(
            parse_action(&url("https://rustic-dnd.invalid/drag?token=abcd-1234-ef&path=%2Fsrv%2Fa+b.txt")),
            Some(Action::Drag { token: "abcd-1234-ef".into(), path: "/srv/a b.txt".into() })
        );
        assert_eq!(
            parse_action(&url("https://rustic-dnd.invalid/drop?token=abcd1234&dir=C%3A%5Cwork")),
            Some(Action::Drop { token: "abcd1234".into(), dir: "C:\\work".into() })
        );
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse_action(&url("https://example.com/drag?token=abcd1234&path=x")), None);
        assert_eq!(parse_action(&url("https://rustic-dnd.invalid/drag?token=short&path=x")), None);
        assert_eq!(parse_action(&url("https://rustic-dnd.invalid/drag?token=abcd1234")), None);
        assert_eq!(parse_action(&url("https://rustic-dnd.invalid/drop?token=abcd1234&dir=")), None);
        assert_eq!(parse_action(&url("https://rustic-dnd.invalid/other?token=abcd1234&dir=x")), None);
        assert_eq!(parse_action(&url("https://rustic-dnd.invalid/drag?token=bad%21token&path=x")), None);
    }

    #[test]
    fn base_name_handles_both_separators() {
        assert_eq!(base_name("/srv/proj/file.rs"), "file.rs");
        assert_eq!(base_name("C:\\work\\dir\\"), "dir");
        assert_eq!(base_name("/"), "download");
    }

    #[test]
    fn status_js_escapes_message() {
        let js = status_js("it's \"x\"</script>", true);
        assert!(js.contains(r#""message":"it's \"x\"</script>""#));
        assert!(js.contains(r#""error":true"#));
    }

    #[test]
    fn navigation_passes_through_normal_urls_only() {
        // Pure check of the host gate used by handle_navigation.
        assert_ne!(url("https://backend.example.com/").host_str(), Some(SENTINEL_HOST));
        assert_eq!(url("https://rustic-dnd.invalid/x").host_str(), Some(SENTINEL_HOST));
    }
}
