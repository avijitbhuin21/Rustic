//! File-tree commands: read/create/rename/delete/stat filesystem entries.
//! Path-based and identical to the desktop bodies, with path-scope guards.

use std::path::Path;

use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use rustic_app::context::AppContext;
use rustic_app::path_scope::{validate_readable_path, validate_writable_path};

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirNameArg {
    dir_path: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProjectFilesArg {
    root_path: String,
    max_files: Option<usize>,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "fs_picker_roots" => fs_picker_roots(ctx),
        "read_dir" => match parse::<PathArg>(args) {
            Ok(a) => read_dir(a.path).await,
            Err(e) => Err(e),
        },
        "read_file_content" => match parse::<PathArg>(args) {
            Ok(a) => read_file_content(a.path).await,
            Err(e) => Err(e),
        },
        "list_project_files" => match parse::<ListProjectFilesArg>(args) {
            Ok(a) => list_project_files(a.root_path, a.max_files).await,
            Err(e) => Err(e),
        },
        "create_file" => match parse::<DirNameArg>(args) {
            Ok(a) => create_file(a.dir_path, a.name),
            Err(e) => Err(e),
        },
        "create_folder" => match parse::<DirNameArg>(args) {
            Ok(a) => create_folder(a.dir_path, a.name),
            Err(e) => Err(e),
        },
        "rename_entry" => rename_entry(args),
        "delete_entry" => match parse::<PathArg>(args) {
            Ok(a) => delete_entry(a.path),
            Err(e) => Err(e),
        },
        "stat_path" => match parse::<PathArg>(args) {
            Ok(a) => stat_path(a.path),
            Err(e) => Err(e),
        },
        "copy_entry" => copy_entry(args),
        "move_entry" => move_entry(args),
        "save_pasted_image_base64" => save_pasted_image_base64(args),
        // DESKTOP-NATIVE — cannot be served headless, intentionally left
        // unhandled so they return 501 (no silent fallback):
        //   * read_clipboard_files / write_clipboard_files: read/write the OS
        //     clipboard via PowerShell/osascript/xclip on the user's machine.
        //   * paste_clipboard_image_into: pulls a bitmap off the OS clipboard
        //     (Clipboard::GetImage etc.) — an OS-clipboard read, not a base64
        //     arg, so it cannot run on the server.
        //   * reveal_in_file_manager: launches the desktop file manager.
        _ => return None,
    })
}

/// Starting locations for the web file picker: the user's home directory plus
/// every mounted drive (Windows) or the filesystem root (Unix). Web-only — the
/// desktop uses the native OS picker.
fn fs_picker_roots(ctx: &ServerContext) -> Result<Value, ApiError> {
    let home = ctx.home_dir();
    let mut roots: Vec<Value> = Vec::new();

    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if Path::new(&drive).exists() {
                roots.push(json!({ "path": drive, "label": format!("{}:", letter as char) }));
            }
        }
    }
    #[cfg(not(windows))]
    {
        roots.push(json!({ "path": "/", "label": "/" }));
    }

    ok(json!({
        "home": home.to_string_lossy().to_string(),
        "roots": roots,
    }))
}

async fn read_dir(path: String) -> Result<Value, ApiError> {
    let p = Path::new(&path);
    validate_readable_path(p)?;
    if !p.exists() || !p.is_dir() {
        return Err(ApiError::bad(format!(
            "Directory does not exist: {}",
            p.display()
        )));
    }
    let path2 = path.clone();
    let nodes = tokio::task::spawn_blocking(move || {
        rustic_core::workspace::file_tree::read_directory(Path::new(&path2), 0)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ApiError::bad(format!("read_dir task failed: {e}")))??;
    ok(nodes)
}

async fn read_file_content(path: String) -> Result<Value, ApiError> {
    let p = Path::new(&path);
    validate_readable_path(p)?;
    if !p.exists() || !p.is_file() {
        return Err(ApiError::bad(format!(
            "File does not exist: {}",
            p.display()
        )));
    }
    let content = tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok::<String, String>(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    })
    .await
    .map_err(|e| ApiError::bad(format!("read task failed: {e}")))??;
    ok(content)
}

/// List every file under `root_path` as forward-slash relative paths, honoring
/// `.gitignore` and skipping a hardcoded set of heavy directories. Verbatim copy
/// of the desktop `list_project_files` body (gitignore-aware `WalkBuilder`).
async fn list_project_files(
    root_path: String,
    max_files: Option<usize>,
) -> Result<Value, ApiError> {
    {
        let root = Path::new(&root_path);
        validate_readable_path(root)?;
        if !root.exists() || !root.is_dir() {
            return Err(ApiError::bad(format!(
                "Directory does not exist: {}",
                root.display()
            )));
        }
    }

    let out = tokio::task::spawn_blocking(move || {
        // Belt-and-suspenders on top of .gitignore: these directories are rarely
        // useful in a file picker and often huge even when not gitignored.
        const HARD_SKIP: &[&str] = &[
            ".git",
            "node_modules",
            "target",
            "dist",
            "build",
            ".next",
            ".venv",
            "venv",
            "__pycache__",
            ".cache",
            ".turbo",
            ".parcel-cache",
        ];

        let root = Path::new(&root_path);
        let cap = max_files.unwrap_or(5000);
        let mut out: Vec<String> = Vec::with_capacity(cap.min(1024));

        let walker = WalkBuilder::new(root)
            .hidden(false) // allow dotfiles like .env.example — .gitignore still applies
            .git_ignore(true)
            .git_exclude(true)
            .filter_entry(|entry| {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let name = entry.file_name().to_string_lossy();
                    return !HARD_SKIP.iter().any(|s| *s == name);
                }
                true
            })
            .build();

        for entry in walker.flatten() {
            if out.len() >= cap {
                break;
            }
            let ft = match entry.file_type() {
                Some(t) => t,
                None => continue,
            };
            if !ft.is_file() {
                continue;
            }
            let rel = match entry.path().strip_prefix(root) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let s = rel.to_string_lossy().replace('\\', "/");
            if !s.is_empty() {
                out.push(s);
            }
        }

        Ok::<_, String>(out)
    })
    .await
    .map_err(|e| ApiError::bad(format!("list_project_files task failed: {e}")))??;

    ok(out)
}

fn create_file(dir_path: String, name: String) -> Result<Value, ApiError> {
    let full = Path::new(&dir_path).join(&name);
    validate_writable_path(&full)?;
    if full.exists() {
        return Err(ApiError::bad(format!(
            "File already exists: {}",
            full.display()
        )));
    }
    rustic_core::io_util::atomic_write(&full, b"").map_err(|e| e.to_string())?;
    ok(full.to_string_lossy().to_string())
}

fn create_folder(dir_path: String, name: String) -> Result<Value, ApiError> {
    let full = Path::new(&dir_path).join(&name);
    validate_writable_path(&full)?;
    if full.exists() {
        return Err(ApiError::bad(format!(
            "Folder already exists: {}",
            full.display()
        )));
    }
    std::fs::create_dir_all(&full).map_err(|e| e.to_string())?;
    ok(full.to_string_lossy().to_string())
}

fn rename_entry(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        old_path: String,
        new_name: String,
    }
    let a: A = parse(args)?;
    let old = Path::new(&a.old_path);
    validate_writable_path(old)?;
    if !old.exists() {
        return Err(ApiError::bad(format!(
            "Path does not exist: {}",
            old.display()
        )));
    }
    let new_path = old
        .parent()
        .ok_or_else(|| ApiError::bad("Cannot determine parent directory"))?
        .join(&a.new_name);
    validate_writable_path(&new_path)?;
    if new_path.exists() {
        return Err(ApiError::bad(format!(
            "Already exists: {}",
            new_path.display()
        )));
    }
    std::fs::rename(old, &new_path).map_err(|e| e.to_string())?;
    ok(new_path.to_string_lossy().to_string())
}

fn delete_entry(path: String) -> Result<Value, ApiError> {
    let p = Path::new(&path);
    validate_writable_path(p)?;
    if !p.exists() {
        return Err(ApiError::bad(format!(
            "Path does not exist: {}",
            p.display()
        )));
    }
    if p.is_dir() {
        std::fs::remove_dir_all(p).map_err(|e| e.to_string())?;
    } else {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    ok(json!(null))
}

fn stat_path(path: String) -> Result<Value, ApiError> {
    let p = Path::new(&path);
    validate_readable_path(p)?;
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    ok(json!({
        "exists": true,
        "isDir": meta.is_dir(),
        "isFile": meta.is_file(),
        "size": meta.len(),
    }))
}

/// Recursively copy a file or directory into `dst_dir`. Identical to the
/// desktop `copy_entry` body: collision-avoidance via `unique_destination`,
/// refuses to copy a folder into itself/descendants, returns the final path.
fn copy_entry(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        src_path: String,
        dst_dir: String,
        new_name: Option<String>,
    }
    let a: A = parse(args)?;
    let src = Path::new(&a.src_path);
    validate_readable_path(src)?;
    if !src.exists() {
        return Err(ApiError::bad(format!(
            "Source does not exist: {}",
            src.display()
        )));
    }
    let dst_root = Path::new(&a.dst_dir);
    validate_writable_path(dst_root)?;
    if !dst_root.exists() || !dst_root.is_dir() {
        return Err(ApiError::bad(format!(
            "Destination directory does not exist: {}",
            dst_root.display()
        )));
    }

    if src.is_dir() {
        let src_can = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        let dst_can = std::fs::canonicalize(dst_root).unwrap_or_else(|_| dst_root.to_path_buf());
        if dst_can.starts_with(&src_can) {
            return Err(ApiError::bad("Cannot copy a folder into itself"));
        }
    }

    let base_name = a.new_name.unwrap_or_else(|| {
        src.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string())
    });

    let final_path = unique_destination(dst_root, &base_name);

    if src.is_dir() {
        copy_dir_recursive(src, &final_path).map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(src, &final_path).map_err(|e| e.to_string())?;
    }

    ok(final_path.to_string_lossy().to_string())
}

/// Move a file or directory into `dst_dir`, preserving its name. Atomic
/// `rename` with copy+delete fallback for cross-device moves. Identical to the
/// desktop `move_entry` body.
fn move_entry(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        src_path: String,
        dst_dir: String,
    }
    let a: A = parse(args)?;
    let src = Path::new(&a.src_path);
    validate_writable_path(src)?;
    if !src.exists() {
        return Err(ApiError::bad(format!(
            "Source does not exist: {}",
            src.display()
        )));
    }
    let dst_root = Path::new(&a.dst_dir);
    validate_writable_path(dst_root)?;
    if !dst_root.exists() || !dst_root.is_dir() {
        return Err(ApiError::bad(format!(
            "Destination directory does not exist: {}",
            dst_root.display()
        )));
    }

    if src.is_dir() {
        let src_can = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        let dst_can = std::fs::canonicalize(dst_root).unwrap_or_else(|_| dst_root.to_path_buf());
        if dst_can.starts_with(&src_can) {
            return Err(ApiError::bad("Cannot move a folder into itself"));
        }
    }

    let base_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let final_path = unique_destination(dst_root, &base_name);

    match std::fs::rename(src, &final_path) {
        Ok(()) => {}
        Err(_) => {
            // Cross-device move: copy then remove source.
            if src.is_dir() {
                copy_dir_recursive(src, &final_path).map_err(|e| e.to_string())?;
                std::fs::remove_dir_all(src).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(src, &final_path).map_err(|e| e.to_string())?;
                std::fs::remove_file(src).map_err(|e| e.to_string())?;
            }
        }
    }

    ok(final_path.to_string_lossy().to_string())
}

/// Decode a base64 image payload (no data URL prefix) and write it under
/// `<dst_dir>/pasted-image[-N].png`, returning the absolute path. Identical to
/// the desktop `save_pasted_image_base64` body — pure disk write, no clipboard.
fn save_pasted_image_base64(args: &Value) -> Result<Value, ApiError> {
    use base64::Engine as _;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        dst_dir: String,
        data: String,
    }
    let a: A = parse(args)?;
    let dst_root = Path::new(&a.dst_dir);
    validate_writable_path(dst_root)?;
    if !dst_root.exists() {
        std::fs::create_dir_all(dst_root).map_err(|e| {
            format!(
                "Couldn't create destination directory {}: {}",
                dst_root.display(),
                e
            )
        })?;
    } else if !dst_root.is_dir() {
        return Err(ApiError::bad(format!(
            "Destination path exists but is not a directory: {}",
            dst_root.display()
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(a.data.as_bytes())
        .map_err(|e| format!("invalid base64: {e}"))?;
    if bytes.len() as u64 > 100 * 1024 * 1024 {
        return Err(ApiError::bad("Refusing to write file larger than 100MB"));
    }
    let final_path = unique_pasted_image_path(dst_root);
    std::fs::write(&final_path, &bytes).map_err(|e| e.to_string())?;
    ok(final_path.to_string_lossy().into_owned())
}

/// Pick `<dst_dir>/pasted-image.png` if free, otherwise `pasted-image-N.png`.
/// Mirrors the desktop helper of the same name.
fn unique_pasted_image_path(dst_dir: &Path) -> std::path::PathBuf {
    let base = dst_dir.join("pasted-image.png");
    if !base.exists() {
        return base;
    }
    for i in 1..=9999 {
        let candidate = dst_dir.join(format!("pasted-image-{}.png", i));
        if !candidate.exists() {
            return candidate;
        }
    }
    dst_dir.join(format!("pasted-image-{}.png", uuid_like_suffix()))
}

/// Generate a non-colliding destination path inside `dst_dir`:
/// `foo.txt` → `foo.txt`, then `foo (1).txt`, `foo (2).txt`, …
/// Mirrors the desktop helper of the same name.
pub(crate) fn unique_destination(dst_dir: &Path, name: &str) -> std::path::PathBuf {
    let candidate = dst_dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let (stem, ext) = match name.rsplit_once('.') {
        Some(("", _)) => (name.to_string(), String::new()),
        Some((s, e)) => (s.to_string(), format!(".{}", e)),
        None => (name.to_string(), String::new()),
    };

    for i in 1..=9999 {
        let candidate_name = format!("{} ({}){}", stem, i, ext);
        let candidate = dst_dir.join(&candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dst_dir.join(format!("{}-{}", stem, uuid_like_suffix()))
}

fn uuid_like_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "x".to_string())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        } else if ty.is_symlink() {
            if let Ok(target) = std::fs::read_link(&from) {
                if target.exists() && target.is_file() {
                    std::fs::copy(&target, &to).ok();
                }
            }
        }
    }
    Ok(())
}

/// Reject path-traversal in a browser-supplied relative upload path and return
/// it as a normalized, component-by-component `PathBuf`. Returns `None` if any
/// component is `..`, absolute, or a drive/root prefix.
pub(crate) fn sanitize_relative(rel: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let normalized = rel.replace('\\', "/");
    let candidate = Path::new(&normalized);
    let mut out = std::path::PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}
