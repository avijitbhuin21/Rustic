//! Integration tests exercising the assembled router end-to-end via
//! `tower::ServiceExt::oneshot` (no network socket needed). Covers the auth
//! gate, login, the command dispatch, and the 501 fallback — the Phase 7
//! "route ↔ command arg mapping" and "auth token issue/verify" gates.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use rustic_app::config::ServerConfig;
use tower::ServiceExt;

// Each test gets its own data dir so the SQLite DBs don't collide.
static SEQ: AtomicU16 = AtomicU16::new(0);

fn test_config(password: &str) -> ServerConfig {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("rustic-it-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    ServerConfig {
        auth_password: password.to_string(),
        session_secret: b"integration-test-secret".to_vec(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.clone(),
        static_dir: dir, // no static assets needed for these tests
        session_ttl_secs: 3600,
        login_max_attempts: 5,
        login_lockout_secs: 300,
        preview_domain: None,
        cookie_domain: None,
    }
}

fn router(password: &str) -> axum::Router {
    let shared = rustic_server::build_shared(test_config(password)).unwrap();
    rustic_server::app::build_router(shared)
}

/// Run an async test body on its own runtime, dropping the last router handle outside any async context.
fn run<F, Fut>(password: &str, body: F)
where
    F: FnOnce(axum::Router) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let app = {
        let _guard = rt.enter();
        router(password)
    };
    rt.block_on(body(app.clone()));
    drop(app);
    drop(rt);
}

/// Build a request with a fake ConnectInfo so the login handler's extractor
/// resolves (oneshot doesn't populate it automatically).
fn req(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

fn with_peer(mut r: Request<Body>) -> Request<Body> {
    let peer: SocketAddr = "127.0.0.1:55555".parse().unwrap();
    r.extensions_mut().insert(ConnectInfo(peer));
    r
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

#[test]
fn health_is_open() {
    run("pw", |app| async move {
        let resp = app
            .oneshot(req("GET", "/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["status"], "ok");
    });
}

#[test]
fn api_requires_auth() {
    run("pw", |app| async move {
        let resp = app
            .oneshot(
                req("POST", "/api/list_projects")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn wrong_password_rejected() {
    run("correct-horse", |app| async move {
        let resp = app
            .oneshot(with_peer(
                req("POST", "/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"password":"nope"}"#))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn login_then_call_api() {
    run("correct-horse", |app| async move {
        let token = login_token(&app, "correct-horse").await;
        assert!(!token.is_empty());
        let resp = app
            .oneshot(
                req("POST", "/api/list_projects")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await, serde_json::json!([]));
    });
}

#[test]
fn unwired_command_returns_501() {
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;
        let resp = app
            .oneshot(
                req("POST", "/api/totally_made_up_command")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        assert!(body_json(resp).await["error"]
            .as_str()
            .unwrap()
            .contains("totally_made_up_command"));
    });
}

#[test]
fn prewarm_agent_project_warms_without_starting_unapproved_servers() {
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;
        let call = |cmd: &'static str, body: serde_json::Value| {
            let app = app.clone();
            let token = token.clone();
            async move {
                app.oneshot(
                    req("POST", &format!("/api/{cmd}"))
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("rustic-it-warm-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
        let marker = root.join("spawned");
        let unapproved = serde_json::json!({ "mcpServers": { "evil": {
            "command": "sh", "args": ["-c", format!("touch '{}'", marker.display())]
        } } });
        std::fs::write(root.join(".mcp.json"), unapproved.to_string()).unwrap();

        let added = call("add_project", serde_json::json!({ "path": root })).await;
        assert_eq!(added.status(), StatusCode::OK);
        let project_id = body_json(added).await["id"].as_str().unwrap().to_string();

        let warmed = call(
            "prewarm_agent_project",
            serde_json::json!({ "projectId": project_id }),
        )
        .await;
        assert_eq!(warmed.status(), StatusCode::OK);
        assert!(!marker.exists(), "unapproved project server was spawned");

        let missing = call(
            "prewarm_agent_project",
            serde_json::json!({ "projectId": "no-such-project" }),
        )
        .await;
        assert_ne!(missing.status(), StatusCode::OK);
        let _ = std::fs::remove_dir_all(&root);
    });
}

async fn login_token(app: &axum::Router, pw: &str) -> String {
    let login = app
        .clone()
        .oneshot(with_peer(
            req("POST", "/login")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"password":"{pw}"}}"#)))
                .unwrap(),
        ))
        .await
        .unwrap();
    body_json(login).await["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn upload_then_download_file_roundtrips() {
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;

        let dir = std::env::temp_dir().join(format!("rustic-upl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let up = app
            .clone()
            .oneshot(
                req(
                    "POST",
                    &format!(
                        "/api/upload_stream?dstDir={}&name=greet.txt&offset=0",
                        urlencode(&dir.to_string_lossy())
                    ),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("hello "))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::OK);
        let written = body_json(up).await["path"].as_str().unwrap().to_string();

        let up2 = app
            .clone()
            .oneshot(
                req(
                    "POST",
                    &format!("/api/upload_stream?path={}&offset=6", urlencode(&written)),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("upload"))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up2.status(), StatusCode::OK);
        assert_eq!(body_json(up2).await["size"].as_u64().unwrap(), 12);
        assert_eq!(std::fs::read(&written).unwrap(), b"hello upload");

        let down = app
            .oneshot(
                req(
                    "GET",
                    &format!("/api/download?path={}", urlencode(&written)),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(down.status(), StatusCode::OK);
        let bytes = to_bytes(down.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&bytes[..], b"hello upload");
    });
}

#[test]
fn upload_rejects_path_traversal() {
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;
        let dir = std::env::temp_dir().join(format!("rustic-trav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let up = app
            .oneshot(
                req(
                    "POST",
                    &format!(
                        "/api/upload_stream?dstDir={}&name=evil&relativePath={}",
                        urlencode(&dir.to_string_lossy()),
                        urlencode("../escape.txt")
                    ),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("x"))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::BAD_REQUEST);
    });
}

#[test]
fn download_folder_returns_zip() {
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;

        let dir = std::env::temp_dir().join(format!("rustic-zip-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"aaa").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), b"bbb").unwrap();

        let down = app
            .oneshot(
                req(
                    "GET",
                    &format!("/api/download?path={}", urlencode(&dir.to_string_lossy())),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(down.status(), StatusCode::OK);
        let ct = down
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(ct, "application/zip");
        let bytes = to_bytes(down.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    });
}

#[test]
fn download_refuses_internal_data() {
    let cfg = test_config("pw");
    let data_dir = cfg.data_dir.clone();
    std::fs::write(data_dir.join("secrets.json"), b"{}").unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let app = {
        let _guard = rt.enter();
        rustic_server::app::build_router(rustic_server::build_shared(cfg).unwrap())
    };
    let inner = app.clone();
    rt.block_on(async move {
        let token = login_token(&inner, "pw").await;
        let secret = data_dir.join("secrets.json");
        let resp = inner
            .oneshot(
                req(
                    "GET",
                    &format!(
                        "/api/download?path={}",
                        urlencode(&secret.to_string_lossy())
                    ),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    });
    drop(app);
    drop(rt);
}

/// Browser lifecycle: `browser_open` starts Chromium (its CDP port answers),
/// and `browser_close` tears it fully down (the port stops answering and no
/// process is left behind). Skipped when the host has no Chromium binary, so
/// dev boxes / CI without it still pass — the real assertion runs on the
/// container image where `chromium` is installed.
#[test]
fn browser_open_starts_and_close_reaps_chromium() {
    if rustic_server::browser::cdp::find_chromium().is_none() {
        eprintln!("skipping browser lifecycle test: no Chromium binary on host");
        return;
    }
    run("pw", |app| async move {
        let token = login_token(&app, "pw").await;

        // Open → Chromium up, at least one tab.
        let open = app
            .clone()
            .oneshot(
                req("POST", "/api/browser_open")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(open.status(), StatusCode::OK);
        let body = body_json(open).await;
        assert_eq!(body["running"], true);
        assert!(body["tabs"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false));

        // The loopback CDP port answers while running.
        let port: u16 = std::env::var("RUSTIC_BROWSER_DEBUG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9222);
        let version_url = format!("http://127.0.0.1:{port}/json/version");
        assert!(
            http_ok(&version_url).await,
            "CDP port should answer while the browser is open"
        );

        // Close → Chromium reaped, port no longer answers.
        let close = app
            .oneshot(
                req("POST", "/api/browser_close")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(close.status(), StatusCode::OK);

        // Give the port a moment to free, then confirm it's refused.
        let mut refused = false;
        for _ in 0..20 {
            if !http_ok(&version_url).await {
                refused = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(refused, "CDP port must stop answering after browser_close");
    });
}

/// True iff a GET to `url` returns a 2xx within a short timeout.
async fn http_ok(url: &str) -> bool {
    reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_millis(800))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Minimal percent-encoding for the `path` query value in tests (spaces, etc.).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/'
            | b':'
            | b'\\' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Stdio MCP server that holds its `initialize` reply until the gate file ($1) exists.
#[cfg(unix)]
const GATED_MCP_SERVER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      while [ ! -e "$1" ]; do sleep 0.02; done
      printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{}}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"probe"}]}}\n' "$id" ;;
  esac
done
"#;

#[cfg(unix)]
#[test]
fn mcp_list_returns_before_servers_connect_and_pushes_their_status() {
    use std::time::{Duration, Instant};

    let config = test_config("pw");
    let data_dir = config.data_dir.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let shared = {
        let _guard = rt.enter();
        rustic_server::build_shared(config).unwrap()
    };
    let app = rustic_server::app::build_router(shared.clone());

    rt.block_on(async {
        let token = login_token(&app, "pw").await;
        let call = |cmd: &'static str, body: serde_json::Value| {
            let app = app.clone();
            let token = token.clone();
            async move {
                let resp = app
                    .oneshot(
                        req("POST", &format!("/api/{cmd}"))
                            .header("content-type", "application/json")
                            .header("authorization", format!("Bearer {token}"))
                            .body(Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK, "{cmd} failed");
                body_json(resp).await
            }
        };

        let script = data_dir.join("gated-mcp.sh");
        std::fs::write(&script, GATED_MCP_SERVER).unwrap();
        let gate = data_dir.join("gate");
        let pool = serde_json::json!({ "mcpServers": {
            "slow": { "command": "sh", "args": [script, gate] }
        } });
        let mut events = shared.ctx.hub.subscribe();

        // Saving and listing both come back while the server is still held shut.
        let started = Instant::now();
        let saved = call(
            "save_mcp_json",
            serde_json::json!({ "scope": "user", "projectId": null, "content": pool.to_string() }),
        )
        .await;
        let listed = call("list_mcp_servers", serde_json::json!({ "projectId": null })).await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "save/list waited on the server handshake ({:?})",
            started.elapsed()
        );
        for rows in [&saved, &listed] {
            assert_eq!(rows.as_array().unwrap().len(), 1);
            assert_eq!(rows[0]["name"], "slow");
            assert_eq!(rows[0]["status"]["state"], "connecting");
        }

        // Once the server answers, its result is pushed as an event.
        std::fs::write(&gate, "").unwrap();
        let settled = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let text = match events.recv().await {
                    Ok(text) => text,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(e) => panic!("hub closed: {e}"),
                };
                let msg: serde_json::Value = serde_json::from_str(&text).unwrap();
                if msg["event"] == "mcp-server-status"
                    && msg["payload"]["status"]["state"] != "connecting"
                {
                    return msg["payload"].clone();
                }
            }
        })
        .await
        .expect("no mcp-server-status event after the server connected");
        assert_eq!(settled["id"], "user-slow");
        assert_eq!(settled["status"]["state"], "connected");
        assert_eq!(settled["status"]["tool_count"], 1);
        // The push outranks the earlier "connecting" snapshot, so a client
        // receiving them out of order keeps the settled status.
        assert!(settled["seq"].as_u64().unwrap() > listed[0]["status_seq"].as_u64().unwrap());

        let listed = call("list_mcp_servers", serde_json::json!({ "projectId": null })).await;
        assert_eq!(listed[0]["status"]["state"], "connected");

        let tools = call(
            "list_mcp_server_tools",
            serde_json::json!({ "id": "user-slow" }),
        )
        .await;
        assert_eq!(tools[0]["name"], "probe");
        call(
            "remove_mcp_server",
            serde_json::json!({ "id": "user-slow" }),
        )
        .await;
    });
    drop(app);
    drop(shared);
    drop(rt);
    let _ = std::fs::remove_dir_all(&data_dir);
}
