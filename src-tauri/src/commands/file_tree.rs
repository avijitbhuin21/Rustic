use ignore::WalkBuilder;
use rustic_core::workspace::file_tree::{self, FileNode};
use std::path::Path;

use crate::path_scope::{validate_readable_path, validate_writable_path};

#[tauri::command]
pub async fn read_dir(path: String) -> Result<Vec<FileNode>, String> {
    let path = Path::new(&path);
    validate_readable_path(path)?;
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }

    file_tree::read_directory(path, 0).map_err(|e| e.to_string())
}

/// List every file under `root_path` as forward-slash relative paths, honoring
/// `.gitignore` and skipping a hardcoded set of heavy directories. Used by the
/// chat input's `@` mention picker to offer file references.
///
/// The walk stops early once `max_files` entries are collected so that huge
/// monorepos don't freeze the UI — callers should pick a cap around 5000.
#[tauri::command]
pub async fn list_project_files(
    root_path: String,
    max_files: Option<usize>,
) -> Result<Vec<String>, String> {
    let root = Path::new(&root_path);
    validate_readable_path(root)?;
    if !root.exists() || !root.is_dir() {
        return Err(format!("Directory does not exist: {}", root.display()));
    }

    // Belt-and-suspenders on top of .gitignore: these directories are rarely
    // useful in a file picker and often huge even when not gitignored.
    const HARD_SKIP: &[&str] = &[
        ".git", "node_modules", "target", "dist", "build", ".next",
        ".venv", "venv", "__pycache__", ".cache", ".turbo", ".parcel-cache",
    ];

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
        // Normalize to forward slashes for display / filtering consistency.
        let s = rel.to_string_lossy().replace('\\', "/");
        if !s.is_empty() {
            out.push(s);
        }
    }

    Ok(out)
}

#[tauri::command]
pub async fn read_file_content(path: String) -> Result<String, String> {
    let path = Path::new(&path);
    validate_readable_path(path)?;
    if !path.exists() || !path.is_file() {
        return Err(format!("File does not exist: {}", path.display()));
    }

    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_file(dir_path: String, name: String) -> Result<String, String> {
    let full_path = Path::new(&dir_path).join(&name);
    validate_writable_path(&full_path)?;
    if full_path.exists() {
        return Err(format!("File already exists: {}", full_path.display()));
    }
    rustic_core::io_util::atomic_write(&full_path, b"").map_err(|e| e.to_string())?;
    Ok(full_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn create_folder(dir_path: String, name: String) -> Result<String, String> {
    let full_path = Path::new(&dir_path).join(&name);
    validate_writable_path(&full_path)?;
    if full_path.exists() {
        return Err(format!("Folder already exists: {}", full_path.display()));
    }
    std::fs::create_dir_all(&full_path).map_err(|e| e.to_string())?;
    Ok(full_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn rename_entry(old_path: String, new_name: String) -> Result<String, String> {
    let old = Path::new(&old_path);
    validate_writable_path(old)?;
    if !old.exists() {
        return Err(format!("Path does not exist: {}", old.display()));
    }
    let new_path = old
        .parent()
        .ok_or_else(|| "Cannot determine parent directory".to_string())?
        .join(&new_name);
    validate_writable_path(&new_path)?;
    if new_path.exists() {
        return Err(format!("Already exists: {}", new_path.display()));
    }
    std::fs::rename(old, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn delete_entry(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    validate_writable_path(p)?;
    if !p.exists() {
        return Err(format!("Path does not exist: {}", p.display()));
    }
    if p.is_dir() {
        std::fs::remove_dir_all(p).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(p).map_err(|e| e.to_string())
    }
}

/// Recursively copy a file or directory into `dst_dir`.
///
/// If `new_name` is provided it's used as the destination name. If a file or
/// folder with that name already exists in `dst_dir`, a numeric suffix is
/// appended (`foo.txt` → `foo (1).txt`, `foo (2).txt`, …) — matching the
/// auto-rename behavior of Windows Explorer / Finder so paste never silently
/// overwrites an existing entry.
///
/// Returns the final destination path as a string (forward slashes preserved
/// from the input where relevant).
/// Read absolute file/folder paths from the OS clipboard. On Windows this
/// catches the CF_HDROP file list that Windows Explorer / Finder put on the
/// clipboard when you press Ctrl+C on a file (which the webview's
/// `navigator.clipboard.readText()` cannot see — that only sees CF_TEXT).
///
/// Implementation note: rather than pulling in a dedicated clipboard crate,
/// we shell out to PowerShell on Windows and `pbpaste` / `xclip` on
/// macOS / Linux. The PowerShell call uses `Get-Clipboard -Format FileDropList`,
/// which returns the same file list Explorer wrote — including paths copied
/// via Ctrl+C from another File Explorer window or another instance of this
/// app. Empty result is returned (not an error) when the clipboard has no
/// file list, so callers can fall back to text-based path detection.
#[tauri::command]
pub async fn read_clipboard_files() -> Result<Vec<String>, String> {
    #[cfg(target_os = "windows")]
    {
        // `Get-Clipboard -Format FileDropList` returns one path per line on
        // success and empty string when no file list is on the clipboard.
        // We use `-ErrorAction SilentlyContinue` so Powershell doesn't write
        // a noisy error to stderr in the empty case.
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Clipboard -Format FileDropList -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName }",
            ])
            // Hide the conhost window flash on Windows by setting CREATE_NO_WINDOW.
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("powershell launch failed: {}", e))?;

        if !output.status.success() {
            return Ok(vec![]);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let paths: Vec<String> = stdout
            .lines()
            .map(|s| s.trim_end_matches('\r').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(paths);
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: `osascript` to read the pasteboard's file list (NSFilenamesPboardType).
        // Returns empty if there's no file list on the pasteboard.
        let output = std::process::Command::new("osascript")
            .args([
                "-e",
                "try\n  set theList to the clipboard as «class furl»\n  POSIX path of theList\non error\n  return \"\"\nend try",
            ])
            .output()
            .map_err(|e| format!("osascript launch failed: {}", e))?;
        if !output.status.success() {
            return Ok(vec![]);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let paths: Vec<String> = stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(paths);
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: try `xclip -selection clipboard -t text/uri-list -o`.
        // Returns one `file://...` URI per line on most desktops.
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list", "-o"])
            .output();
        let mut paths: Vec<String> = Vec::new();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let s = line.trim();
                    if let Some(rest) = s.strip_prefix("file://") {
                        // urlencoded — decode percent escapes
                        let decoded = percent_decode_simple(rest);
                        paths.push(decoded);
                    }
                }
            }
        }
        return Ok(paths);
    }
}

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Write a list of absolute file paths to the OS clipboard as a "file list"
/// (the same format Windows Explorer / Finder use for Ctrl+C on a file). After
/// this runs, pasting in any other app — Windows Explorer, Finder, Outlook,
/// Slack, an image-friendly app — drops actual file copies, not just the
/// path as text. We also keep a plain-text representation alongside (the
/// newline-joined paths) so apps that only know how to handle CF_TEXT still
/// get something useful.
///
/// `cut` controls the "preferred drop effect" on Windows so Explorer knows
/// whether to copy or move the file when the user pastes — same convention
/// Explorer itself uses.
///
/// Implementation: shells out to PowerShell on Windows. The PowerShell script
/// constructs a `System.Collections.Specialized.StringCollection` and calls
/// `[Windows.Forms.Clipboard]::SetFileDropList`, then sets the
/// "Preferred DropEffect" on the data object so paste-as-cut works. This is
/// the same dance Explorer does internally.
#[tauri::command]
pub async fn write_clipboard_files(paths: Vec<String>, cut: bool) -> Result<(), String> {
    // Normalize: skip blanks; nothing to do if list ends up empty.
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.is_empty()).collect();
    if paths.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        // Build a single-quoted PowerShell array literal. Each path has its
        // single quotes escaped (PS-style: `'` → `''`) so embedded apostrophes
        // in filenames don't break the script.
        let ps_paths: Vec<String> = paths
            .iter()
            .map(|p| format!("'{}'", p.replace('\'', "''")))
            .collect();
        let array_literal = ps_paths.join(",");

        // Drop effect codes: 5 = move (cut), 2 = copy.  See
        // https://learn.microsoft.com/en-us/windows/win32/com/clipboard-formats
        let drop_effect: u8 = if cut { 5 } else { 2 };

        // The script:
        //   * Loads WinForms so [Clipboard]::SetFileDropList is available
        //   * Builds a StringCollection of paths
        //   * Calls Clipboard.SetDataObject(dataObject, true) so the data
        //     persists on the clipboard after PowerShell exits
        //   * Sets "Preferred DropEffect" so Explorer knows copy-vs-move
        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
$paths = @({arr})
$col = New-Object System.Collections.Specialized.StringCollection
foreach ($p in $paths) {{ [void]$col.Add($p) }}
$dataObj = New-Object System.Windows.Forms.DataObject
$dataObj.SetFileDropList($col)
$ms = New-Object System.IO.MemoryStream
$bytes = [byte[]]({eff},0,0,0)
$ms.Write($bytes,0,$bytes.Length)
$dataObj.SetData('Preferred DropEffect',$ms)
[System.Windows.Forms.Clipboard]::SetDataObject($dataObj,$true)
"#,
            arr = array_literal,
            eff = drop_effect,
        );

        // STA threading is required for the WinForms clipboard APIs — pass
        // `-Sta` so PowerShell starts in single-threaded apartment mode.
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Sta",
                "-Command",
                &script,
            ])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("powershell launch failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("clipboard write failed: {}", stderr.trim()));
        }
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Use Swift via osascript to set NSFilenamesPboardType. Simpler form:
        // use AppleScript's `set the clipboard to {file "<posix path>", ...}`.
        // We escape backslashes and double-quotes in each path before embedding.
        let mut applescript = String::from("set the clipboard to {");
        for (i, p) in paths.iter().enumerate() {
            if i > 0 {
                applescript.push_str(", ");
            }
            // POSIX path -> file alias
            let escaped = p.replace('\\', "\\\\").replace('"', "\\\"");
            applescript.push_str(&format!("(POSIX file \"{}\")", escaped));
        }
        applescript.push('}');
        let _ = cut; // macOS pasteboard doesn't expose copy-vs-move; same flow either way.
        let output = std::process::Command::new("osascript")
            .args(["-e", &applescript])
            .output()
            .map_err(|e| format!("osascript launch failed: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("clipboard write failed: {}", stderr.trim()));
        }
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: write a `text/uri-list` blob via xclip. xclip reads from
        // stdin so we pipe our URI list in.
        use std::io::Write;
        let _ = cut; // We can't carry copy/move semantics via xclip.
        let body: String = paths
            .iter()
            .map(|p| format!("file://{}", p))
            .collect::<Vec<_>>()
            .join("\n");
        let mut child = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("xclip launch failed: {}", e))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(body.as_bytes())
                .map_err(|e| format!("xclip stdin write failed: {}", e))?;
        }
        let status = child
            .wait()
            .map_err(|e| format!("xclip wait failed: {}", e))?;
        if !status.success() {
            return Err("xclip exited non-zero".to_string());
        }
        return Ok(());
    }
}


#[cfg(target_os = "linux")]
fn percent_decode_simple(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stat a path so the frontend can decide whether something the user copied
/// from another app (Windows Explorer, VS Code, etc.) is a real file/folder
/// it can paste. Returns `None` for paths that don't exist; otherwise
/// `(name, is_dir)`. Cheap — single `metadata()` call.
#[tauri::command]
pub async fn stat_path(path: String) -> Result<Option<(String, bool)>, String> {

    let p = Path::new(&path);
    // stat() reads no file content, but it confirms presence of secret files
    // (e.g. ~/.ssh/id_rsa). Apply the same readable-path scope as content reads.
    if validate_readable_path(p).is_err() {
        return Ok(None);
    }
    if !p.exists() {
        return Ok(None);
    }
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    Ok(Some((name, meta.is_dir())))
}

#[tauri::command]
pub async fn copy_entry(

    src_path: String,
    dst_dir: String,
    new_name: Option<String>,
) -> Result<String, String> {
    let src = Path::new(&src_path);
    validate_readable_path(src)?;
    if !src.exists() {
        return Err(format!("Source does not exist: {}", src.display()));
    }
    let dst_root = Path::new(&dst_dir);
    validate_writable_path(dst_root)?;
    if !dst_root.exists() || !dst_root.is_dir() {
        return Err(format!(
            "Destination directory does not exist: {}",
            dst_root.display()
        ));
    }

    // Refuse to copy a directory into itself or any of its descendants —
    // would either fail mid-copy with a partial tree or recurse forever.
    if src.is_dir() {
        let src_can = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        let dst_can =
            std::fs::canonicalize(dst_root).unwrap_or_else(|_| dst_root.to_path_buf());
        if dst_can.starts_with(&src_can) {
            return Err("Cannot copy a folder into itself".to_string());
        }
    }

    let base_name = new_name.unwrap_or_else(|| {
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

    Ok(final_path.to_string_lossy().to_string())
}

/// Generate a non-colliding destination path inside `dst_dir`.
/// `foo.txt` → `foo.txt`, then `foo (1).txt`, `foo (2).txt`, …
/// For names without an extension (or directories) we append the suffix
/// to the whole name: `foo` → `foo (1)`.
fn unique_destination(dst_dir: &Path, name: &str) -> std::path::PathBuf {
    let candidate = dst_dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    // Split into stem + extension. `Path::file_stem` / `Path::extension` work
    // for files; for "foo" with no dot, stem == "foo" and ext == None.
    let (stem, ext) = match name.rsplit_once('.') {
        // Hidden files like ".env" — treat the whole thing as the stem
        // (rsplit_once returns ("", "env") which we don't want).
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
    // Extreme fallback — should never happen
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
            // Best-effort: copy the link target as a regular file. Symlinks
            // on Windows require elevated privileges to recreate so we don't
            // try to round-trip them.
            if let Ok(target) = std::fs::read_link(&from) {
                if target.exists() && target.is_file() {
                    std::fs::copy(&target, &to).ok();
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", p.display()));
    }
    validate_readable_path(p)?;

    // Reject argument-injection metacharacters and control bytes. Explorer
    // re-parses its raw command line on Windows; comma in particular is the
    // /select,<path> separator and could be coerced if the path contains one.
    if path.contains(',') || path.contains('"') || path.contains('\n')
        || path.contains('\r') || path.contains('\0')
    {
        return Err("Path contains characters not permitted by reveal_in_file_manager".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        // Reject UNC and DOS-device paths so explorer never reaches a remote
        // SMB share (which would leak NTLM credentials on connect).
        let starts_with_unc = path.starts_with(r"\\") || path.starts_with("//");
        if starts_with_unc {
            return Err("UNC paths are not allowed for reveal_in_file_manager".to_string());
        }

        // Canonicalize and strip any leading \\?\ (long-path prefix). After
        // this `final_path` is a plain absolute path with no UNC prefix.
        let canon = std::fs::canonicalize(p).map_err(|e| e.to_string())?;
        let canon_str = canon.to_string_lossy().to_string();
        let final_path = canon_str
            .strip_prefix(r"\\?\UNC\")
            .map(return_err_for_unc)
            .unwrap_or_else(|| Ok(canon_str.trim_start_matches(r"\\?\").to_string()))?;
        if final_path.starts_with(r"\\") {
            return Err("UNC paths are not allowed for reveal_in_file_manager".to_string());
        }
        if final_path.contains(',') {
            return Err("Path contains a comma; cannot be passed to explorer.exe".to_string());
        }

        if canon.is_dir() {
            std::process::Command::new("explorer")
                .arg(&final_path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("explorer")
                .arg(format!("/select,{}", final_path))
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if p.is_dir() {
            std::process::Command::new("open")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("open")
                .args(["-R", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let dir = if p.is_dir() {
            path.clone()
        } else {
            p.parent()
                .map(|pp| pp.to_string_lossy().to_string())
                .unwrap_or(path.clone())
        };
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn return_err_for_unc(_: &str) -> Result<String, String> {
    Err("UNC paths are not allowed for reveal_in_file_manager".to_string())
}
