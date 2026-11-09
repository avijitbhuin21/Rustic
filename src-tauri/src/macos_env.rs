//! macOS apps launched from Finder/Dock inherit launchd's minimal PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), so node/npx/bun, formatters, MCP
//! servers and CLI agents installed via the user's shell profile are
//! invisible to every subprocess. Import the login shell's PATH once at
//! startup, before any threads exist (`set_var` is not thread-safe).

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const MARKER: &str = "__RUSTIC_PATH__";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Merge the user's login-shell PATH into this process's PATH (macOS GUI launches only).
pub fn import_login_shell_path() {
    // Launched from a terminal: PATH is already the user's.
    if std::env::var_os("TERM_PROGRAM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    let script = if shell.ends_with("fish") {
        format!("printf '{MARKER}%s{MARKER}' (string join : $PATH)")
    } else {
        format!("printf '{MARKER}%s{MARKER}' \"$PATH\"")
    };

    let Ok(mut child) = Command::new(&shell)
        .args(["-ilc", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };

    // Read on a helper thread so a chatty rc file can't fill the pipe and a
    // hung profile can't block startup past TIMEOUT.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let output = match rx.recv_timeout(TIMEOUT) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("[startup] login-shell PATH import timed out");
            return;
        }
    };
    let _ = child.wait();

    if let Some(login_path) = extract_marked(&output) {
        let current = std::env::var("PATH").unwrap_or_default();
        let merged = merge_paths(login_path, &current);
        if !merged.is_empty() {
            std::env::set_var("PATH", merged);
        }
    }
}

/// Point TMPDIR at the app bundle's volume when it differs from the system
/// temp volume, so the updater's rename-based install doesn't fail with
/// "Cross-device link (os error 18)" for apps on external drives
/// (tauri-apps/plugins-workspace#1983).
pub fn keep_temp_on_app_volume() {
    use std::os::unix::fs::MetadataExt;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // <Name>.app/Contents/MacOS/<binary>
    let Some(bundle) = exe.ancestors().nth(3) else {
        return;
    };
    if bundle.extension().is_none_or(|e| e != "app") {
        return;
    }
    let Some(parent) = bundle.parent() else {
        return;
    };
    let tmp = std::env::temp_dir();
    let (Ok(app_meta), Ok(tmp_meta)) = (std::fs::metadata(parent), std::fs::metadata(&tmp)) else {
        return;
    };
    if app_meta.dev() == tmp_meta.dev() {
        return;
    }
    let dir = parent.join(".rustic-tmp");
    let probe = dir.join(".probe");
    let writable = std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    if writable {
        std::env::set_var("TMPDIR", &dir);
    }
}

/// Pull the PATH value out from between the markers, ignoring any rc-file noise.
fn extract_marked(output: &str) -> Option<&str> {
    let start = output.find(MARKER)? + MARKER.len();
    let len = output[start..].find(MARKER)?;
    let value = output[start..start + len].trim();
    (!value.is_empty()).then_some(value)
}

/// Login-shell entries first, then any existing entries it lacks; duplicates dropped.
fn merge_paths(login: &str, current: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    login
        .split(':')
        .chain(current.split(':'))
        .filter(|p| !p.is_empty() && seen.insert(*p))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_value_between_markers_despite_noise() {
        let out = format!("welcome!\n{MARKER}/a:/b{MARKER}\nbye");
        assert_eq!(extract_marked(&out), Some("/a:/b"));
        assert_eq!(extract_marked("no markers"), None);
    }

    #[test]
    fn merge_keeps_login_order_and_dedupes() {
        assert_eq!(
            merge_paths("/opt/x:/usr/bin", "/usr/bin:/bin"),
            "/opt/x:/usr/bin:/bin"
        );
    }
}
