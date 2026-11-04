//! Default shell for agent-spawned terminals, shared by both hosts and the
//! system prompt so the advertised shell always matches the one that runs.
//!
//! macOS / Linux: zsh → bash → sh. Windows: Git Bash → pwsh → Windows PowerShell.
//! User terminals and external CLI agents do NOT use this (they keep the
//! user's login shell / PowerShell).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Env applied to agent shells: silence macOS's bash deprecation banner, keep
/// oh-my-zsh from blocking on its update / insecure-dir prompts, and keep Git
/// Bash's login profile from `cd`-ing to $HOME.
pub const AGENT_SHELL_ENV: &[(&str, &str)] = &[
    ("BASH_SILENCE_DEPRECATION_WARNING", "1"),
    ("DISABLE_AUTO_UPDATE", "true"),
    ("DISABLE_UPDATE_PROMPT", "true"),
    ("ZSH_DISABLE_COMPFIX", "true"),
    ("CHERE_INVOKING", "1"),
];

struct Choice {
    path: Option<String>,
    label: &'static str,
}

static CHOICE: OnceLock<Choice> = OnceLock::new();

/// Absolute path of the shell agent terminals should spawn (`None` = pty default).
pub fn agent_shell_path() -> Option<String> {
    choice().path.clone()
}

/// Human label for the system prompt's `shell=` field, e.g. "zsh on macOS".
pub fn agent_shell_label() -> &'static str {
    choice().label
}

/// Shell names in the order agents should prefer them (first = default).
pub fn preferred_shell_order() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["bash", "pwsh", "powershell", "cmd", "zsh", "sh", "fish"]
    } else {
        &["zsh", "bash", "sh", "fish"]
    }
}

fn choice() -> &'static Choice {
    CHOICE.get_or_init(detect)
}

#[cfg(not(target_os = "windows"))]
fn detect() -> Choice {
    let os = if cfg!(target_os = "macos") { "macOS" } else { "Linux" };
    for (name, fallbacks) in [
        ("zsh", &["/bin/zsh", "/usr/bin/zsh"][..]),
        ("bash", &["/bin/bash", "/usr/bin/bash"][..]),
        ("sh", &["/bin/sh"][..]),
    ] {
        let found = find_in_path(name).or_else(|| {
            fallbacks
                .iter()
                .find(|p| Path::new(p).is_file())
                .map(|p| p.to_string())
        });
        if let Some(path) = found {
            return Choice { path: Some(path), label: unix_label(name, os) };
        }
    }
    Choice { path: None, label: unix_label("sh", os) }
}

#[cfg(not(target_os = "windows"))]
fn unix_label(name: &str, os: &str) -> &'static str {
    match (name, os) {
        ("zsh", "macOS") => "zsh on macOS",
        ("zsh", _) => "zsh on Linux",
        ("bash", "macOS") => "bash on macOS",
        ("bash", _) => "bash on Linux",
        (_, "macOS") => "sh on macOS",
        _ => "sh on Linux",
    }
}

#[cfg(target_os = "windows")]
fn detect() -> Choice {
    if let Some(path) = git_bash_path() {
        return Choice { path: Some(path), label: "bash (Git Bash) on Windows" };
    }
    if let Some(path) = find_in_path("pwsh.exe") {
        return Choice { path: Some(path), label: "PowerShell on Windows" };
    }
    let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    if Path::new(legacy).exists() {
        return Choice { path: Some(legacy.to_string()), label: "PowerShell on Windows" };
    }
    Choice { path: None, label: "cmd on Windows" }
}

/// Locates Git for Windows' bash. Never returns WSL's `System32\bash.exe` or the
/// `WindowsApps` alias, which start a Linux VM instead of Git Bash.
pub fn git_bash_path() -> Option<String> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for var in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(var) {
            candidates.push(Path::new(&base).join("Git").join("bin").join("bash.exe"));
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(Path::new(&local).join("Programs").join("Git").join("bin").join("bash.exe"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        candidates.push(
            Path::new(&home).join("scoop").join("apps").join("git").join("current").join("bin").join("bash.exe"),
        );
    }
    // git.exe on PATH lives in <Git>\cmd, <Git>\bin or <Git>\mingw64\bin.
    if let Some(git) = find_in_path("git.exe") {
        let mut dir = Path::new(&git).parent().map(Path::to_path_buf);
        for _ in 0..3 {
            let Some(d) = dir else { break };
            candidates.push(d.join("bin").join("bash.exe"));
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    if let Some(p) = find_in_path("bash.exe") {
        candidates.push(PathBuf::from(p));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file() && !is_wsl_launcher(p))
        .map(|p| p.to_string_lossy().to_string())
}

fn is_wsl_launcher(p: &Path) -> bool {
    let low = p.to_string_lossy().to_ascii_lowercase();
    low.contains("\\system32\\") || low.contains("\\windowsapps\\")
}

fn find_in_path(exe: &str) -> Option<String> {
    let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
    let path_var = std::env::var("PATH").ok()?;
    path_var
        .split(sep)
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(exe))
        .find(|c| c.is_file())
        .map(|c| c.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_matches_platform_and_path() {
        let label = agent_shell_label();
        if cfg!(target_os = "macos") {
            assert!(label.ends_with("on macOS"), "{label}");
        } else if cfg!(target_os = "windows") {
            assert!(label.ends_with("on Windows"), "{label}");
        } else {
            assert!(label.ends_with("on Linux"), "{label}");
        }
        if let Some(path) = agent_shell_path() {
            let stem = Path::new(&path).file_stem().unwrap().to_string_lossy().to_lowercase();
            assert!(label.starts_with(stem.trim_end_matches(".exe")) || label.starts_with("PowerShell"), "{label} vs {path}");
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_prefers_zsh_when_present() {
        let zsh_present = find_in_path("zsh").is_some() || Path::new("/bin/zsh").is_file();
        if zsh_present {
            assert!(agent_shell_label().starts_with("zsh"));
            assert_eq!(preferred_shell_order()[0], "zsh");
        }
    }

    #[test]
    fn wsl_launchers_are_rejected() {
        assert!(is_wsl_launcher(Path::new(r"C:\Windows\System32\bash.exe")));
        assert!(is_wsl_launcher(Path::new(r"C:\Users\a\AppData\Local\Microsoft\WindowsApps\bash.exe")));
        assert!(!is_wsl_launcher(Path::new(r"C:\Program Files\Git\bin\bash.exe")));
    }
}
