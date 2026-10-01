//! Cloudflare quick tunnel ("TryCloudflare") for the sync listener, so two
//! desktops can sync over the internet without port forwarding. Runs
//! `cloudflared tunnel --url https://127.0.0.1:<port> --no-tls-verify`; the
//! public side is Cloudflare's certificate, so peers reached through it rely
//! on the paired-device token rather than certificate pinning.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// A running quick tunnel.
pub struct Tunnel {
    pub url: String,
    child: Child,
}

impl Tunnel {
    /// Stop the cloudflared process.
    pub fn stop(mut self) {
        let _ = self.child.start_kill();
    }

    /// Whether cloudflared is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Start a quick tunnel to the local listener on `port` using the cloudflared
/// binary at `bin`; resolves once an edge connection is registered (so the
/// URL is actually live).
pub async fn start(port: u16, bin: &std::path::Path) -> Result<Tunnel, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("tunnel")
        .arg("--no-autoupdate")
        .arg("--protocol")
        .arg("http2")
        .arg("--no-tls-verify")
        .arg("--url")
        .arg(format!("https://127.0.0.1:{port}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start cloudflared ({}): {e}", bin.display()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "cloudflared: no stderr handle".to_string())?;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tx = Some(tx);
        let mut url: Option<String> = None;
        while let Ok(Some(line)) = lines.next_line().await {
            if url.is_none() {
                url = extract_url(&line);
            }
            if line.to_lowercase().contains("registered tunnel connection") {
                if let (Some(u), Some(tx)) = (url.clone(), tx.take()) {
                    let _ = tx.send(u);
                }
            }
        }
    });

    match tokio::time::timeout(Duration::from_secs(40), rx).await {
        Ok(Ok(url)) => Ok(Tunnel { url, child }),
        _ => {
            let _ = child.start_kill();
            Err("cloudflared did not establish a tunnel within 40s — check your internet connection or firewall.".into())
        }
    }
}

/// Official release asset for this platform, and whether it's a `.tgz`.
/// Source: Cloudflare's GitHub releases (linked from their downloads page).
fn release_asset() -> Option<(&'static str, bool)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(("cloudflared-windows-amd64.exe", false)),
        ("windows", "x86") => Some(("cloudflared-windows-386.exe", false)),
        ("linux", "x86_64") => Some(("cloudflared-linux-amd64", false)),
        ("linux", "aarch64") => Some(("cloudflared-linux-arm64", false)),
        ("macos", "x86_64") => Some(("cloudflared-darwin-amd64.tgz", true)),
        ("macos", "aarch64") => Some(("cloudflared-darwin-arm64.tgz", true)),
        _ => None,
    }
}

const RELEASE_BASE: &str = "https://github.com/cloudflare/cloudflared/releases/latest/download";

/// Where Rustic keeps its own copy of cloudflared.
pub fn managed_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    let name = if cfg!(windows) { "cloudflared.exe" } else { "cloudflared" };
    data_dir.join("bin").join(name)
}

/// Whether `bin` runs (`cloudflared --version` exits successfully).
async fn runs(bin: &std::path::Path) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(10), cmd.status()).await,
        Ok(Ok(s)) if s.success()
    )
}

/// An already-usable cloudflared: `CLOUDFLARED_BIN`, then PATH, then Rustic's copy.
pub async fn find_binary(data_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CLOUDFLARED_BIN") {
        let p = std::path::PathBuf::from(p);
        if runs(&p).await {
            return Some(p);
        }
    }
    let on_path = std::path::PathBuf::from("cloudflared");
    if runs(&on_path).await {
        return Some(on_path);
    }
    let managed = managed_path(data_dir);
    (managed.exists() && runs(&managed).await).then_some(managed)
}

/// cloudflared to use, downloading the official release into Rustic's data
/// dir when none is installed. `progress(done, total)` reports download bytes.
pub async fn ensure_binary(
    data_dir: &std::path::Path,
    progress: impl Fn(u64, u64),
) -> Result<std::path::PathBuf, String> {
    if let Some(p) = find_binary(data_dir).await {
        return Ok(p);
    }
    let (asset, tgz) = release_asset().ok_or_else(|| {
        format!(
            "No cloudflared build for {}/{} — install it manually or use port forwarding.",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let dest = managed_path(data_dir);
    let dir = dest.parent().ok_or("bad data dir")?.to_path_buf();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{asset}.part"));

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{RELEASE_BASE}/{asset}"))
        .send()
        .await
        .map_err(|e| format!("Downloading cloudflared failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Downloading cloudflared failed (HTTP {})", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();
        let mut done: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Downloading cloudflared failed: {e}"))?;
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }

    if tgz {
        let tmp_c = tmp.clone();
        let dest_c = dest.clone();
        tokio::task::spawn_blocking(move || extract_tgz_binary(&tmp_c, &dest_c))
            .await
            .map_err(|e| e.to_string())??;
        let _ = tokio::fs::remove_file(&tmp).await;
    } else {
        let _ = tokio::fs::remove_file(&dest).await;
        tokio::fs::rename(&tmp, &dest).await.map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    if !runs(&dest).await {
        return Err("Downloaded cloudflared but it won't run — your antivirus may have blocked it.".into());
    }
    Ok(dest)
}

/// Pull the `cloudflared` executable out of a macOS release `.tgz`.
fn extract_tgz_binary(tgz: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let f = std::fs::File::open(tgz).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let is_bin = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "cloudflared"))
            .unwrap_or(false);
        if is_bin {
            let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("cloudflared not found in the downloaded archive".into())
}

/// `https://<sub>.trycloudflare.com` from a cloudflared log line.
fn extract_url(line: &str) -> Option<String> {
    let idx = line.find("https://")?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches('/');
    url.contains("trycloudflare.com").then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_platform_has_a_release_asset() {
        if cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(release_asset(), Some(("cloudflared-windows-amd64.exe", false)));
        }
        let p = managed_path(std::path::Path::new("data"));
        assert!(p.ends_with(if cfg!(windows) { "bin/cloudflared.exe" } else { "bin/cloudflared" }));
    }

    #[test]
    fn extracts_trycloudflare_url() {
        let line = "2026-10-01 INF |  https://brave-fox-123.trycloudflare.com                  |";
        assert_eq!(extract_url(line).as_deref(), Some("https://brave-fox-123.trycloudflare.com"));
        assert_eq!(extract_url("INF Starting metrics server on 127.0.0.1:20241/metrics"), None);
    }
}
