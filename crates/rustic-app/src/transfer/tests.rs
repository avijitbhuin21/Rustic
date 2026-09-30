use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use super::client::{self, Endpoint, TransferError};
use super::routes::{self, TransferHost};
use super::session::{ApplyFn, BuildFn, SessionManager};
use super::*;

/// Fresh scratch dir under the system temp dir.
fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("rustic-transfer-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Deterministic pseudo-random bytes.
fn sample(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x9e37_79b9;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

#[test]
fn assembler_accepts_out_of_order_and_streams_prefix() {
    let dir = scratch();
    let data = sample((2 * CHUNK_SIZE + 1234) as usize);
    let asm = Arc::new(Assembler::create(&dir.join("a.part")).unwrap());
    let mut reader = asm.growing().reader().unwrap();
    let out = std::thread::spawn(move || {
        let mut v = Vec::new();
        reader.read_to_end(&mut v).unwrap();
        v
    });
    let chunk = |i: u64| {
        let s = (i * CHUNK_SIZE) as usize;
        let e = (s + CHUNK_SIZE as usize).min(data.len());
        data[s..e].to_vec()
    };
    for i in [2u64, 0, 1] {
        let c = chunk(i);
        assert!(
            asm.put(i, &c, "deadbeef").is_err(),
            "bad checksum must be rejected"
        );
        asm.put(i, &c, &sha256_hex(&c)).unwrap();
        asm.put(i, &c, &sha256_hex(&c)).unwrap(); // duplicate is a no-op
    }
    assert!(!asm.is_complete(), "not complete before the total is known");
    asm.set_total(data.len() as u64);
    assert!(asm.missing().is_empty());
    assert!(asm.is_complete());
    assert_eq!(out.join().unwrap(), data);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn expired_upload_fails_the_extractor() {
    let dir = scratch();
    let mgr = SessionManager::new(dir.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let apply: ApplyFn = Box::new(move |mut r| {
        let mut v = Vec::new();
        let res = r
            .read_to_end(&mut v)
            .map(|_| json!(v.len()))
            .map_err(|e| e.to_string());
        let _ = tx.send(res.clone());
        res
    });
    let id = mgr.begin_upload(apply).unwrap();
    let c = sample(1000);
    mgr.put_chunk(&id, 0, &c, &sha256_hex(&c)).unwrap();
    mgr.expire_idle(Duration::ZERO);
    let res = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("extractor must not hang");
    assert!(res.unwrap_err().contains("expired"));
    assert!(mgr.upload_status(&id).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// Loopback host: uploads are hashed, downloads produce `payload`.
struct TestHost {
    sessions: Arc<SessionManager>,
    payload: Arc<Vec<u8>>,
}

impl TransferHost for TestHost {
    fn sessions(&self) -> Arc<SessionManager> {
        Arc::clone(&self.sessions)
    }
    fn authorize(&self, headers: &HeaderMap) -> Result<(), String> {
        match headers.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer secret") => Ok(()),
            _ => Err("bad token".into()),
        }
    }
    fn apply_for(&self, kind: &str, _headers: &HeaderMap) -> Result<ApplyFn, String> {
        let kind = kind.to_string();
        Ok(Box::new(move |mut r| {
            let mut v = Vec::new();
            r.read_to_end(&mut v).map_err(|e| e.to_string())?;
            Ok(json!({ "kind": kind, "len": v.len(), "sha": sha256_hex(&v) }))
        }))
    }
    fn build_for(&self, _body: &Value, _headers: &HeaderMap) -> Result<BuildFn, String> {
        let payload = Arc::clone(&self.payload);
        Ok(Box::new(move |mut w| {
            // Small, slow writes so chunks are served while still packing.
            for piece in payload.chunks(512 * 1024) {
                w.write_all(piece).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_millis(2));
            }
            w.flush().map_err(|e| e.to_string())?;
            Ok(json!({}))
        }))
    }
}

/// Fails the first request to each chunk path listed in `targets` with a 500.
#[derive(Default)]
struct Flaky {
    targets: Vec<String>,
    tripped: Mutex<HashSet<String>>,
}

async fn flaky(State(f): State<Arc<Flaky>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if let Some(t) = f.targets.iter().find(|t| path.ends_with(t.as_str())) {
        if f.tripped.lock().unwrap().insert(t.clone()) {
            return (StatusCode::INTERNAL_SERVER_ERROR, "injected").into_response();
        }
    }
    next.run(req).await
}

/// Serve the v2 routes on an ephemeral port; returns the base URL.
async fn serve(host: Arc<TestHost>, flaky_state: Arc<Flaky>) -> String {
    let app: axum::Router = routes::router::<TestHost, ()>(host)
        .layer(middleware::from_fn_with_state(flaky_state, flaky));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn endpoint(base: String, parallel: usize) -> Endpoint {
    Endpoint {
        client: reqwest::Client::new(),
        base,
        token: "secret".into(),
        parallel,
    }
}

fn no_progress() -> client::Progress {
    Arc::new(|_: &str, _: u64, _: u64| {})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_streams_in_parallel_and_retries_failed_chunk() {
    let dir = scratch();
    let host = Arc::new(TestHost {
        sessions: SessionManager::new(dir.join("srv")),
        payload: Arc::new(Vec::new()),
    });
    let flaky_state = Arc::new(Flaky {
        targets: vec!["/chunk/1".into()],
        ..Default::default()
    });
    let base = serve(host, Arc::clone(&flaky_state)).await;

    let data = Arc::new(sample((3 * CHUNK_SIZE + 777) as usize));
    let grow = Growing::new(&dir.join("send.part"));
    let mut w = grow.writer().unwrap();
    let (g, d) = (Arc::clone(&grow), Arc::clone(&data));
    let packer = std::thread::spawn(move || {
        for piece in d.chunks(256 * 1024) {
            w.write_all(piece).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        g.finish();
    });
    let res = client::upload(&endpoint(base, 4), "full", grow, no_progress())
        .await
        .unwrap();
    packer.join().unwrap();
    assert_eq!(res["kind"], "full");
    assert_eq!(res["len"], data.len());
    assert_eq!(res["sha"], sha256_hex(&data));
    assert!(
        flaky_state.tripped.lock().unwrap().contains("/chunk/1"),
        "failure was injected"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_streams_into_extractor_and_retries_failed_chunk() {
    let dir = scratch();
    let payload = Arc::new(sample((2 * CHUNK_SIZE + 99) as usize));
    let host = Arc::new(TestHost {
        sessions: SessionManager::new(dir.join("srv")),
        payload: Arc::clone(&payload),
    });
    let flaky_state = Arc::new(Flaky {
        targets: vec!["/chunk/0".into(), "/chunk/2".into()],
        ..Default::default()
    });
    let base = serve(host, Arc::clone(&flaky_state)).await;

    let asm = Arc::new(Assembler::create(&dir.join("recv.part")).unwrap());
    let mut reader = asm.growing().reader().unwrap();
    let extractor = std::thread::spawn(move || {
        let mut v = Vec::new();
        reader.read_to_end(&mut v).map(|_| v)
    });
    client::download(
        &endpoint(base, 2),
        json!({ "projects": true }),
        Arc::clone(&asm),
        no_progress(),
    )
    .await
    .unwrap();
    let got = extractor.join().unwrap().unwrap();
    assert_eq!(got.len(), payload.len());
    assert!(got == *payload);
    assert_eq!(
        flaky_state.tripped.lock().unwrap().len(),
        2,
        "failures were injected"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_endpoints_report_unsupported() {
    let app = axum::Router::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = scratch();
    let grow = Growing::new(&dir.join("x.part"));
    let err = client::upload(
        &endpoint(format!("http://{addr}"), 2),
        "full",
        grow,
        no_progress(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, TransferError::Unsupported));
    let _ = std::fs::remove_dir_all(dir);
}
