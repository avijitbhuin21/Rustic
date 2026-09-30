//! Local-network sync (issue #15): discover other Rustic desktops on the same
//! network (mDNS), pair them with an Accept/Decline prompt plus a matching
//! 6-digit code, and sync over TLS pinned to each device's self-signed
//! certificate. The listener serves the same `/api/sync/*` routes as
//! rustic-server, so the existing sync client code is reused unchanged.

pub mod discovery;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// mDNS service type advertised by listening instances.
pub const SERVICE_TYPE: &str = "_rustic-sync._tcp.local.";

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
    pub pending_pairs: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
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

/// Human-readable name for this machine.
fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Rustic".to_string())
}

/// Load this install's identity, generating a certificate on first use.
pub fn load_or_create_identity(data_dir: &Path) -> Result<Identity, String> {
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
        device_name: machine_name(),
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

/// HTTP client that only talks to the device whose certificate is `expected_fp`.
pub fn pinned_client(expected_fp: &str) -> Result<reqwest::Client, String> {
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
    reqwest::Client::builder()
        .use_preconfigured_tls(cfg)
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(pinned_client(&a.fingerprint).is_ok());
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

        let ok = pinned_client(&id.fingerprint)
            .unwrap()
            .get(format!("https://127.0.0.1:{port}/ping"))
            .send()
            .await
            .expect("pinned client must connect");
        assert_eq!(ok.text().await.unwrap(), "pong");

        let wrong = pinned_client(&"0".repeat(64))
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
