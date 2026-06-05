//! formatters commands — server dispatch.
//!
//! Mirrors `src-tauri/src/commands/formatters.rs`. The registry, persistent
//! state, path helpers, and format invocation are reimplemented verbatim so
//! browser behavior matches desktop. Settings persist through the same SQLite
//! `formatters` key the desktop uses.
//!
//! The download-backed commands (`formatter_install`, `formatter_update`,
//! `formatter_check_update`) are NOT wired here: they need `reqwest`/`zip`/
//! `tar`/`flate2`, none of which are dependencies of the `rustic-server` crate
//! (and we may not edit Cargo.toml). They fall through to a 501.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::state::AppState;
use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "formatter_registry" => formatter_registry(),
        "formatter_list" => formatter_list(ctx),
        "formatter_uninstall" => formatter_uninstall(ctx, args),
        "formatter_add_custom" => formatter_add_custom(ctx, args),
        "formatter_update_custom" => formatter_update_custom(ctx, args),
        "formatter_remove_custom" => formatter_remove_custom(ctx, args),
        "formatter_format" => formatter_format(ctx, args).await,
        "formatter_install" => formatter_install(ctx, args).await,
        "formatter_update" => formatter_update(ctx, args).await,
        "formatter_check_update" => formatter_check_update(args).await,
        _ => return None,
    })
}

// ─── Built-in registry ────────────────────────────────────────────────────────

/// How a formatter is acquired.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    Download,
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
    pub binary: &'static str,
    pub args: &'static [&'static str],
    pub stdin: bool,
    pub github: Option<&'static str>,
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
];

fn formatter_registry() -> Result<Value, ApiError> {
    let list: Vec<BuiltinDescriptor> = REGISTRY.iter().map(BuiltinDescriptor::from).collect();
    ok(list)
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

#[derive(Debug, Clone, Copy, Serialize)]
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

// ─── Path helpers ─────────────────────────────────────────────────────────────

fn formatters_dir(ctx: &ServerContext) -> Result<PathBuf, String> {
    let dir = ctx.data_dir.join("formatters");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn formatter_install_path(ctx: &ServerContext, id: &str, binary: &str) -> Result<PathBuf, String> {
    let dir = formatters_dir(ctx)?.join(id);
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

fn formatter_list(ctx: &ServerContext) -> Result<Value, ApiError> {
    let state = ctx.state();
    let persisted = load_state(state)?;
    let mut entries = Vec::new();

    for b in REGISTRY {
        let desc = BuiltinDescriptor::from(b);
        let bin = platform_binary_name(b.binary);

        let status = if let Some(rec) = persisted.installed.get(b.id) {
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
            let candidate = formatter_install_path(ctx, b.id, &bin).ok();
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
        let resolved = if Path::new(&c.command).is_absolute() {
            Path::new(&c.command).is_file().then(|| c.command.clone())
        } else {
            which(&c.command).map(|p| p.to_string_lossy().to_string())
        };
        let kind = if resolved.is_some() {
            StatusKind::Detected
        } else {
            StatusKind::Missing
        };
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

    ok(entries)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdArg {
    id: String,
}

fn formatter_uninstall(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let state = ctx.state();
    let _ = lookup(&a.id).ok_or_else(|| format!("Unknown formatter '{}'", a.id))?;
    let dir = formatters_dir(ctx)?.join(&a.id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let mut st = load_state(state)?;
    st.installed.remove(&a.id);
    save_state(state, &st)?;
    ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatterArg {
    formatter: CustomFormatter,
}

fn formatter_add_custom(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: FormatterArg = parse(args)?;
    let formatter = a.formatter;
    let state = ctx.state();
    let mut st = load_state(state)?;
    if st.custom.iter().any(|c| c.id == formatter.id) {
        return Err(ApiError::from(format!(
            "A custom formatter with id '{}' already exists",
            formatter.id
        )));
    }
    if lookup(&formatter.id).is_some() {
        return Err(ApiError::from(format!(
            "'{}' is reserved by a built-in formatter",
            formatter.id
        )));
    }
    st.custom.push(formatter);
    save_state(state, &st)?;
    ok(())
}

fn formatter_update_custom(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: FormatterArg = parse(args)?;
    let formatter = a.formatter;
    let state = ctx.state();
    let mut st = load_state(state)?;
    let slot = st.custom.iter_mut().find(|c| c.id == formatter.id);
    match slot {
        Some(s) => {
            *s = formatter;
        }
        None => {
            return Err(ApiError::from(format!(
                "No custom formatter with id '{}'",
                formatter.id
            )))
        }
    }
    save_state(state, &st)?;
    ok(())
}

fn formatter_remove_custom(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let state = ctx.state();
    let mut st = load_state(state)?;
    st.custom.retain(|c| c.id != a.id);
    save_state(state, &st)?;
    ok(())
}

// ─── Format invocation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatRequest {
    pub language: String,
    pub source: String,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatResponse {
    pub formatted: String,
    pub formatter_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatReqArg {
    req: FormatRequest,
}

fn resolve_formatter(state: &FormattersState, language: &str) -> Option<ResolvedFormatter> {
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

async fn formatter_format(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: FormatReqArg = parse(args)?;
    let req = a.req;
    let state = ctx.state();
    let persisted = load_state(state)?;
    let resolved = resolve_formatter(&persisted, &req.language).ok_or_else(|| {
        format!("No formatter configured for language '{}'", req.language)
    })?;

    let (id, command, raw_args, use_stdin) = match resolved {
        ResolvedFormatter::Builtin(b) => {
            let bin = platform_binary_name(b.binary);
            let resolved_path = match b.install_kind {
                InstallKind::Download => {
                    let rec = persisted.installed.get(b.id);
                    match rec {
                        Some(r) if Path::new(&r.path).is_file() => PathBuf::from(&r.path),
                        _ => {
                            let p = formatter_install_path(ctx, b.id, &bin)?;
                            if p.is_file() {
                                p
                            } else {
                                return Err(ApiError::from(format!(
                                    "'{}' is not installed. Open Formatters and install it first.",
                                    b.display_name
                                )));
                            }
                        }
                    }
                }
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
    let ext = Path::new(&file_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let args: Vec<String> = raw_args
        .iter()
        .map(|a| a.replace("{file}", &file_path).replace("{ext}", ext))
        .collect();

    let formatted = run_formatter(&command, &args, &req.source, use_stdin).await?;
    ok(FormatResponse {
        formatted,
        formatter_id: id,
    })
}

async fn run_formatter(
    command: &Path,
    args: &[String],
    source: &str,
    use_stdin: bool,
) -> Result<String, String> {
    let cmd_path = command.to_path_buf();
    let args = args.to_vec();
    let source = source.to_string();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(&cmd_path);
        cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if use_stdin {
            cmd.stdin(Stdio::piped());
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        if use_stdin {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin
                    .write_all(source.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        String::from_utf8(output.stdout)
            .map_err(|e| format!("formatter produced invalid UTF-8: {e}"))
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

// ─── Install / update / check (GitHub release download) ────────────────────────

async fn formatter_install(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let id = a.id;
    let b = lookup(&id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    if !matches!(b.install_kind, InstallKind::Download) {
        return Err(ApiError::from(format!(
            "'{id}' is a toolchain formatter and cannot be installed by Rustic. Install it externally."
        )));
    }
    let github = b.github.ok_or("No GitHub repo configured")?;
    let release = fetch_latest_release(github).await?;
    download_and_extract(ctx, b, &release).await?;

    let bin = platform_binary_name(b.binary);
    let path = formatter_install_path(ctx, b.id, &bin)?;
    let rec = InstalledRecord {
        version: release.tag_name.clone(),
        path: path.to_string_lossy().to_string(),
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    let state = ctx.state();
    let mut st = load_state(state)?;
    st.installed.insert(b.id.to_string(), rec.clone());
    save_state(state, &st)?;
    ok(rec)
}

async fn formatter_update(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    // Same as install — fetches whatever the latest release tag is and
    // overwrites. We deliberately don't cache the "latest" check; each call
    // hits GitHub so the version shown to the user is always fresh.
    formatter_install(ctx, args).await
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub published_at: Option<String>,
}

async fn formatter_check_update(args: &Value) -> Result<Value, ApiError> {
    let a: IdArg = parse(args)?;
    let id = a.id;
    let b = lookup(&id).ok_or_else(|| format!("Unknown formatter '{id}'"))?;
    let github = b
        .github
        .ok_or("No GitHub repo configured for update checks")?;
    let release = fetch_latest_release(github).await?;
    ok(UpdateInfo {
        latest_version: release.tag_name,
        published_at: release.published_at,
    })
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
/// to download on the current platform.
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
    Ok(match formatter_id {
        "ruff" => {
            let tokens: Vec<&'static str> = match (os, arch) {
                ("windows", "x86_64") => vec!["ruff-", "x86_64", "windows", ".zip"],
                ("windows", "aarch64") => vec!["ruff-", "aarch64", "windows", ".zip"],
                ("macos", "x86_64") => vec!["ruff-", "x86_64", "apple", ".tar.gz"],
                ("macos", "aarch64") => vec!["ruff-", "aarch64", "apple", ".tar.gz"],
                ("linux", "x86_64") => vec!["ruff-", "x86_64", "linux", ".tar.gz"],
                ("linux", "aarch64") => vec!["ruff-", "aarch64", "linux", ".tar.gz"],
                _ => return Err("unsupported platform".into()),
            };
            let is_zip = tokens.last().is_some_and(|t| *t == ".zip");
            (tokens, is_zip)
        }
        "shfmt" => {
            let arch_word = if arch == "x86_64" { "amd64" } else { "arm64" };
            (vec!["shfmt_", os, arch_word], false)
        }
        _ => return Err(format!("no asset pattern for '{formatter_id}'")),
    })
}

async fn download_and_extract(
    ctx: &ServerContext,
    b: &BuiltinFormatter,
    release: &GithubRelease,
) -> Result<(), String> {
    let (tokens, is_archive) = platform_asset_pattern(b.id)?;
    let asset = release
        .assets
        .iter()
        .find(|a| {
            let name_lower = a.name.to_ascii_lowercase();
            tokens
                .iter()
                .all(|t| name_lower.contains(&t.to_ascii_lowercase()))
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

    let dir = formatters_dir(ctx)?.join(b.id);
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
        let mut perms = std::fs::metadata(&dest)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
    }

    Ok(())
}
