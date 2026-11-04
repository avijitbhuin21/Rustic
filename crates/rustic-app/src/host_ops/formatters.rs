//! External formatter management, shared by the desktop and server hosts.
//!
//! Two storage layers:
//!   * Built-in registry — hard-coded here. Lists known formatters with
//!     metadata for downloading or detecting them.
//!   * Per-user state — persisted in SQLite under setting key `formatters`.
//!     Tracks installed versions of built-ins and any custom user entries.
//!
//! Binaries downloaded by the app live under `<app_data>/formatters/<id>/`
//! and never touch system PATH. Toolchain formatters (rustfmt, gofmt,
//! clang-format) are detected on PATH only.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::sync_ext::MutexExt;

// ─── Built-in registry ────────────────────────────────────────────────────────

/// How a formatter is acquired.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// App downloads a single binary from a GitHub release.
    Download,
    /// Part of a language toolchain; only detected on PATH.
    Toolchain,
}

/// One known formatter the UI exposes a row for.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinFormatter {
    pub id: &'static str,
    pub display_name: &'static str,
    pub languages: &'static [&'static str],
    pub description: &'static str,
    pub install_kind: InstallKind,
    /// `Toolchain`: binary looked up on PATH. `Download`: filename on disk.
    pub binary: &'static str,
    /// `{file}` / `{ext}` are substituted at invocation time.
    pub args: &'static [&'static str],
    pub stdin: bool,
    /// GitHub `owner/repo` for downloads and update checks.
    pub github: Option<&'static str>,
    pub install_url: Option<&'static str>,
}

pub const REGISTRY: &[BuiltinFormatter] = &[
    BuiltinFormatter {
        id: "ruff",
        display_name: "Ruff",
        languages: &["python"],
        description: "Extremely fast Python formatter (and linter). Single binary, no Python required.",
        install_kind: InstallKind::Download,
        binary: "ruff",
        args: &["format", "-"],
        stdin: true,
        github: Some("astral-sh/ruff"),
        install_url: None,
    },
    BuiltinFormatter {
        id: "shfmt",
        display_name: "shfmt",
        languages: &["shell", "bash", "sh", "zsh"],
        description: "Formatter for shell scripts (bash, sh, mksh).",
        install_kind: InstallKind::Download,
        binary: "shfmt",
        args: &["-"],
        stdin: true,
        github: Some("mvdan/sh"),
        install_url: None,
    },
    BuiltinFormatter {
        id: "rustfmt",
        display_name: "rustfmt",
        languages: &["rust"],
        description: "Official Rust formatter. Ships with rustup; install via `rustup component add rustfmt`.",
        install_kind: InstallKind::Toolchain,
        binary: "rustfmt",
        args: &["--emit", "stdout"],
        stdin: true,
        github: None,
        install_url: Some("https://rustup.rs"),
    },
    BuiltinFormatter {
        id: "gofmt",
        display_name: "gofmt",
        languages: &["go"],
        description: "Official Go formatter. Bundled with the Go toolchain.",
        install_kind: InstallKind::Toolchain,
        binary: "gofmt",
        args: &[],
        stdin: true,
        github: None,
        install_url: Some("https://go.dev/dl/"),
    },
    BuiltinFormatter {
        id: "clang-format",
        display_name: "clang-format",
        languages: &["c", "cpp", "c++", "objective-c", "objective-cpp"],
        description: "Formatter for C, C++, and Objective-C from the LLVM project.",
        install_kind: InstallKind::Toolchain,
        binary: "clang-format",
        args: &["--assume-filename={file}"],
        stdin: true,
        github: None,
        install_url: Some("https://releases.llvm.org/"),
    },
    // Prettier is bundled into the renderer (src/lib/prettier-worker.js);
    // don't list it here or the modal would offer to "install" it.
];

/// Trimmed shape sent to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinDescriptor {
    pub id: String,
    pub display_name: String,
    pub languages: Vec<String>,
    pub description: String,
    pub install_kind: InstallKind,
    pub binary: String,
    pub stdin: bool,
    pub install_url: Option<String>,
}

impl From<&BuiltinFormatter> for BuiltinDescriptor {
    fn from(b: &BuiltinFormatter) -> Self {
        Self {
            id: b.id.to_string(),
            display_name: b.display_name.to_string(),
            languages: b.languages.iter().map(|s| s.to_string()).collect(),
            description: b.description.to_string(),
            install_kind: b.install_kind,
            binary: platform_binary_name(b.binary),
            stdin: b.stdin,
            install_url: b.install_url.map(|s| s.to_string()),
        }
    }
}

pub fn registry() -> Vec<BuiltinDescriptor> {
    REGISTRY.iter().map(BuiltinDescriptor::from).collect()
}

fn lookup(id: &str) -> Option<&'static BuiltinFormatter> {
    REGISTRY.iter().find(|b| b.id == id)
}

/// Filename on disk — appends `.exe` on Windows.
fn platform_binary_name(stem: &str) -> String {
    if cfg!(target_os = "windows") && !stem.ends_with(".exe") {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

// ─── Persistent state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FormattersState {
    pub installed: HashMap<String, InstalledRecord>,
    pub custom: Vec<CustomFormatter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledRecord {
    pub version: String,
    pub path: String,
    pub installed_at: String, // ISO8601
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFormatter {
    pub id: String,
    pub display_name: String,
    pub languages: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default = "default_true")]
    pub stdin: bool,
    #[serde(default)]
    pub description: String,
}

fn default_true() -> bool {
    true
}

fn load_state(state: &AppState) -> Result<FormattersState, String> {
    let db = state.db.lock_safe();
    match db.get_setting("formatters").map_err(|e| e.to_string())? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(FormattersState::default()),
    }
}

fn save_state(state: &AppState, s: &FormattersState) -> Result<(), String> {
    let db = state.db.lock_safe();
    let json = serde_json::to_string(s).map_err(|e| e.to_string())?;
    db.set_setting("formatters", &json).map_err(|e| e.to_string())
}

// ─── Public command shapes ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct FormatterStatus {
    pub id: String,
    pub kind: StatusKind,
    pub resolved_path: Option<String>,
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Installed,
    Detected,
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatterListEntry {
    pub builtin: Option<BuiltinDescriptor>,
    pub custom: Option<CustomFormatter>,
    pub status: FormatterStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatRequest {
    pub language: String,
    pub source: String,
    /// Used for `{file}` substitution and extension-derived language.
    #[serde(default, alias = "file_path")]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatResponse {
    pub formatted: String,
    pub formatter_id: String,
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

fn formatters_dir(data_dir: &Path) -> Result<PathBuf, String> {
    let dir = data_dir.join("formatters");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn formatter_install_path(data_dir: &Path, id: &str, binary: &str) -> Result<PathBuf, String> {
    let dir = formatters_dir(data_dir)?.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(binary))
}

fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|entry| entry.join(binary))
        .find(|candidate| candidate.is_file())
}

fn status(id: &str, kind: StatusKind, path: Option<String>, version: Option<String>) -> FormatterStatus {
    FormatterStatus {
        id: id.to_string(),
        kind,
        resolved_path: path,
        installed_version: version,
    }
}

// ─── Operations ───────────────────────────────────────────────────────────────

/// `data_dir` is the host's app data directory (profile-scoped on desktop).
pub fn list(state: &AppState, data_dir: &Path) -> Result<Vec<FormatterListEntry>, String> {
    let persisted = load_state(state)?;
    let mut entries = Vec::new();

    for b in REGISTRY {
        let bin = platform_binary_name(b.binary);
        let st = if let Some(rec) = persisted.installed.get(b.id) {
            if Path::new(&rec.path).is_file() {
                status(b.id, StatusKind::Installed, Some(rec.path.clone()), Some(rec.version.clone()))
            } else {
                status(b.id, StatusKind::Missing, None, None)
            }
        } else if matches!(b.install_kind, InstallKind::Toolchain) {
            match which(&bin) {
                Some(p) => status(b.id, StatusKind::Detected, Some(p.to_string_lossy().to_string()), None),
                None => status(b.id, StatusKind::Missing, None, None),
            }
        } else {
            // Not recorded, but a previous session may have left the binary.
            let candidate = formatter_install_path(data_dir, b.id, &bin).ok();
            match candidate.filter(|p| p.is_file()) {
                Some(p) => status(b.id, StatusKind::Installed, Some(p.to_string_lossy().to_string()), None),
                None => status(b.id, StatusKind::Missing, None, None),
            }
        };
        entries.push(FormatterListEntry {
            builtin: Some(BuiltinDescriptor::from(b)),
            custom: None,
            status: st,
        });
    }

    for c in &persisted.custom {
        let resolved = if Path::new(&c.command).is_absolute() {
            Path::new(&c.command).is_file().then(|| c.command.clone())
        } else {
            which(&c.command).map(|p| p.to_string_lossy().to_string())
        };
        let kind = if resolved.is_some() { StatusKind::Detected } else { StatusKind::Missing };
        entries.push(FormatterListEntry {
            builtin: None,
            custom: Some(c.clone()),
            status: status(&c.id, kind, resolved, None),
        });
    }

    Ok(entries)
}

/// Install (or update — same thing: always fetches the latest release).
pub async fn install(state: &AppState, data_dir: &Path, id: &str) -> Result<InstalledRecord, String> {
    let b = lookup(id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    if !matches!(b.install_kind, InstallKind::Download) {
        return Err(format!(
            "'{id}' is a toolchain formatter and cannot be installed by Rustic. Install it externally."
        ));
    }
    let github = b.github.ok_or("No GitHub repo configured")?;
    let release = fetch_latest_release(github).await?;
    download_and_extract(data_dir, b, &release).await?;

    let path = formatter_install_path(data_dir, b.id, &platform_binary_name(b.binary))?;
    let rec = InstalledRecord {
        version: release.tag_name.clone(),
        path: path.to_string_lossy().to_string(),
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut st = load_state(state)?;
    st.installed.insert(b.id.to_string(), rec.clone());
    save_state(state, &st)?;
    Ok(rec)
}

pub async fn check_update(id: &str) -> Result<UpdateInfo, String> {
    let b = lookup(id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    let github = b.github.ok_or("No GitHub repo configured for update checks")?;
    let release = fetch_latest_release(github).await?;
    Ok(UpdateInfo {
        latest_version: release.tag_name,
        published_at: release.published_at,
    })
}

pub fn uninstall(state: &AppState, data_dir: &Path, id: &str) -> Result<(), String> {
    lookup(id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    let dir = formatters_dir(data_dir)?.join(id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let mut st = load_state(state)?;
    st.installed.remove(id);
    save_state(state, &st)
}

pub fn add_custom(state: &AppState, formatter: CustomFormatter) -> Result<(), String> {
    let mut st = load_state(state)?;
    validate_new_custom(&st, &formatter)?;
    st.custom.push(formatter);
    save_state(state, &st)
}

fn validate_new_custom(st: &FormattersState, formatter: &CustomFormatter) -> Result<(), String> {
    if st.custom.iter().any(|c| c.id == formatter.id) {
        return Err(format!("A custom formatter with id '{}' already exists", formatter.id));
    }
    if lookup(&formatter.id).is_some() {
        return Err(format!("'{}' is reserved by a built-in formatter", formatter.id));
    }
    Ok(())
}

pub fn update_custom(state: &AppState, formatter: CustomFormatter) -> Result<(), String> {
    let mut st = load_state(state)?;
    match st.custom.iter_mut().find(|c| c.id == formatter.id) {
        Some(s) => *s = formatter,
        None => return Err(format!("No custom formatter with id '{}'", formatter.id)),
    }
    save_state(state, &st)
}

pub fn remove_custom(state: &AppState, id: &str) -> Result<(), String> {
    let mut st = load_state(state)?;
    st.custom.retain(|c| c.id != id);
    save_state(state, &st)
}

// ─── Format invocation ────────────────────────────────────────────────────────

enum ResolvedFormatter {
    Builtin(&'static BuiltinFormatter),
    Custom(CustomFormatter),
}

/// Custom formatters claiming the language win over built-ins.
fn resolve_formatter(state: &FormattersState, language: &str) -> Option<ResolvedFormatter> {
    if let Some(c) = state
        .custom
        .iter()
        .find(|c| c.languages.iter().any(|l| l.eq_ignore_ascii_case(language)))
    {
        return Some(ResolvedFormatter::Custom(c.clone()));
    }
    REGISTRY
        .iter()
        .find(|b| b.languages.iter().any(|l| l.eq_ignore_ascii_case(language)))
        .map(ResolvedFormatter::Builtin)
}

fn substitute_args(raw_args: &[String], file_path: &str) -> Vec<String> {
    let ext = Path::new(file_path).extension().and_then(|s| s.to_str()).unwrap_or("");
    raw_args
        .iter()
        .map(|a| a.replace("{file}", file_path).replace("{ext}", ext))
        .collect()
}

pub async fn format(state: &AppState, data_dir: &Path, req: FormatRequest) -> Result<FormatResponse, String> {
    let persisted = load_state(state)?;
    let resolved = resolve_formatter(&persisted, &req.language)
        .ok_or_else(|| format!("No formatter configured for language '{}'", req.language))?;

    let (id, command, raw_args, use_stdin) = match resolved {
        ResolvedFormatter::Builtin(b) => {
            let bin = platform_binary_name(b.binary);
            let resolved_path = match b.install_kind {
                InstallKind::Download => match persisted.installed.get(b.id) {
                    Some(r) if Path::new(&r.path).is_file() => PathBuf::from(&r.path),
                    _ => {
                        let p = formatter_install_path(data_dir, b.id, &bin)?;
                        if !p.is_file() {
                            return Err(format!(
                                "'{}' is not installed. Open Formatters and install it first.",
                                b.display_name
                            ));
                        }
                        p
                    }
                },
                InstallKind::Toolchain => which(&bin).ok_or_else(|| {
                    format!(
                        "'{}' not found on PATH. Install it or open Formatters for instructions.",
                        b.display_name
                    )
                })?,
            };
            (
                b.id.to_string(),
                resolved_path,
                b.args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                b.stdin,
            )
        }
        ResolvedFormatter::Custom(c) => {
            let cmd_path = if Path::new(&c.command).is_absolute() {
                PathBuf::from(&c.command)
            } else {
                which(&c.command).ok_or_else(|| format!("'{}' not found on PATH", c.command))?
            };
            (c.id.clone(), cmd_path, c.args.clone(), c.stdin)
        }
    };

    let file_path = req.file_path.unwrap_or_else(|| "stdin".to_string());
    let args = substitute_args(&raw_args, &file_path);
    let formatted = run_formatter(command, args, req.source, use_stdin).await?;
    Ok(FormatResponse {
        formatted,
        formatter_id: id,
    })
}

async fn run_formatter(
    cmd_path: PathBuf,
    args: Vec<String>,
    source: String,
    use_stdin: bool,
) -> Result<String, String> {
    // Command IO is blocking; keep it off the async runtime.
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(&cmd_path);
        cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if use_stdin {
            cmd.stdin(Stdio::piped());
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW — no console flash when spawning console tools.
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        if use_stdin {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(source.as_bytes()).map_err(|e| e.to_string())?;
            }
            // Send EOF — gofmt/ruff block forever otherwise.
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        String::from_utf8(output.stdout).map_err(|e| format!("formatter produced invalid UTF-8: {e}"))
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

// ─── GitHub release fetch + binary extraction ─────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("rustic-editor")
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch_latest_release(repo: &str) -> Result<GithubRelease, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = http_client()?.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }
    resp.json::<GithubRelease>().await.map_err(|e| e.to_string())
}

/// (asset-name substrings, is_zip) identifying the asset for `os`/`arch`.
/// All substrings must be present, case-insensitively.
fn asset_pattern_for(
    formatter_id: &str,
    os: &'static str,
    arch: &'static str,
) -> Result<(Vec<&'static str>, bool), String> {
    Ok(match formatter_id {
        // ruff: ruff-x86_64-pc-windows-msvc.zip / ruff-aarch64-apple-darwin.tar.gz
        "ruff" => {
            let tokens: Vec<&'static str> = match (os, arch) {
                ("windows", "x86_64") => vec!["ruff-", "x86_64", "windows", ".zip"],
                ("windows", "aarch64") => vec!["ruff-", "aarch64", "windows", ".zip"],
                ("darwin", "x86_64") => vec!["ruff-", "x86_64", "apple", ".tar.gz"],
                ("darwin", "aarch64") => vec!["ruff-", "aarch64", "apple", ".tar.gz"],
                ("linux", "x86_64") => vec!["ruff-", "x86_64", "linux", ".tar.gz"],
                ("linux", "aarch64") => vec!["ruff-", "aarch64", "linux", ".tar.gz"],
                _ => return Err("unsupported platform".into()),
            };
            let is_zip = tokens.last().is_some_and(|t| *t == ".zip");
            (tokens, is_zip)
        }
        // shfmt publishes plain binaries: shfmt_v3.8.0_windows_amd64.exe etc.
        "shfmt" => {
            let arch_word = if arch == "x86_64" { "amd64" } else { "arm64" };
            (vec!["shfmt_", os, arch_word], false)
        }
        _ => return Err(format!("no asset pattern for '{formatter_id}'")),
    })
}

fn platform_asset_pattern(formatter_id: &str) -> Result<(Vec<&'static str>, bool), String> {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return Err("unsupported OS".into());
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return Err("unsupported arch".into());
    };
    asset_pattern_for(formatter_id, os, arch)
}

async fn download_and_extract(
    data_dir: &Path,
    b: &BuiltinFormatter,
    release: &GithubRelease,
) -> Result<(), String> {
    let (tokens, is_archive) = platform_asset_pattern(b.id)?;
    let asset = release
        .assets
        .iter()
        .find(|a| {
            let name_lower = a.name.to_ascii_lowercase();
            tokens.iter().all(|t| name_lower.contains(&t.to_ascii_lowercase()))
        })
        .ok_or_else(|| format!("no matching asset in release {} for tokens {:?}", release.tag_name, tokens))?;

    let bytes = http_client()?
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let dir = formatters_dir(data_dir)?.join(b.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let binary_name = platform_binary_name(b.binary);
    let dest = dir.join(&binary_name);
    let target_lower = binary_name.to_ascii_lowercase();

    if is_archive {
        // ruff zips contain a top-level dir with the binary inside.
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_ref())).map_err(|e| e.to_string())?;
        let mut wrote = false;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
            if file.name().to_ascii_lowercase().ends_with(&target_lower) && !file.is_dir() {
                let mut buf = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                std::fs::File::create(&dest)
                    .and_then(|mut out| out.write_all(&buf))
                    .map_err(|e| e.to_string())?;
                wrote = true;
                break;
            }
        }
        if !wrote {
            return Err(format!("binary '{binary_name}' not found in archive"));
        }
    } else if asset.name.ends_with(".tar.gz") || asset.name.ends_with(".tgz") {
        // ruff tarballs contain `ruff-<triple>/ruff`.
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes.as_ref()));
        let mut tarball = tar::Archive::new(gz);
        let mut wrote = false;
        for entry in tarball.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case(&target_lower) && entry.header().entry_type().is_file() {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                std::fs::write(&dest, &buf).map_err(|e| e.to_string())?;
                wrote = true;
                break;
            }
        }
        if !wrote {
            return Err(format!("binary '{binary_name}' not found in tarball"));
        }
    } else {
        // Plain binary (shfmt).
        std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: &str, langs: &[&str]) -> CustomFormatter {
        CustomFormatter {
            id: id.into(),
            display_name: id.into(),
            languages: langs.iter().map(|s| s.to_string()).collect(),
            command: "x".into(),
            args: vec![],
            stdin: true,
            description: String::new(),
        }
    }

    #[test]
    fn resolve_prefers_custom_then_builtin_case_insensitive() {
        let mut st = FormattersState::default();
        assert!(matches!(resolve_formatter(&st, "Rust"), Some(ResolvedFormatter::Builtin(b)) if b.id == "rustfmt"));
        assert!(matches!(resolve_formatter(&st, "BASH"), Some(ResolvedFormatter::Builtin(b)) if b.id == "shfmt"));
        assert!(resolve_formatter(&st, "cobol").is_none());
        st.custom.push(custom("mine", &["rust"]));
        assert!(matches!(resolve_formatter(&st, "rust"), Some(ResolvedFormatter::Custom(c)) if c.id == "mine"));
    }

    #[test]
    fn custom_ids_cannot_clash() {
        let mut st = FormattersState::default();
        assert!(validate_new_custom(&st, &custom("ruff", &[])).unwrap_err().contains("reserved"));
        st.custom.push(custom("mine", &[]));
        assert!(validate_new_custom(&st, &custom("mine", &[])).unwrap_err().contains("already exists"));
        assert!(validate_new_custom(&st, &custom("other", &[])).is_ok());
    }

    #[test]
    fn args_substitute_file_and_ext() {
        let raw = vec!["--assume-filename={file}".to_string(), "-x={ext}".to_string()];
        assert_eq!(substitute_args(&raw, "src/a.cpp"), vec!["--assume-filename=src/a.cpp", "-x=cpp"]);
        assert_eq!(substitute_args(&raw, "stdin"), vec!["--assume-filename=stdin", "-x="]);
    }

    #[test]
    fn asset_patterns_cover_all_desktop_platforms() {
        for os in ["windows", "darwin", "linux"] {
            for arch in ["x86_64", "aarch64"] {
                let (tokens, is_zip) = asset_pattern_for("ruff", os, arch).unwrap();
                assert_eq!(is_zip, os == "windows");
                assert!(tokens.contains(&arch));
                assert!(asset_pattern_for("shfmt", os, arch).is_ok());
            }
        }
        assert!(asset_pattern_for("rustfmt", "linux", "x86_64").is_err());
    }

    #[test]
    fn descriptor_exposes_platform_binary() {
        let reg = registry();
        assert_eq!(reg.len(), REGISTRY.len());
        let ruff = reg.iter().find(|d| d.id == "ruff").unwrap();
        assert_eq!(ruff.binary, platform_binary_name("ruff"));
    }
}
