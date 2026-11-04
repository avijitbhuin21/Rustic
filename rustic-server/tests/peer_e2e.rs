//! End-to-end peer sync between two real rustic-server instances on
//! localhost: add by URL, pair (approved via the other side's event hub),
//! share, browse, preview, batch-approved file pull with conflict
//! auto-rename, auto-accepted upload, version gate, and forget-on-both-sides.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustic_app::config::ServerConfig;
use serde_json::{json, Value};

static SEQ: AtomicU16 = AtomicU16::new(0);

/// Fresh config with its own data dir.
fn config() -> ServerConfig {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("rustic-peer-e2e-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ServerConfig {
        auth_password: "pw".into(),
        session_secret: b"peer-e2e-secret".to_vec(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.clone(),
        static_dir: dir,
        session_ttl_secs: 3600,
        login_max_attempts: 50,
        login_lockout_secs: 1,
        preview_domain: None,
        cookie_domain: None,
    }
}

/// A running server: its base URL, login token and shared state.
struct Node {
    base: String,
    token: String,
    shared: Arc<rustic_server::app::Shared>,
    http: reqwest::Client,
}

impl Node {
    /// Start a server on a random localhost port and log in.
    async fn start() -> Node {
        let shared = rustic_server::build_shared(config()).unwrap();
        let router = rustic_server::app::build_router(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        let base = format!("http://{addr}");
        let http = reqwest::Client::new();
        let login: Value = http
            .post(format!("{base}/login"))
            .json(&json!({ "password": "pw" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let token = login["token"].as_str().unwrap().to_string();
        Node { base, token, shared, http }
    }

    /// Call a command; panics with the server's error text on failure.
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.try_call(cmd, args).await.unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    /// Call a command, returning the error text on failure.
    async fn try_call(&self, cmd: &str, args: Value) -> Result<Value, String> {
        let resp = self
            .http
            .post(format!("{}/api/{cmd}", self.base))
            .bearer_auth(&self.token)
            .json(&args)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let ok = resp.status().is_success();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if ok { Ok(body) } else { Err(body["error"].as_str().unwrap_or("?").to_string()) }
    }
}

/// Wait for `event` on a hub subscription and return its payload. The hub
/// fans out pre-serialized `EventMsg` JSON, so each message is parsed here.
async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<str>>, event: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match rx.recv().await {
                Ok(text) => {
                    let mut msg: Value = serde_json::from_str(&text).expect("hub message is JSON");
                    if msg["event"] == event {
                        return msg["payload"].take();
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(e) => panic!("hub closed: {e}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {event}"))
}

/// Pair `a` with `b` (B approves via its hub) and return (b_id on A, a_id on B).
async fn pair(a: &Arc<Node>, b: &Arc<Node>) -> (String, String) {
    a.call("lan_set_enabled", json!({ "enabled": true })).await;
    b.call("lan_set_enabled", json!({ "enabled": true })).await;
    let mut b_events = b.shared.ctx.hub.subscribe();
    let dev = a.call("lan_add_manual", json!({ "address": b.base })).await;
    let b_id = dev["device_id"].as_str().unwrap().to_string();
    let pair = {
        let (a, id) = (a.clone(), b_id.clone());
        tokio::spawn(async move { a.try_call("lan_pair", json!({ "deviceId": id })).await })
    };
    let req = next_event(&mut b_events, "lan-pair-request").await;
    b.call("lan_respond_pair", json!({ "requestId": req["request_id"], "accept": true })).await;
    pair.await.unwrap().expect("pairing should succeed");
    let devices = b.call("lan_devices", json!({})).await;
    let a_id = devices.as_array().unwrap().iter().find(|d| d["paired"] == json!(true)).unwrap()["device_id"]
        .as_str()
        .unwrap()
        .to_string();
    (b_id, a_id)
}

/// Deterministic pseudo-random bytes.
fn blob(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8 ^ seed).collect()
}

/// The transfers tray entry `pred` picks, polled until it appears.
async fn wait_transfer(node: &Node, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        let list = node.call("lan_transfers", json!({})).await;
        if let Some(t) = list.as_array().unwrap().iter().rev().find(|t| pred(t)) {
            return t.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("transfer never matched");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfers_pause_survive_network_change_and_resume_from_offsets() {
    let a = Arc::new(Node::start().await);
    let b = Arc::new(Node::start().await);
    let (b_id, a_id) = pair(&a, &b).await;
    let mut b_events = b.shared.ctx.hub.subscribe();

    let proj_dir = b.shared.config.data_dir.join("big-proj");
    std::fs::create_dir_all(proj_dir.join("data")).unwrap();
    let big = blob(24 * 1024 * 1024, 7);
    std::fs::write(proj_dir.join("data/big.bin"), &big).unwrap();
    std::fs::write(proj_dir.join("data/small.txt"), "tiny").unwrap();
    let project = b.call("add_project", json!({ "path": proj_dir.to_string_lossy() })).await;
    let pid = project["id"].as_str().unwrap().to_string();
    b.call("lan_set_share", json!({ "deviceId": a_id, "share": { "projects": [pid], "meta": [] } })).await;

    // 1) Files in a shared project need no second approval: B auto-accepts
    //    (no prompt), and a user pause / resume still lands the files intact.
    let dest = a.shared.config.data_dir.join("pulled");
    let items = json!([{ "project_id": pid, "project_name": "big-proj", "path": "data", "is_dir": true }]);
    let pull = {
        let (a, id, items, dest) = (a.clone(), b_id.clone(), items.clone(), dest.to_string_lossy().into_owned());
        tokio::spawn(async move { a.try_call("lan_pull_files", json!({ "deviceId": id, "items": items, "destDir": dest })).await })
    };
    let accepted = next_event(&mut b_events, "lan-transfer-auto-accepted").await;
    assert_eq!(accepted["kind"], json!("pull"), "{accepted}");
    assert_eq!(accepted["files"][0]["path"], json!("data"));
    let t = wait_transfer(&a, |t| t["direction"] == json!("pull") && t["label"] == json!("data")).await;
    assert_eq!(t["can_pause"], json!(true));
    assert_eq!(t["total"].as_u64(), Some(big.len() as u64 + 4), "size known up front: {t}");
    let tid = t["id"].as_str().unwrap().to_string();
    // The pull may already be far along on loopback; pausing must never corrupt it.
    let _ = a.try_call("lan_transfer_pause", json!({ "id": tid })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 2) Resume, and immediately simulate a network switch: the transfer
    //    pauses for the lost connection, then carries on by itself.
    let _ = a.try_call("lan_transfer_resume", json!({ "id": tid })).await;
    rustic_app::transfers::network_changed();
    let summary = tokio::time::timeout(Duration::from_secs(60), pull).await.expect("pull finished").unwrap().expect("pull ok");
    assert_eq!(summary["files"], json!(2));
    assert!(std::fs::read(dest.join("data/big.bin")).unwrap() == big, "resumed file must be byte-identical");
    assert!(!dest.join("data/big.bin.rustic-part").exists());
    let t = wait_transfer(&a, |t| t["id"] == json!(tid)).await;
    assert_eq!(t["state"], json!("done"), "{t}");

    // 3) Protocol-level resume: read from an offset, and upload in two halves.
    let peer = rustic_app::peer::load_peers(&a.shared.config.data_dir)
        .into_iter()
        .find(|p| p.device_id == b_id)
        .unwrap();
    let http = reqwest::Client::new();
    let lan = format!("{}/lan", b.base);
    let ver = rustic_app::peer::app_version();
    // Files in the shared project are approved without a prompt on B.
    let approved: Value = http
        .post(format!("{lan}/request"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .json(&json!({ "kind": "pull", "files": [{ "project_id": pid, "path": "data", "is_dir": true }] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(approved["approved"], json!(true), "shared files auto-approve: {approved}");
    let pull_ticket = approved["ticket"].as_str().expect("pull ticket").to_string();

    // Files in a project B does NOT share are refused outright (never auto-approved).
    let private_dir = b.shared.config.data_dir.join("private-proj");
    std::fs::create_dir_all(&private_dir).unwrap();
    std::fs::write(private_dir.join("secret.txt"), "nope").unwrap();
    let private = b.call("add_project", json!({ "path": private_dir.to_string_lossy() })).await;
    let private_id = private["id"].as_str().unwrap().to_string();
    let denied = http
        .post(format!("{lan}/request"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .json(&json!({ "kind": "pull", "files": [{ "project_id": private_id, "path": "secret.txt", "is_dir": false }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status().as_u16(), 403, "unshared files must not be approved");
    let body: Value = denied.json().await.unwrap();
    assert!(body["error"].as_str().unwrap_or("").contains("not shared"), "{body}");
    let tail = http
        .post(format!("{lan}/api/fs/read"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .header("x-rustic-ticket", &pull_ticket)
        .json(&json!({ "project_id": pid, "path": "data/big.bin", "offset": 1000 }))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(tail[..] == big[1000..], "read from offset");

    let push: Value = http
        .post(format!("{lan}/request"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .json(&json!({ "kind": "push", "files": [{ "project_id": pid, "path": "", "is_dir": true, "names": ["up.bin"] }] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let push_ticket = push["ticket"].as_str().expect("server auto-accepts pushes").to_string();
    let payload = blob(3 * 1024 * 1024 + 17, 3);
    let begin: Value = http
        .post(format!("{lan}/api/fs/upload/begin"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .header("x-rustic-ticket", &push_ticket)
        .json(&json!({ "project_id": pid, "dir": "", "entries": [{ "rel": "up.bin", "is_dir": false, "size": payload.len() }] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = begin["id"].as_str().unwrap().to_string();
    let half = payload.len() / 2;
    let send = |offset: usize, bytes: Vec<u8>| {
        let (http, lan, token, sid) = (http.clone(), lan.clone(), peer.token_out.clone(), sid.clone());
        async move {
            http.post(format!("{lan}/api/fs/upload/chunk"))
                .bearer_auth(&token)
                .header("x-rustic-version", ver)
                .header("x-rustic-chunk", json!({ "id": sid, "rel": "up.bin", "offset": offset }).to_string())
                .body(bytes)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let r = send(0, payload[..half].to_vec()).await;
    assert_eq!(r["received"].as_u64(), Some(half as u64));
    let off: Value = http
        .post(format!("{lan}/api/fs/upload/offset"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .json(&json!({ "id": sid, "rel": "up.bin" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(off["offset"].as_u64(), Some(half as u64), "receiver reports the resume offset");
    let r = send(half, payload[half..].to_vec()).await;
    assert_eq!(r["received"].as_u64(), Some(payload.len() as u64));
    let fin: Value = http
        .post(format!("{lan}/api/fs/upload/finish"))
        .bearer_auth(&peer.token_out)
        .header("x-rustic-version", ver)
        .json(&json!({ "id": sid }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fin["files"], json!(1));
    assert!(std::fs::read(proj_dir.join("up.bin")).unwrap() == payload, "uploaded halves reassemble exactly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_servers_pair_browse_pull_push_and_forget() {
    let a = Arc::new(Node::start().await);
    let b = Arc::new(Node::start().await);
    a.call("lan_set_enabled", json!({ "enabled": true })).await;
    b.call("lan_set_enabled", json!({ "enabled": true })).await;
    let mut b_events = b.shared.ctx.hub.subscribe();

    // Add B by URL; versions match.
    let dev = a.call("lan_add_manual", json!({ "address": b.base })).await;
    let b_id = dev["device_id"].as_str().unwrap().to_string();
    assert_eq!(dev["update_required"], json!(false), "same build must not need an update: {dev}");

    // Pair: B approves through its prompt event.
    let pair = {
        let a = a.clone();
        let id = b_id.clone();
        tokio::spawn(async move { a.try_call("lan_pair", json!({ "deviceId": id })).await })
    };
    let req = next_event(&mut b_events, "lan-pair-request").await;
    b.call("lan_respond_pair", json!({ "requestId": req["request_id"], "accept": true })).await;
    pair.await.unwrap().expect("pairing should succeed");

    // B shares a project with A.
    let proj_dir = b.shared.config.data_dir.join("shared-proj");
    std::fs::create_dir_all(proj_dir.join("src/inner")).unwrap();
    std::fs::write(proj_dir.join("src/inner/a.rs"), "fn a() {}").unwrap();
    std::fs::write(proj_dir.join("README.md"), "hello from B").unwrap();
    let project = b.call("add_project", json!({ "path": proj_dir.to_string_lossy() })).await;
    let pid = project["id"].as_str().unwrap().to_string();
    let b_devices = b.call("lan_devices", json!({})).await;
    let a_id = b_devices.as_array().unwrap().iter().find(|d| d["paired"] == json!(true)).expect("A paired on B")["device_id"]
        .as_str()
        .unwrap()
        .to_string();
    b.call("lan_set_share", json!({ "deviceId": a_id, "share": { "projects": [pid], "meta": [] } })).await;

    // Browse + preview without approval.
    let projects = a.call("lan_list_projects", json!({ "deviceId": b_id })).await;
    assert!(projects.as_array().unwrap().iter().any(|p| p["id"] == json!(pid)), "shared project listed: {projects}");
    let root = a.call("lan_list_files", json!({ "deviceId": b_id, "projectId": pid, "path": "" })).await;
    let entries = root.as_array().unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"src") && names.contains(&"README.md"), "listing: {names:?}");
    let first_file = entries.iter().position(|e| e["is_dir"] == json!(false)).unwrap();
    assert!(entries[..first_file].iter().all(|e| e["is_dir"] == json!(true)), "folders first: {names:?}");
    assert!(entries[first_file..].iter().all(|e| e["is_dir"] == json!(false)), "folders first: {names:?}");
    let pv = a.call("lan_preview_file", json!({ "deviceId": b_id, "projectId": pid, "path": "README.md" })).await;
    assert_eq!(pv["content"], json!("hello from B"));

    // Pull a folder with one approval on B.
    let dest = a.shared.config.data_dir.join("imports");
    let items = json!([{ "project_id": pid, "project_name": "shared-proj", "path": "src", "is_dir": true }]);
    let size = a.call("lan_remote_size", json!({ "deviceId": b_id, "items": items })).await;
    assert_eq!(size["files"], json!(1));
    for round in 0..2 {
        let pull = {
            let a = a.clone();
            let (id, items, dest) = (b_id.clone(), items.clone(), dest.to_string_lossy().into_owned());
            tokio::spawn(async move {
                a.try_call("lan_pull_files", json!({ "deviceId": id, "items": items, "destDir": dest, "opts": { "policy": "auto_rename" } }))
                    .await
            })
        };
        // Shared project: B auto-accepts, no prompt.
        let accepted = next_event(&mut b_events, "lan-transfer-auto-accepted").await;
        assert_eq!(accepted["kind"], json!("pull"));
        assert_eq!(accepted["files"][0]["path"], json!("src"));
        let summary = pull.await.unwrap().expect("pull should succeed");
        assert_eq!(summary["files"], json!(1), "round {round}: {summary}");
    }
    assert_eq!(std::fs::read_to_string(dest.join("src/inner/a.rs")).unwrap(), "fn a() {}");
    assert!(dest.join("src-1/inner/a.rs").exists(), "second pull must auto-rename, not overwrite");

    // Transfers tray recorded both pulls as done with their location.
    let transfers = a.call("lan_transfers", json!({})).await;
    let done = transfers.as_array().unwrap().iter().filter(|t| t["direction"] == json!("pull") && t["state"] == json!("done")).count();
    assert!(done >= 2, "transfers: {transfers}");

    // Upload to B (servers auto-accept pushes) into a subfolder.
    let up = a.shared.config.data_dir.join("upload.txt");
    std::fs::write(&up, "from A").unwrap();
    std::fs::create_dir_all(proj_dir.join("incoming")).unwrap();
    a.call(
        "lan_push_files",
        json!({ "deviceId": b_id, "localPaths": [up.to_string_lossy()], "projectId": pid, "dir": "incoming" }),
    )
    .await;
    assert_eq!(std::fs::read_to_string(proj_dir.join("incoming/upload.txt")).unwrap(), "from A");
    let clash = a
        .call("lan_remote_conflicts", json!({ "deviceId": b_id, "projectId": pid, "dir": "incoming", "names": ["upload.txt", "nope.txt"] }))
        .await;
    assert_eq!(clash, json!(["upload.txt"]));

    // Version gate: a peer request without our version is refused with 409.
    let r = reqwest::Client::new().post(format!("{}/lan/hello", b.base)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 409);
    let info: Value = reqwest::get(format!("{}/lan/info", b.base)).await.unwrap().json().await.unwrap();
    assert_eq!(info["version"].as_str(), Some(rustic_app::peer::app_version()));

    // Forget on A also unpairs on B.
    a.call("lan_forget", json!({ "deviceId": b_id })).await;
    let b_devices = b.call("lan_devices", json!({})).await;
    assert!(
        b_devices.as_array().unwrap().iter().all(|d| d["paired"] != json!(true)),
        "B should have forgotten A too: {b_devices}"
    );
}

/// Cancelling an outgoing pairing request stops our wait AND dismisses the
/// prompt on the other machine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_pairing_dismisses_the_other_sides_prompt() {
    let a = Arc::new(Node::start().await);
    let b = Arc::new(Node::start().await);
    a.call("lan_set_enabled", json!({ "enabled": true })).await;
    b.call("lan_set_enabled", json!({ "enabled": true })).await;
    let mut b_events = b.shared.ctx.hub.subscribe();
    let dev = a.call("lan_add_manual", json!({ "address": b.base })).await;
    let b_id = dev["device_id"].as_str().unwrap().to_string();
    let pair = {
        let (a, id) = (a.clone(), b_id.clone());
        tokio::spawn(async move { a.try_call("lan_pair", json!({ "deviceId": id })).await })
    };
    let req = next_event(&mut b_events, "lan-pair-request").await;
    a.call("lan_cancel_outgoing", json!({ "deviceId": b_id })).await;
    let gone = next_event(&mut b_events, "lan-pair-cancelled").await;
    assert_eq!(gone["request_id"], req["request_id"], "B's prompt is dismissed");
    let res = tokio::time::timeout(Duration::from_secs(10), pair).await.expect("pair returns promptly").unwrap();
    assert_eq!(res.unwrap_err(), "Pairing cancelled");
    // The answer is gone on B; nobody ended up paired.
    assert!(b.try_call("lan_respond_pair", json!({ "requestId": req["request_id"], "accept": true })).await.is_err());
    let devices = a.call("lan_devices", json!({})).await;
    assert!(devices.as_array().unwrap().iter().all(|d| d["paired"] != json!(true)), "{devices}");
}

/// A transfer waiting for approval can be cancelled (prompt dismissed on the
/// other side), and a running transfer can be cancelled without leaving
/// partial files behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_waiting_and_running_transfers() {
    let a = Arc::new(Node::start().await);
    let b = Arc::new(Node::start().await);
    let (b_id, a_id) = pair(&a, &b).await;
    let mut b_events = b.shared.ctx.hub.subscribe();
    let proj_dir = b.shared.config.data_dir.join("cancel-proj");
    std::fs::create_dir_all(proj_dir.join("cancel-me")).unwrap();
    for i in 0..8u8 {
        std::fs::write(proj_dir.join(format!("cancel-me/f{i}.bin")), blob(5 * 1024 * 1024, i)).unwrap();
    }
    let project = b.call("add_project", json!({ "path": proj_dir.to_string_lossy() })).await;
    let pid = project["id"].as_str().unwrap().to_string();
    b.call("lan_set_share", json!({ "deviceId": a_id, "share": { "projects": [pid], "meta": [] } })).await;

    // 1) Whole-project pull still prompts on B; cancel while it waits.
    let sync = {
        let (a, id, pid) = (a.clone(), b_id.clone(), pid.clone());
        tokio::spawn(async move {
            a.try_call("lan_sync_items", json!({ "deviceId": id, "direction": "pull", "projects": [{ "id": pid, "name": "cancel-proj" }], "meta": [] })).await
        })
    };
    let prompt = next_event(&mut b_events, "lan-transfer-request").await;
    a.call("lan_cancel_outgoing", json!({ "deviceId": b_id })).await;
    let gone = next_event(&mut b_events, "lan-transfer-cancelled").await;
    assert_eq!(gone["request_id"], prompt["request_id"], "B's approval prompt is dismissed");
    let res = tokio::time::timeout(Duration::from_secs(15), sync).await.expect("sync returns promptly").unwrap();
    assert!(res.is_err() || res.as_ref().is_ok_and(|v| v["failures"].as_array().is_some_and(|f| !f.is_empty())), "{res:?}");

    // 2) Running per-file pull: pause as soon as it shows up, then cancel.
    let dest = a.shared.config.data_dir.join("cancelled");
    let items = json!([{ "project_id": pid, "project_name": "cancel-proj", "path": "cancel-me", "is_dir": true }]);
    let pull = {
        let (a, id, items, dest) = (a.clone(), b_id.clone(), items, dest.to_string_lossy().into_owned());
        tokio::spawn(async move { a.try_call("lan_pull_files", json!({ "deviceId": id, "items": items, "destDir": dest })).await })
    };
    // The transfers registry is process-wide (parallel tests share it): match by label.
    let t = wait_transfer(&a, |t| t["direction"] == json!("pull") && t["label"] == json!("cancel-me")).await;
    let tid = t["id"].as_str().unwrap().to_string();
    let _ = a.try_call("lan_transfer_pause", json!({ "id": tid })).await;
    let cancel = a.try_call("lan_transfer_cancel", json!({ "id": tid })).await;
    let res = tokio::time::timeout(Duration::from_secs(30), pull).await.expect("cancelled pull returns").unwrap();
    let t = wait_transfer(&a, |t| t["id"] == json!(tid) && !matches!(t["state"].as_str(), Some("running" | "waiting"))).await;
    match t["state"].as_str() {
        // Loopback can finish 40 MB before the cancel lands; that's not a failure of cancel.
        Some("done") => assert!(res.is_ok(), "{res:?}"),
        Some("cancelled") => {
            assert!(cancel.is_ok(), "{cancel:?}");
            assert!(res.is_err(), "a cancelled pull reports an error: {res:?}");
            let leftovers: Vec<_> = walk(&dest).into_iter().filter(|p| p.to_string_lossy().ends_with(".rustic-part")).collect();
            assert!(leftovers.is_empty(), "no partial files after cancel: {leftovers:?}");
        }
        other => panic!("unexpected final state {other:?}: {t}"),
    }
}

/// Every file under `dir` (empty when it doesn't exist).
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)); } else { out.push(p); }
    }
    out
}

/// Reconnects: B's listener restarting keeps the pairing usable, and when B
/// lost the pairing on its own, A is told to re-pair (no "forget on both
/// sides" needed) and one new pairing restores browsing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_after_restart_and_one_sided_forget() {
    let a = Arc::new(Node::start().await);
    let b = Arc::new(Node::start().await);
    let (b_id, a_id) = pair(&a, &b).await;
    let proj_dir = b.shared.config.data_dir.join("re-proj");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join("x.txt"), "x").unwrap();
    let project = b.call("add_project", json!({ "path": proj_dir.to_string_lossy() })).await;
    let pid = project["id"].as_str().unwrap().to_string();
    b.call("lan_set_share", json!({ "deviceId": a_id, "share": { "projects": [pid], "meta": [] } })).await;
    let listed = |v: &Value| v.as_array().is_some_and(|l| l.iter().any(|p| p["id"] == json!(pid)));
    assert!(listed(&a.call("lan_list_projects", json!({ "deviceId": b_id })).await));

    // 1) B restarts sync (listener off → on): A keeps working without re-pairing.
    b.call("lan_set_enabled", json!({ "enabled": false })).await;
    b.call("lan_set_enabled", json!({ "enabled": true })).await;
    let mut back = false;
    for _ in 0..40 {
        if a.try_call("lan_list_projects", json!({ "deviceId": b_id })).await.is_ok_and(|v| listed(&v)) {
            back = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(back, "A reaches B again after B's listener restarted, without re-pairing");

    // 2) B loses the pairing on its own (reinstall / wiped data).
    rustic_app::peer::save_peers(&b.shared.config.data_dir, &[]).unwrap();
    let err = a.try_call("lan_list_projects", json!({ "deviceId": b_id })).await.unwrap_err();
    assert!(!err.is_empty());
    let devices = a.call("lan_devices", json!({})).await;
    let d = devices.as_array().unwrap().iter().find(|d| d["device_id"] == json!(b_id)).expect("B still listed on A").clone();
    assert_eq!(d["needs_repair"], json!(true), "A flags B for re-pairing: {d}");

    // One re-pair from A (B approves) is enough; no forgetting on either side.
    let mut b_events = b.shared.ctx.hub.subscribe();
    let pair = {
        let (a, id) = (a.clone(), b_id.clone());
        tokio::spawn(async move { a.try_call("lan_pair", json!({ "deviceId": id })).await })
    };
    let req = next_event(&mut b_events, "lan-pair-request").await;
    b.call("lan_respond_pair", json!({ "requestId": req["request_id"], "accept": true })).await;
    pair.await.unwrap().expect("re-pair succeeds");
    let b_devices = b.call("lan_devices", json!({})).await;
    let a_id = b_devices.as_array().unwrap().iter().find(|d| d["paired"] == json!(true)).expect("A paired on B again")["device_id"]
        .as_str()
        .unwrap()
        .to_string();
    b.call("lan_set_share", json!({ "deviceId": a_id, "share": { "projects": [pid], "meta": [] } })).await;
    assert!(listed(&a.call("lan_list_projects", json!({ "deviceId": b_id })).await), "browsing works after one re-pair");
    let devices = a.call("lan_devices", json!({})).await;
    let d = devices.as_array().unwrap().iter().find(|d| d["device_id"] == json!(b_id)).unwrap().clone();
    assert_eq!(d["needs_repair"], json!(false), "{d}");
}
