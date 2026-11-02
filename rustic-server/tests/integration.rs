//! Integration tests exercising the assembled router end-to-end via
//! `tower::ServiceExt::oneshot` (no network socket needed). Covers the auth
//! gate, login, the command dispatch, and the 501 fallback — the Phase 7
//! "route ↔ command arg mapping" and "auth token issue/verify" gates.

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
                req("GET", &format!("/api/download?path={}", urlencode(&written)))
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
                req("GET", &format!("/api/download?path={}", urlencode(&secret.to_string_lossy())))
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
