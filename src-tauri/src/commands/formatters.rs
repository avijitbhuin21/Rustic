//! External formatter management.
//!
//! Two storage layers:
//!   * Built-in registry — hard-coded in this file. Lists known formatters with
//!     metadata for downloading or detecting them.
//!   * Per-user state — persisted in SQLite under setting key `formatters`.
//!     Tracks installed versions of built-ins and any custom user entries.
//!
//! Binaries downloaded by the app live under `<app_data>/formatters/<id>/`
//! and never touch system PATH. Toolchain formatters (rustfmt, gofmt,
//! clang-format) are detected on PATH only — the app does not try to install
//! a toolchain on the user's behalf.

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{AppHandle, State};

// ─── Built-in registry ────────────────────────────────────────────────────────

/// How a formatter is acquired.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// App downloads a single binary from a GitHub release into its private
    /// formatters directory.
    Download,
    /// Formatter is part of a language toolchain. App only detects whether the
    /// binary is on PATH; install is the user's responsibility.
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
    /// For `Toolchain`: name of the binary to look up on PATH.
    /// For `Download`: filename written to the formatters directory.
    pub binary: &'static str,
    /// Args passed when formatting. `{file}` is substituted with the file path
    /// (or a synthetic name for unsaved buffers); `{ext}` with the extension.
    pub args: &'static [&'static str],
    /// Whether to pipe source via stdin. If false, the formatter is expected to
    /// modify the file in place (rare; most stream via stdin/stdout).
    pub stdin: bool,
    /// GitHub `owner/repo` used both for resolving release downloads and for
    /// "Check for updates". `None` for toolchain formatters.
    pub github: Option<&'static str>,
    /// URL the UI opens for toolchain formatters when the binary isn't found.
    pub install_url: Option<&'static str>,
}

const REGISTRY: &[BuiltinFormatter] = &[
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
    // Prettier is bundled into the renderer via prettier/standalone — see
    // src/lib/prettier-worker.js. Don't list it here as a toolchain formatter
    // or the modal would offer to "install" a PATH binary we'd never call.
];

#[tauri::command]
pub fn formatter_registry() -> Vec<BuiltinDescriptor> {
    REGISTRY.iter().map(BuiltinDescriptor::from).collect()
}

/// Trimmed shape sent to the frontend (replaces &'static strings with owned).
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

fn lookup(id: &str) -> Option<&'static BuiltinFormatter> {
    REGISTRY.iter().find(|b| b.id == id)
}

/// Returns the actual filename on disk — appends `.exe` on Windows when the
/// builtin's `binary` field is plain (handles the platform branching that the
/// const `bin_name` can't do).
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
    /// Built-ins installed by the app. Key = formatter id from REGISTRY.
    pub installed: HashMap<String, InstalledRecord>,
    /// User-defined formatters.
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

fn default_true() -> bool { true }

fn load_state(state: &State<'_, AppState>) -> Result<FormattersState, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    match db.get_setting("formatters").map_err(|e| e.to_string())? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(FormattersState::default()),
    }
}

fn save_state(state: &State<'_, AppState>, s: &FormattersState) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(s).map_err(|e| e.to_string())?;
    db.set_setting("formatters", &json).map_err(|e| e.to_string())
}

// ─── Public command shapes ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct FormatterStatus {
    pub id: String,
    pub kind: StatusKind,
    /// Resolved binary path or PATH-found location. None when not installed.
    pub resolved_path: Option<String>,
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    /// App-managed install present and runnable.
    Installed,
    /// Toolchain binary found on PATH.
    Detected,
    /// Built-in known but not installed/detected.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatterListEntry {
    pub builtin: Option<BuiltinDescriptor>,
    pub custom: Option<CustomFormatter>,
    pub status: FormatterStatus,
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

fn formatters_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())?;
    let dir = base.join("formatters");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn formatter_install_path(app: &AppHandle, id: &str, binary: &str) -> Result<PathBuf, String> {
    let dir = formatters_dir(app)?.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(binary))
}

fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path_var) {
        let candidate = entry.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ─── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn formatter_list(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<FormatterListEntry>, String> {
    let persisted = load_state(&state)?;
    let mut entries = Vec::new();

    for b in REGISTRY {
        let desc = BuiltinDescriptor::from(b);
        let bin = platform_binary_name(b.binary);

        let status = if let Some(rec) = persisted.installed.get(b.id) {
            // Verify the path still exists; if not, treat as missing.
            if Path::new(&rec.path).is_file() {
                FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Installed,
                    resolved_path: Some(rec.path.clone()),
                    installed_version: Some(rec.version.clone()),
                }
            } else {
                FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Missing,
                    resolved_path: None,
                    installed_version: None,
                }
            }
        } else if matches!(b.install_kind, InstallKind::Toolchain) {
            match which(&bin) {
                Some(p) => FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Detected,
                    resolved_path: Some(p.to_string_lossy().to_string()),
                    installed_version: None,
                },
                None => FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Missing,
                    resolved_path: None,
                    installed_version: None,
                },
            }
        } else {
            // Downloadable but not installed — also check if app-private path exists
            // in case a previous session left it without a state record.
            let candidate = formatter_install_path(&app, b.id, &bin).ok();
            match candidate.filter(|p| p.is_file()) {
                Some(p) => FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Installed,
                    resolved_path: Some(p.to_string_lossy().to_string()),
                    installed_version: None,
                },
                None => FormatterStatus {
                    id: b.id.to_string(),
                    kind: StatusKind::Missing,
                    resolved_path: None,
                    installed_version: None,
                },
            }
        };

        entries.push(FormatterListEntry {
            builtin: Some(desc),
            custom: None,
            status,
        });
    }

    for c in &persisted.custom {
        // Custom formatters resolve through PATH or absolute path verbatim.
        let resolved = if Path::new(&c.command).is_absolute() {
            Path::new(&c.command).is_file().then(|| c.command.clone())
        } else {
            which(&c.command).map(|p| p.to_string_lossy().to_string())
        };
        let kind = if resolved.is_some() { StatusKind::Detected } else { StatusKind::Missing };
        let status = FormatterStatus {
            id: c.id.clone(),
            kind,
            resolved_path: resolved,
            installed_version: None,
        };
        entries.push(FormatterListEntry {
            builtin: None,
            custom: Some(c.clone()),
            status,
        });
    }

    Ok(entries)
}

#[tauri::command]
pub async fn formatter_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<InstalledRecord, String> {
    let b = lookup(&id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    if !matches!(b.install_kind, InstallKind::Download) {
        return Err(format!("'{id}' is a toolchain formatter and cannot be installed by Rustic. Install it externally."));
    }
    let github = b.github.ok_or("No GitHub repo configured")?;
    let release = fetch_latest_release(github).await?;
    download_and_extract(&app, b, &release).await?;

    let bin = platform_binary_name(b.binary);
    let path = formatter_install_path(&app, b.id, &bin)?;
    let rec = InstalledRecord {
        version: release.tag_name.clone(),
        path: path.to_string_lossy().to_string(),
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut st = load_state(&state)?;
    st.installed.insert(b.id.to_string(), rec.clone());
    save_state(&state, &st)?;
    Ok(rec)
}

#[tauri::command]
pub async fn formatter_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<InstalledRecord, String> {
    // Same as install — fetches whatever the latest release tag is and
    // overwrites. We deliberately don't cache the "latest" check; each call
    // hits GitHub so the version shown to the user is always fresh.
    formatter_install(app, state, id).await
}

#[tauri::command]
pub async fn formatter_check_update(id: String) -> Result<UpdateInfo, String> {
    let b = lookup(&id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    let github = b.github.ok_or("No GitHub repo configured for update checks")?;
    let release = fetch_latest_release(github).await?;
    Ok(UpdateInfo {
        latest_version: release.tag_name,
        published_at: release.published_at,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub published_at: Option<String>,
}

#[tauri::command]
pub fn formatter_uninstall(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let _ = lookup(&id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    let dir = formatters_dir(&app)?.join(&id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let mut st = load_state(&state)?;
    st.installed.remove(&id);
    save_state(&state, &st)?;
    Ok(())
}

#[tauri::command]
pub fn formatter_add_custom(
    state: State<'_, AppState>,
    formatter: CustomFormatter,
) -> Result<(), String> {
    let mut st = load_state(&state)?;
    if st.custom.iter().any(|c| c.id == formatter.id) {
        return Err(format!("A custom formatter with id '{}' already exists", formatter.id));
    }
    if lookup(&formatter.id).is_some() {
        return Err(format!("'{}' is reserved by a built-in formatter", formatter.id));
    }
    st.custom.push(formatter);
    save_state(&state, &st)
}

#[tauri::command]
pub fn formatter_update_custom(
    state: State<'_, AppState>,
    formatter: CustomFormatter,
) -> Result<(), String> {
    let mut st = load_state(&state)?;
    let slot = st.custom.iter_mut().find(|c| c.id == formatter.id);
    match slot {
        Some(s) => { *s = formatter; }
        None => return Err(format!("No custom formatter with id '{}'", formatter.id)),
    }
    save_state(&state, &st)
}

#[tauri::command]
pub fn formatter_remove_custom(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let mut st = load_state(&state)?;
    st.custom.retain(|c| c.id != id);
    save_state(&state, &st)
}

// ─── Format invocation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct FormatRequest {
    pub language: String,
    pub source: String,
    /// File path (or synthetic name) — used both for `{file}` substitution and
    /// for formatters like prettier/clang-format that derive language from
    /// extension.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatResponse {
    pub formatted: String,
    pub formatter_id: String,
}

/// Resolve which formatter to invoke for a language. Resolution order:
///   1. Custom formatter whose `languages` includes the request language.
///   2. Built-in formatter whose `languages` includes the request language AND
///      is installed/detected.
/// Returns None if no formatter is available.
fn resolve_formatter(
    state: &FormattersState,
    language: &str,
) -> Option<ResolvedFormatter> {
    let lang = language.to_ascii_lowercase();
    if let Some(c) = state
        .custom
        .iter()
        .find(|c| c.languages.iter().any(|l| l.eq_ignore_ascii_case(&lang)))
    {
        return Some(ResolvedFormatter::Custom(c.clone()));
    }
    for b in REGISTRY {
        if b.languages.iter().any(|l| l.eq_ignore_ascii_case(&lang)) {
            return Some(ResolvedFormatter::Builtin(b));
        }
    }
    None
}

enum ResolvedFormatter {
    Builtin(&'static BuiltinFormatter),
    Custom(CustomFormatter),
}

#[tauri::command]
pub async fn formatter_format(
    app: AppHandle,
    state: State<'_, AppState>,
    req: FormatRequest,
) -> Result<FormatResponse, String> {
    let persisted = load_state(&state)?;
    let resolved = resolve_formatter(&persisted, &req.language)
        .ok_or_else(|| format!("No formatter configured for language '{}'", req.language))?;

    let (id, command, raw_args, use_stdin) = match resolved {
        ResolvedFormatter::Builtin(b) => {
            let bin = platform_binary_name(b.binary);
            let resolved_path = match b.install_kind {
                InstallKind::Download => {
                    let rec = persisted.installed.get(b.id);
                    match rec {
                        Some(r) if Path::new(&r.path).is_file() => PathBuf::from(&r.path),
                        _ => {
                            // Fall back to checking the install dir directly
                            let p = formatter_install_path(&app, b.id, &bin)?;
                            if p.is_file() { p } else {
                                return Err(format!("'{}' is not installed. Open Formatters and install it first.", b.display_name));
                            }
                        }
                    }
                }
                InstallKind::Toolchain => {
                    which(&bin).ok_or_else(|| format!(
                        "'{}' not found on PATH. Install it or open Formatters for instructions.",
                        b.display_name
                    ))?
                }
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
    let ext = Path::new(&file_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let args: Vec<String> = raw_args
        .iter()
        .map(|a| a.replace("{file}", &file_path).replace("{ext}", ext))
        .collect();

    let formatted = run_formatter(&command, &args, &req.source, use_stdin).await?;
    Ok(FormatResponse { formatted, formatter_id: id })
}

async fn run_formatter(
    command: &Path,
    args: &[String],
    source: &str,
    use_stdin: bool,
) -> Result<String, String> {
    // Run on a blocking thread — Command IO is blocking and we don't want to
    // tie up the async runtime.
    let cmd_path = command.to_path_buf();
    let args = args.to_vec();
    let source = source.to_string();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(&cmd_path);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if use_stdin {
            cmd.stdin(Stdio::piped());
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // 0x08000000 = CREATE_NO_WINDOW — keeps a console flash from
            // popping up when spawning console tools from a Tauri app.
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        if use_stdin {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(source.as_bytes()).map_err(|e| e.to_string())?;
            }
            // Drop the handle to send EOF — without this, some formatters
            // (gofmt, ruff) block forever waiting for more input.
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

async fn fetch_latest_release(repo: &str) -> Result<GithubRelease, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent("rustic-editor")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }
    resp.json::<GithubRelease>().await.map_err(|e| e.to_string())
}

/// Returns the (asset_name_substrings, is_zip) pair that identifies the asset
/// to download on the current platform. The substring match is loose: we
/// require all substrings present, case-insensitively. Each tool's release
/// pages use slightly different naming so we keep the patterns per-tool.
fn platform_asset_pattern(formatter_id: &str) -> Result<(Vec<&'static str>, bool), String> {
    let os = if cfg!(target_os = "windows") { "windows" }
             else if cfg!(target_os = "macos") { "darwin" }
             else if cfg!(target_os = "linux") { "linux" }
             else { return Err("unsupported OS".into()) };
    let arch = if cfg!(target_arch = "x86_64") { "x86_64" }
               else if cfg!(target_arch = "aarch64") { "aarch64" }
               else { return Err("unsupported arch".into()) };
    Ok(match formatter_id {
        // ruff releases use rust target triples and zip archives.
        // Example: ruff-x86_64-pc-windows-msvc.zip / ruff-aarch64-apple-darwin.tar.gz
        "ruff" => {
            let tokens: Vec<&'static str> = match (os, arch) {
                ("windows", "x86_64")  => vec!["ruff-", "x86_64", "windows", ".zip"],
                ("windows", "aarch64") => vec!["ruff-", "aarch64", "windows", ".zip"],
                ("macos",   "x86_64")  => vec!["ruff-", "x86_64", "apple", ".tar.gz"],
                ("macos",   "aarch64") => vec!["ruff-", "aarch64", "apple", ".tar.gz"],
                ("linux",   "x86_64")  => vec!["ruff-", "x86_64", "linux", ".tar.gz"],
                ("linux",   "aarch64") => vec!["ruff-", "aarch64", "linux", ".tar.gz"],
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

async fn download_and_extract(
    app: &AppHandle,
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
        .ok_or_else(|| {
            format!(
                "no matching asset in release {} for tokens {:?}",
                release.tag_name, tokens
            )
        })?;

    let client = reqwest::Client::builder()
        .user_agent("rustic-editor")
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let dir = formatters_dir(app)?.join(b.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let binary_name = platform_binary_name(b.binary);
    let dest = dir.join(&binary_name);

    if is_archive {
        // Zip extraction. ruff zips contain a top-level dir with the binary inside.
        let reader = std::io::Cursor::new(bytes.as_ref());
        let mut zip = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
        let mut wrote = false;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = file.name();
            // Match either "ruff" or "ruff.exe" (case-insensitive) at end of path.
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(&binary_name.to_ascii_lowercase()) && !file.is_dir() {
                let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
                let mut buf = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                out.write_all(&buf).map_err(|e| e.to_string())?;
                wrote = true;
                break;
            }
        }
        if !wrote {
            return Err(format!("binary '{binary_name}' not found in archive"));
        }
    } else if asset.name.ends_with(".tar.gz") || asset.name.ends_with(".tgz") {
        // Used for ruff on macOS/Linux. ruff's tarballs contain `ruff-<triple>/ruff`.
        let cursor = std::io::Cursor::new(bytes.as_ref());
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut tarball = tar::Archive::new(gz);
        let mut wrote = false;
        let target_lower = binary_name.to_ascii_lowercase();
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
        // Plain binary (shfmt) — just write it to disk.
        std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    }

    // Mark executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
    }

    Ok(())
}
