//! Low-level Chrome DevTools Protocol helpers + Chromium binary discovery.
//!
//! These are deliberately tiny one-shot helpers: the browser feature issues a
//! handful of infrequent control commands (create/close/navigate a tab), so for
//! each we open a fresh CDP WebSocket, send one command, await its reply, and
//! close. The high-throughput path (screencast + DevTools) does NOT go through
//! here — it's a raw bidirectional proxy (see [`super::proxy`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as TMessage;

/// One open tab/page, as surfaced to the frontend tab strip.
#[derive(Clone, Debug, Serialize)]
pub struct TabInfo {
    /// The CDP `targetId` — the stable handle the proxy + navigate commands use.
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
}

/// How long a single CDP request may take before we give up. Generous because
/// `Target.createTarget` waits for the renderer to come up.
const CDP_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Send a single CDP method to a WebSocket endpoint and return its `result`.
///
/// Opens the socket, writes `{id, method, params}`, reads frames until the one
/// whose `id` matches comes back (ignoring unsolicited events), then closes.
pub async fn call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let fut = async {
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| format!("CDP connect to {ws_url} failed: {e}"))?;

        let id: u64 = 1;
        let payload = json!({ "id": id, "method": method, "params": params });
        ws.send(TMessage::Text(payload.to_string().into()))
            .await
            .map_err(|e| format!("CDP send failed: {e}"))?;

        while let Some(frame) = ws.next().await {
            let frame = frame.map_err(|e| format!("CDP recv failed: {e}"))?;
            if let TMessage::Text(text) = frame {
                let v: Value = match serde_json::from_str(text.as_str()) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("id").and_then(Value::as_u64) == Some(id) {
                    let _ = ws.send(TMessage::Close(None)).await;
                    if let Some(err) = v.get("error") {
                        return Err(format!("CDP {method} error: {err}"));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                // else: an unsolicited event — keep reading.
            }
        }
        Err(format!("CDP socket closed before {method} replied"))
    };

    tokio::time::timeout(CDP_CALL_TIMEOUT, fut)
        .await
        .map_err(|_| format!("CDP {method} timed out"))?
}

/// `GET {http_base}/json/version` → the browser-level CDP WebSocket URL. This is
/// the endpoint that owns `Target.*` / `Browser.*` domains.
pub async fn browser_ws_url(http_base: &str) -> Result<String, String> {
    let url = format!("{http_base}/json/version");
    let v: Value = http_get_json(&url).await?;
    v.get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "json/version missing webSocketDebuggerUrl".to_string())
}

/// `GET {http_base}/json` → the list of page targets, mapped to [`TabInfo`].
/// Filters to `type == "page"` so service workers / extension backgrounds /
/// devtools targets never show up as tabs.
pub async fn list_pages(http_base: &str) -> Result<Vec<TabInfo>, String> {
    let url = format!("{http_base}/json");
    let v: Value = http_get_json(&url).await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let mut tabs = Vec::new();
    for t in arr {
        if t.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let Some(id) = t.get("id").and_then(Value::as_str) else {
            continue;
        };
        tabs.push(TabInfo {
            id: id.to_string(),
            title: t
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            url: t
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            favicon: t
                .get("faviconUrl")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        });
    }
    Ok(tabs)
}

/// `Target.createTarget` on the browser endpoint → the new target's id.
pub async fn create_target(browser_ws: &str, url: &str) -> Result<String, String> {
    let result = call(browser_ws, "Target.createTarget", json!({ "url": url })).await?;
    result
        .get("targetId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Target.createTarget returned no targetId".to_string())
}

/// `Target.closeTarget` on the browser endpoint.
pub async fn close_target(browser_ws: &str, target_id: &str) -> Result<(), String> {
    call(
        browser_ws,
        "Target.closeTarget",
        json!({ "targetId": target_id }),
    )
    .await
    .map(|_| ())
}

/// `Page.navigate` on a specific page target's CDP socket.
pub async fn navigate(page_ws: &str, url: &str) -> Result<(), String> {
    call(page_ws, "Page.navigate", json!({ "url": url }))
        .await
        .map(|_| ())
}

/// `Browser.close` — ask Chromium to shut itself down gracefully. Best-effort;
/// the manager hard-kills the process group afterwards regardless.
pub async fn browser_close(browser_ws: &str) -> Result<(), String> {
    call(browser_ws, "Browser.close", json!({}))
        .await
        .map(|_| ())
}

/// Minimal JSON GET against the loopback CDP HTTP endpoints.
async fn http_get_json(url: &str) -> Result<Value, String> {
    let resp = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("GET {url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GET {url} → HTTP {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("GET {url} bad JSON: {e}"))
}

// ─── Chromium binary discovery ──────────────────────────────────────────────

/// Locate a Chromium/Chrome executable, or `None` if the host has none.
///
/// Order: `$CHROME_BIN` (set in the container image), then a set of well-known
/// names on `PATH`, then — on Windows, for local dev — the standard install
/// dirs. Cross-platform so the feature is testable on a dev box even though the
/// real deployment target is the Debian `chromium` package on Linux.
pub fn find_chromium() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_BIN") {
        let path = PathBuf::from(&p);
        if !p.is_empty() && path.exists() {
            return Some(path);
        }
    }

    let names: &[&str] = if cfg!(windows) {
        &["chrome.exe", "chromium.exe", "msedge.exe"]
    } else {
        &[
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "chrome",
        ]
    };
    if let Some(p) = which(names) {
        return Some(p);
    }

    #[cfg(windows)]
    {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Ok(dir) = std::env::var(base) {
                for rel in [
                    r"Google\Chrome\Application\chrome.exe",
                    r"Chromium\Application\chrome.exe",
                    r"Microsoft\Edge\Application\msedge.exe",
                ] {
                    let candidate = Path::new(&dir).join(rel);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    None
}

/// Resolve the first of `names` found on `PATH`.
fn which(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
