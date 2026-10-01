//! Peer sync (issue #15), shared by the desktop app and rustic-server:
//! discover other Rustic instances (mDNS), pair them with an Accept/Decline
//! prompt plus a matching 6-digit code, and sync projects / metadata with
//! per-transfer approval tickets and a per-peer sharing allowlist.
//!
//! Everything host-specific (data dir, state, events, secrets, where incoming
//! projects land) goes through [`PeerHost`]. Pieces:
//! - [`listener`]: the peer HTTP routes ([`listener::build_router`]) and the
//!   optional TLS listener on [`DEFAULT_PORT`] ([`listener::start`]).
//! - [`ops`]: the operations behind the `lan_*` commands.
//! - [`client`]: push / pull / metadata client shared with the
//!   password-authenticated remote-backend sync.
//! - [`discovery`] (mDNS) and [`tunnel`] (Cloudflare quick tunnel): optional.

pub mod client;
pub mod consent;
pub mod discovery;
pub mod listener;
pub mod ops;
pub mod tunnel;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What the peer-sync code needs from the process hosting it (desktop app or
/// rustic-server). Shared as [`HostRef`].
pub trait PeerHost: Send + Sync + 'static {
    /// Shared application state (workspace, DB, …).
    fn state(&self) -> &crate::state::AppState;
    /// Application data dir (`<data>/lan/*` lives under it).
    fn data_dir(&self) -> Result<PathBuf, String>;
    /// User home dir; the default landing place is `~/projects/<name>`.
    fn home_dir(&self) -> PathBuf;
    /// Event sink for `lan-*` and `rustic:sync-progress` events.
    fn emitter(&self) -> Arc<dyn crate::EventEmitter>;
    /// Secret store used by metadata / full-environment sync (API keys).
    fn secrets(&self) -> &dyn crate::secrets::SecretStore;
    /// Where an incoming project lands: its existing local root (`old`), the
    /// sender's path when native to this OS, else `~/projects/<name>`.
    fn incoming_root(
        &self,
        entry: &crate::cloud_sync::SyncProjectEntry,
        old: Option<&str>,
    ) -> PathBuf {
        if let Some(old) = old {
            return PathBuf::from(old);
        }
        if client::path_is_native(&entry.origin_root_path) {
            return PathBuf::from(&entry.origin_root_path);
        }
        self.home_dir()
            .join("projects")
            .join(crate::cloud_sync::safe_dir_name(&entry.name))
    }
    /// Approve incoming PUSH requests without prompting (headless server).
    /// Pulls always prompt and are limited by the sharing allowlist.
    fn auto_accept_push(&self) -> bool {
        false
    }
    /// Machine name used when the user hasn't set one.
    fn default_device_name(&self) -> String {
        hostname()
    }
}

/// Shared handle to the host.
pub type HostRef = Arc<dyn PeerHost>;

/// Hostname from the environment, or "Rustic".
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Rustic".to_string())
}

/// mDNS service type advertised by listening instances.
pub const SERVICE_TYPE: &str = "_rustic-sync._tcp.local.";

/// Listener port tried first so saved addresses stay valid across restarts;
/// falls back to an OS-assigned port when it's taken.
pub const DEFAULT_PORT: u16 = 47820;

/// Header carrying the caller's own listener port, so the callee learns a
/// reachable address for it even when mDNS doesn't work in that direction.
pub const PORT_HEADER: &str = "x-rustic-lan-port";

/// Header carrying the caller's public tunnel URL (when it runs one), so the
/// callee can reach it back over the internet.
pub const URL_HEADER: &str = "x-rustic-lan-url";

/// Header carrying an approval ticket for a push / pull.
pub const TICKET_HEADER: &str = "x-rustic-ticket";

/// Header naming the project a push upload carries (checked against the ticket).
pub const PROJECT_HEADER: &str = "x-rustic-project";

/// This machine's public tunnel URL, sent in [`URL_HEADER`].
static PUBLIC_URL: Mutex<Option<String>> = Mutex::new(None);

/// Record (or clear) the public tunnel URL advertised to peers.
pub fn set_public_url(url: Option<String>) {
    *PUBLIC_URL.lock().unwrap_or_else(|p| p.into_inner()) = url;
}

/// A paired device counts as online this long after it last talked to us
/// or answered a probe.
pub const ONLINE_WINDOW: std::time::Duration = std::time::Duration::from_secs(45);

/// This machine's listener port, read by [`peer_client`] for [`PORT_HEADER`].
static LISTEN_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// Record the port the listener actually bound (0 when stopped).
pub fn set_listen_port(port: u16) {
    LISTEN_PORT.store(port, std::sync::atomic::Ordering::Relaxed);
}

/// This install's LAN identity: stable device id, display name and the
/// self-signed TLS certificate other devices pin during pairing.
#[derive(Clone)]
pub struct Identity {
    pub device_id: String,
    pub device_name: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    pub fingerprint: String,
}

/// A device this machine has paired with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub device_id: String,
    pub name: String,
    /// SHA-256 of the peer's TLS certificate (pinned).
    pub fingerprint: String,
    /// Token we present when calling the peer.
    pub token_out: String,
    /// Token the peer presents when calling us.
    pub token_in: String,
    /// Last known `ip:port`.
    #[serde(default)]
    pub addr: Option<String>,
    /// User-chosen display name; overrides `name` in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    /// What this device may see and pull from us. Empty = nothing (default).
    #[serde(default)]
    pub share: Share,
}

/// Per-device sharing allowlist: project ids and metadata keys (`category/name`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Share {
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub meta: Vec<String>,
}

impl Share {
    /// Whether project `id` is shared.
    pub fn has_project(&self, id: &str) -> bool {
        self.projects.iter().any(|p| p == id)
    }

    /// Whether metadata item `key` is shared.
    pub fn has_meta(&self, key: &str) -> bool {
        self.meta.iter().any(|k| k == key)
    }
}

impl Peer {
    /// Name to show: the nickname when set, else the device's own name.
    pub fn display_name(&self) -> String {
        self.nickname
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// Last time a paired device proved it was reachable, and from where.
#[derive(Debug, Clone)]
pub struct Seen {
    pub at: std::time::Instant,
    pub addr: Option<String>,
}

/// A device currently advertising itself on the network.
#[derive(Debug, Clone, Serialize)]
pub struct Discovered {
    pub device_id: String,
    pub name: String,
    pub fingerprint: String,
    pub addr: String,
    #[serde(skip)]
    pub fullname: String,
}

/// Live LAN state shared by the listener, discovery thread and commands.
#[derive(Default)]
pub struct LanInner {
    pub identity: Option<Identity>,
    pub port: u16,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub mdns: Option<mdns_sd::ServiceDaemon>,
    pub discovered: HashMap<String, Discovered>,
    /// Devices added by address ("Add machine") rather than found via mDNS.
    pub manual: HashMap<String, Discovered>,
    pub seen: HashMap<String, Seen>,
    /// When each paired device was last probed, to rate-limit probes.
    pub probed: HashMap<String, std::time::Instant>,
    pub pending_pairs: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    /// Push/pull approval prompts awaiting the user's answer.
    pub pending_transfers: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    /// Approved transfers: ticket id → what it allows.
    pub tickets: HashMap<String, consent::Ticket>,
    /// Running Cloudflare quick tunnel exposing the listener, if any.
    pub tunnel: Option<tunnel::Tunnel>,
}

impl LanInner {
    /// A not-yet-paired device found by mDNS or added by address.
    pub fn candidate(&self, device_id: &str) -> Option<Discovered> {
        self.discovered
            .get(device_id)
            .or_else(|| self.manual.get(device_id))
            .cloned()
    }

    /// Mark a device reachable now, optionally at `addr`.
    pub fn mark_seen(&mut self, device_id: &str, addr: Option<String>) {
        let addr = addr.or_else(|| self.seen.get(device_id).and_then(|s| s.addr.clone()));
        self.seen.insert(
            device_id.to_string(),
            Seen {
                at: std::time::Instant::now(),
                addr,
            },
        );
    }

    /// Whether the device answered or called within [`ONLINE_WINDOW`].
    pub fn recently_seen(&self, device_id: &str) -> bool {
        self.seen
            .get(device_id)
            .is_some_and(|s| s.at.elapsed() < ONLINE_WINDOW)
    }
}

/// Tauri-managed handle to [`LanInner`].
#[derive(Default, Clone)]
pub struct LanState(pub Arc<Mutex<LanInner>>);

impl LanState {
    /// Poison-tolerant lock.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, LanInner> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// `<data_dir>/lan`.
pub fn lan_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("lan")
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn fingerprint(bytes: &[u8]) -> String {
    let h = Sha256::digest(bytes);
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// 6-digit code both sides display during pairing, derived from both
/// certificate fingerprints (order-independent). A man-in-the-middle presents
/// a different certificate, so the two screens would show different codes.
pub fn pairing_code(a: &str, b: &str) -> String {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    let h = Sha256::digest(format!("{x}|{y}").as_bytes());
    let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
    format!("{n:06}")
}

/// 32 random bytes as hex, for pairing tokens.
pub fn random_token() -> Result<String, String> {
    let mut buf = [0u8; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut buf)
        .map_err(|_| "secure random unavailable".to_string())?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Human-readable name for this machine: the user's custom name when set,
/// else `default`.
fn machine_name(data_dir: &Path, default: String) -> String {
    custom_device_name(data_dir).unwrap_or(default)
}

/// User-chosen name for this machine (`<data_dir>/lan/name`), if any.
pub fn custom_device_name(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(lan_dir(data_dir).join("name"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist (or clear, when empty) this machine's custom name.
pub fn set_custom_device_name(data_dir: &Path, name: &str) -> Result<(), String> {
    let dir = lan_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("name");
    let name = name.trim();
    if name.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        std::fs::write(&path, name.as_bytes()).map_err(|e| e.to_string())
    }
}

/// This machine's primary LAN IPv4 address. Connecting a UDP socket sends no
/// packets; it only asks the OS which interface routes outward.
pub fn local_ipv4() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            Some(ip.to_string())
        }
        _ => None,
    }
}

/// Load this install's identity (hostname as default name).
pub fn load_or_create_identity(data_dir: &Path) -> Result<Identity, String> {
    load_or_create_identity_named(data_dir, hostname())
}

/// Load this install's identity, generating a certificate on first use;
/// `default_name` applies when no custom name is set.
pub fn load_or_create_identity_named(
    data_dir: &Path,
    default_name: String,
) -> Result<Identity, String> {
    let dir = lan_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cert_path = dir.join("cert.der");
    let key_path = dir.join("key.der");
    let (cert_der, key_der) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(c), Ok(k)) if !c.is_empty() && !k.is_empty() => (c, k),
        _ => {
            let ck = rcgen::generate_simple_self_signed(vec!["rustic-lan.local".to_string()])
                .map_err(|e| format!("certificate generation failed: {e}"))?;
            let c = ck.cert.der().to_vec();
            let k = ck.key_pair.serialize_der();
            std::fs::write(&cert_path, &c).map_err(|e| e.to_string())?;
            std::fs::write(&key_path, &k).map_err(|e| e.to_string())?;
            (c, k)
        }
    };
    let fp = fingerprint(&cert_der);
    Ok(Identity {
        device_id: fp[..16].to_string(),
        device_name: machine_name(data_dir, default_name),
        cert_der,
        key_der,
        fingerprint: fp,
    })
}

/// Paired devices from `<data_dir>/lan/peers.json`.
pub fn load_peers(data_dir: &Path) -> Vec<Peer> {
    std::fs::read_to_string(lan_dir(data_dir).join("peers.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist paired devices.
pub fn save_peers(data_dir: &Path, peers: &[Peer]) -> Result<(), String> {
    let dir = lan_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(peers).map_err(|e| e.to_string())?;
    rustic_agent::io_util::atomic_write(&dir.join("peers.json"), text.as_bytes())
        .map_err(|e| e.to_string())
}

/// Insert or replace a paired device.
pub fn upsert_peer(data_dir: &Path, peer: Peer) -> Result<(), String> {
    let mut peers = load_peers(data_dir);
    peers.retain(|p| p.device_id != peer.device_id);
    peers.push(peer);
    save_peers(data_dir, &peers)
}

/// Whether "Allow local-network sync" is switched on (persisted flag file).
pub fn is_enabled_persisted(data_dir: &Path) -> bool {
    lan_dir(data_dir).join("enabled").exists()
}

/// How this machine is reachable from outside the LAN: `"cloudflare"`,
/// `"portforward"`, or `None` (LAN only). Persisted in `<data>/lan/internet`.
pub fn internet_mode(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(lan_dir(data_dir).join("internet"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s == "cloudflare" || s == "portforward")
}

/// Persist the internet-reachability mode (`None` clears it).
pub fn set_internet_mode(data_dir: &Path, mode: Option<&str>) -> Result<(), String> {
    let dir = lan_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("internet");
    match mode {
        Some(m) => std::fs::write(&path, m.as_bytes()).map_err(|e| e.to_string()),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

/// Persist the "Allow local-network sync" switch.
pub fn set_enabled_persisted(data_dir: &Path, enabled: bool) -> Result<(), String> {
    let dir = lan_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let flag = dir.join("enabled");
    if enabled {
        std::fs::write(&flag, b"1").map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&flag) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// TLS server config presenting this install's certificate.
pub fn server_tls_config(id: &Identity) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cert = CertificateDer::from(id.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der.clone()));
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| format!("TLS config failed: {e}"))?;
    Ok(Arc::new(cfg))
}

/// Accepts exactly one certificate: the one whose fingerprint was pinned.
#[derive(Debug)]
struct PinnedVerifier {
    expected: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity.as_ref()) == self.expected {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "this device's certificate does not match the paired device — re-pair it".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Accepts any certificate but records its fingerprint. Used only to read
/// `/lan/info` from a manually entered address; the device is then pinned to
/// the captured fingerprint and pairing still requires the matching 6-digit
/// code on both screens, which a man-in-the-middle can't reproduce.
#[derive(Debug)]
struct CaptureVerifier {
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for CaptureVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(fingerprint(end_entity.as_ref()));
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Client for a manually entered address plus the slot that receives the
/// certificate fingerprint it presents.
pub fn capture_client() -> Result<(reqwest::Client, Arc<Mutex<Option<String>>>), String> {
    let seen = Arc::new(Mutex::new(None));
    let client = build_client(Arc::new(CaptureVerifier {
        seen: seen.clone(),
        provider: Arc::new(rustls::crypto::ring::default_provider()),
    }))?;
    Ok((client, seen))
}

/// reqwest client over `verifier`, advertising our listener port.
fn build_client(
    verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
) -> Result<reqwest::Client, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let mut headers = reqwest::header::HeaderMap::new();
    let port = LISTEN_PORT.load(std::sync::atomic::Ordering::Relaxed);
    if port != 0 {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&port.to_string()) {
            headers.insert(PORT_HEADER, v);
        }
    }
    let public = PUBLIC_URL.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(url) = public {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&url) {
            headers.insert(URL_HEADER, v);
        }
    }
    reqwest::Client::builder()
        .use_preconfigured_tls(cfg)
        .default_headers(headers)
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// Whether `addr` is a public tunnel URL rather than a LAN `host:port`.
pub fn is_url_addr(addr: &str) -> bool {
    addr.starts_with("https://") || addr.starts_with("http://")
}

/// Base URL for requests to `addr` (LAN `host:port` or a tunnel URL).
pub fn base_url(addr: &str) -> String {
    if is_url_addr(addr) {
        addr.trim_end_matches('/').to_string()
    } else {
        format!("https://{addr}")
    }
}

/// Client for a paired device at `addr`, sending `extra` headers (ticket,
/// project) on every request. LAN addresses pin the device's self-signed
/// certificate; tunnel URLs terminate at Cloudflare, so they use normal
/// WebPKI TLS and rely on the paired-device token for identity.
pub fn peer_client(
    expected_fp: &str,
    addr: &str,
    extra: &[(&'static str, String)],
) -> Result<reqwest::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    let port = LISTEN_PORT.load(std::sync::atomic::Ordering::Relaxed);
    if port != 0 {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&port.to_string()) {
            headers.insert(PORT_HEADER, v);
        }
    }
    if let Some(url) = PUBLIC_URL.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&url) {
            headers.insert(URL_HEADER, v);
        }
    }
    for (k, v) in extra {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(v) {
            headers.insert(*k, v);
        }
    }
    let builder = reqwest::Client::builder()
        .default_headers(headers)
        .connect_timeout(std::time::Duration::from_secs(15));
    let builder = if is_url_addr(addr) {
        builder
    } else {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
                expected: expected_fp.to_string(),
                provider,
            }))
            .with_no_client_auth();
        builder.use_preconfigured_tls(cfg)
    };
    builder.build().map_err(|e| e.to_string())
}

/// Base for the token-authenticated peer API on `addr`: every peer route is
/// served under `/lan` (rustic-server owns the root `/api/sync/*`).
pub fn api_base(addr: &str) -> String {
    format!("{}/lan", base_url(addr))
}

/// Normalise a user-typed address. `http://…` and `https://host` (no port)
/// are kept whole as URL addresses (a rustic-server or tunnel URL, WebPKI /
/// plain HTTP). Anything else becomes a pinned LAN `host:port` (default port
/// when omitted), including `https://host:port`.
pub fn normalize_addr(input: &str) -> Result<String, String> {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("http://") {
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || rest.contains(char::is_whitespace) {
            return Err("Enter an address like http://192.168.1.20:8787".into());
        }
        return Ok(format!("http://{rest}"));
    }
    if let Some(rest) = s.strip_prefix("https://") {
        let host = rest.trim_end_matches('/');
        if !host.is_empty() && !host.contains(':') && !host.contains('/') && host.contains('.') {
            return Ok(format!("https://{host}"));
        }
    }
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.trim_end_matches('/');
    if s.is_empty() || s.contains('/') || s.contains(char::is_whitespace) {
        return Err("Enter an address like 192.168.1.20 or 192.168.1.20:47820".into());
    }
    let has_port = s
        .rsplit_once(':')
        .is_some_and(|(h, p)| !h.is_empty() && !h.contains(':') && p.parse::<u16>().is_ok());
    Ok(if has_port {
        s.to_string()
    } else {
        format!("{s}:{DEFAULT_PORT}")
    })
}

/// Whether `addr` (`ip:port`) points at this computer: loopback or `own_ip`.
pub fn is_this_host(addr: &str, own_ip: Option<&str>) -> bool {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return true;
    }
    own_ip.is_some_and(|ip| ip == host)
}

/// Same host as `addr`, on [`DEFAULT_PORT`] (for addresses saved before the
/// listener used a fixed port).
pub fn with_default_port(addr: &str) -> Option<String> {
    let (host, port) = addr.rsplit_once(':')?;
    (port != DEFAULT_PORT.to_string()).then(|| format!("{host}:{DEFAULT_PORT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_addr_adds_default_port() {
        assert_eq!(normalize_addr("192.168.1.5").unwrap(), "192.168.1.5:47820");
        assert_eq!(normalize_addr(" https://desk.local:5000/ ").unwrap(), "desk.local:5000");
        assert_eq!(normalize_addr("https://abc-def.trycloudflare.com/").unwrap(), "https://abc-def.trycloudflare.com");
        assert!(is_url_addr("https://abc-def.trycloudflare.com"));
        assert_eq!(base_url("10.0.0.2:47820"), "https://10.0.0.2:47820");
        assert_eq!(normalize_addr("http://127.0.0.1:8787/").unwrap(), "http://127.0.0.1:8787");
        assert!(is_url_addr("http://127.0.0.1:8787"));
        assert_eq!(normalize_addr("https://server.example.com").unwrap(), "https://server.example.com");
        assert_eq!(api_base("http://127.0.0.1:8787"), "http://127.0.0.1:8787/lan");
        assert_eq!(api_base("10.0.0.2:47820"), "https://10.0.0.2:47820/lan");
        assert!(normalize_addr("").is_err());
        assert!(normalize_addr("a b").is_err());
        assert_eq!(with_default_port("10.0.0.2:51234").as_deref(), Some("10.0.0.2:47820"));
        assert_eq!(with_default_port("10.0.0.2:47820"), None);
    }

    #[test]
    fn is_this_host_matches_own_ip_and_loopback() {
        assert!(is_this_host("10.23.82.71:56128", Some("10.23.82.71")));
        assert!(is_this_host("127.0.0.1:47820", None));
        assert!(!is_this_host("10.23.82.90:47820", Some("10.23.82.71")));
        assert!(!is_this_host("10.23.82.71:47820", None));
    }

    #[test]
    fn pairing_code_is_symmetric_and_six_digits() {
        let a = pairing_code("aaa", "bbb");
        assert_eq!(a, pairing_code("bbb", "aaa"));
        assert_eq!(a.len(), 6);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(a, pairing_code("aaa", "ccc"));
    }

    #[test]
    fn identity_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("rustic-lan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = load_or_create_identity(&dir).unwrap();
        let b = load_or_create_identity(&dir).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.device_id, b.device_id);
        assert_eq!(a.fingerprint, fingerprint(&a.cert_der));
        assert!(server_tls_config(&a).is_ok());
        assert!(peer_client(&a.fingerprint, "127.0.0.1:1", &[]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peers_round_trip() {
        let dir = std::env::temp_dir().join(format!("rustic-lan-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Peer {
            device_id: "d1".into(),
            name: "Laptop".into(),
            fingerprint: "ff".into(),
            token_out: "o".into(),
            token_in: "i".into(),
            addr: None,
            nickname: None,
            share: Share::default(),
        };
        upsert_peer(&dir, p.clone()).unwrap();
        upsert_peer(
            &dir,
            Peer {
                name: "Laptop 2".into(),
                ..p
            },
        )
        .unwrap();
        let peers = load_peers(&dir);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Laptop 2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Serve one route over TLS with `id`'s cert, the same way the listener does.
    async fn serve_once(id: &Identity) -> u16 {
        let acceptor = tokio_rustls::TlsAcceptor::from(server_tls_config(id).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = axum::Router::new().route("/ping", axum::routing::get(|| async { "pong" }));
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                let router = router.clone();
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let io = hyper_util::rt::TokioIo::new(tls);
                    let svc = hyper_util::service::TowerToHyperService::new(router);
                    let _ = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, svc)
                    .await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn pinned_tls_accepts_paired_cert_and_rejects_others() {
        let dir = std::env::temp_dir().join(format!("rustic-lan-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = load_or_create_identity(&dir).unwrap();
        let port = serve_once(&id).await;

        let ok = peer_client(&id.fingerprint, "127.0.0.1:1", &[])
            .unwrap()
            .get(format!("https://127.0.0.1:{port}/ping"))
            .send()
            .await
            .expect("pinned client must connect");
        assert_eq!(ok.text().await.unwrap(), "pong");

        let wrong = peer_client(&"0".repeat(64), "127.0.0.1:1", &[])
            .unwrap()
            .get(format!("https://127.0.0.1:{port}/ping"))
            .send()
            .await;
        assert!(
            wrong.is_err(),
            "a different pinned fingerprint must be refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
