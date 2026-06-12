use ignore::WalkBuilder;
use rustic_core::workspace::file_tree::{self, FileNode};
use std::path::Path;

use crate::path_scope::{validate_readable_path, validate_writable_path};

#[tauri::command]
pub async fn read_dir(path: String) -> Result<Vec<FileNode>, String> {
    {
        let p = Path::new(&path);
        validate_readable_path(p)?;
        if !p.exists() || !p.is_dir() {
            return Err(format!("Directory does not exist: {}", p.display()));
        }
    }

    // `read_directory` does two synchronous walks (one with gitignore, one
    // without) plus disk stats per entry. On a large project root that adds
    // up — keep it off the runtime thread so other IPC commands stay
    // responsive while the walk is in flight.
    tauri::async_runtime::spawn_blocking(move || {
        file_tree::read_directory(Path::new(&path), 0).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("read_dir task failed: {}", e))?
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
    {
        let root = Path::new(&root_path);
        validate_readable_path(root)?;
        if !root.exists() || !root.is_dir() {
            return Err(format!("Directory does not exist: {}", root.display()));
        }
    }

    // The ignore-aware walk traverses the entire project — many seconds on
    // a 2 GB monorepo. Don't keep the runtime worker thread parked for that
    // long; hand it to a blocking pool task.
    tauri::async_runtime::spawn_blocking(move || {
        // Belt-and-suspenders on top of .gitignore: these directories are rarely
        // useful in a file picker and often huge even when not gitignored.
        const HARD_SKIP: &[&str] = &[
            ".git", "node_modules", "target", "dist", "build", ".next",
            ".venv", "venv", "__pycache__", ".cache", ".turbo", ".parcel-cache",
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
    .map_err(|e| format!("list_project_files task failed: {}", e))?
}

#[tauri::command]
pub async fn read_file_content(path: String) -> Result<String, String> {
    {
        let p = Path::new(&path);
        validate_readable_path(p)?;
        if !p.exists() || !p.is_file() {
            return Err(format!("File does not exist: {}", p.display()));
        }
    }

    // Sync `std::fs::read_to_string` blocks the tokio worker thread for as
    // long as the read takes — on a slow disk or a multi-MB source file,
    // long enough to back up every other Tauri command queued behind it.
    // Hop onto a blocking thread so the runtime stays responsive.
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    })
    .await
    .map_err(|e| format!("read_file_content task failed: {}", e))?
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
/// clipboard when you press Ctrl+C / Ctrl+X on a file (which the webview's
/// `navigator.clipboard.readText()` cannot see — that only sees CF_TEXT).
///
/// Returns the paths plus a `cut` flag (Windows "Preferred DropEffect" has
/// the MOVE bit set when the source app did a Ctrl+X) so the paste site can
/// move instead of copy, matching Explorer semantics. Empty list (not an
/// error) when the clipboard has no file list.
#[derive(serde::Serialize)]
pub struct ClipboardFileDrop {
    pub paths: Vec<String>,
    pub cut: bool,
}

#[tauri::command]
pub async fn read_clipboard_files() -> Result<ClipboardFileDrop, String> {
    #[cfg(target_os = "windows")]
    {
        // One STA PowerShell call reads BOTH the CF_HDROP file list and the
        // "Preferred DropEffect" stream. `GetData(FileDrop)` returns plain
        // strings, which sidesteps the FileInfo-vs-string shape difference
        // between PowerShell hosts that the previous
        // `Get-Clipboard -Format FileDropList | % { $_.FullName }` pipeline
        // silently tripped over (a string element has no .FullName → empty
        // output → "nothing to paste" with files on the clipboard).
        //
        // The payload is UTF-8 + base64 INSIDE PowerShell because redirected
        // powershell.exe stdout uses the legacy OEM codepage — a `…` or CJK
        // character in a filename gets best-fit-mangled (`…` → `.`), producing
        // a path that doesn't exist on disk. Base64 is pure ASCII and survives
        // any codepage.
        const SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
$d = [System.Windows.Forms.Clipboard]::GetDataObject()
if ($null -eq $d) { exit 0 }
if (-not $d.GetDataPresent([System.Windows.Forms.DataFormats]::FileDrop)) { exit 0 }
$files = $d.GetData([System.Windows.Forms.DataFormats]::FileDrop)
$eff = 0
try {
  $ms = $d.GetData('Preferred DropEffect')
  if ($ms -is [System.IO.MemoryStream]) { $b = New-Object byte[] 4; [void]$ms.Read($b, 0, 4); $eff = $b[0] }
} catch {}
$lines = @('EFFECT:' + $eff) + @($files | ForEach-Object { $_ })
Write-Output ([Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes(($lines -join [char]10))))
"#;
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Sta", "-Command", SCRIPT])
            // Hide the conhost window flash on Windows by setting CREATE_NO_WINDOW.
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| {
                tracing::warn!(target: "rustic::clipboard", error = %e, "read_clipboard_files: powershell launch failed");
                format!("powershell launch failed: {}", e)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            tracing::warn!(
                target: "rustic::clipboard",
                status = ?output.status.code(),
                stderr = %stderr.trim(),
                "read_clipboard_files: powershell exited non-zero"
            );
            return Ok(ClipboardFileDrop { paths: vec![], cut: false });
        }

        let encoded = stdout.trim();
        if encoded.is_empty() {
            tracing::info!(target: "rustic::clipboard", "read_clipboard_files: no file list on clipboard");
            return Ok(ClipboardFileDrop { paths: vec![], cut: false });
        }
        let decoded = {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|e| format!("clipboard payload decode failed: {e}"))?;
            String::from_utf8(bytes).map_err(|e| format!("clipboard payload not UTF-8: {e}"))?
        };

        let mut cut = false;
        let mut paths: Vec<String> = Vec::new();
        for line in decoded.lines() {
            let line = line.trim_end_matches('\r').trim();
            if line.is_empty() {
                continue;
            }
            if let Some(eff) = line.strip_prefix("EFFECT:") {
                // DROPEFFECT_MOVE == 2. Explorer copy sets 5 (COPY|LINK).
                cut = eff.trim().parse::<u8>().map(|v| v & 2 != 0).unwrap_or(false);
                continue;
            }
            paths.push(line.to_string());
        }
        tracing::info!(
            target: "rustic::clipboard",
            count = paths.len(),
            cut,
            stderr = %stderr.trim(),
            "read_clipboard_files: done"
        );
        return Ok(ClipboardFileDrop { paths, cut });
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
            return Ok(ClipboardFileDrop { paths: vec![], cut: false });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let paths: Vec<String> = stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(ClipboardFileDrop { paths, cut: false });
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
        return Ok(ClipboardFileDrop { paths, cut: false });
    }
}

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Create an empty destination file for an externally-dropped file. The
/// desktop webview receives OS drags as HTML5 drops carrying bytes, not
/// paths (Tauri's native drag-drop is disabled — it would break the HTML5
/// tab/editor DnD on WebView2), so the frontend streams the bytes across
/// in chunks: this command resolves a unique name (Explorer-style
/// `foo (1).mp4` auto-rename) and creates the empty file;
/// `write_upload_chunk` appends the data.
#[tauri::command]
pub async fn begin_drop_upload(dst_dir: String, file_name: String) -> Result<String, String> {
    let dir = Path::new(&dst_dir);
    if !dir.is_dir() {
        return Err(format!("Destination is not a folder: {}", dst_dir));
    }
    // Basename only — a crafted name must never escape the target folder.
    let name = Path::new(&file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("Invalid file name: {}", file_name))?;
    let dest = unique_destination(dir, name);
    std::fs::write(&dest, []).map_err(|e| format!("Create failed: {}", e))?;
    Ok(dest.to_string_lossy().to_string())
}

/// Append one chunk to a file created by `begin_drop_upload`. The bytes ride
/// as the raw IPC body (no JSON/base64 inflation — a video stays binary all
/// the way); the destination path travels URI-encoded in the `x-file-path`
/// header because raw-body invokes carry no JSON args.
#[tauri::command]
pub async fn write_upload_chunk(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    use std::io::Write;
    let path_header = request
        .headers()
        .get("x-file-path")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-file-path header")?;
    let path = percent_decode_simple(path_header);
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected a raw (binary) request body".into());
    };
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("Open failed for {}: {}", path, e))?;
    f.write_all(bytes)
        .map_err(|e| format!("Write failed for {}: {}", path, e))?;
    Ok(())
}

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

        // "Preferred DropEffect" codes, per the Explorer convention: copy = 5
        // (DROPEFFECT_COPY | DROPEFFECT_LINK), move/cut = 2 (DROPEFFECT_MOVE).
        // These were previously swapped (cut→5, copy→2), so a Rustic "copy"
        // tagged the clipboard as a MOVE — Explorer then moved (or refused) the
        // file on paste instead of copying it. See:
        // https://learn.microsoft.com/en-us/windows/win32/com/clipboard-formats
        let drop_effect: u8 = if cut { 2 } else { 5 };

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


/// Paste image **bitmap data** from the OS clipboard into `dst_dir`.
///
/// `read_clipboard_files` covers file PATHS on the clipboard (Ctrl+C on a
/// file in Explorer). This handles the other case: raw bitmap bytes — what
/// the clipboard carries when you copy an image from a browser, the
/// Snipping Tool, a paint program, or any other source that doesn't have
/// a backing file. Without this, "paste" in the project explorer is a
/// no-op for that very common workflow.
///
/// Returns `Ok(Some(path))` if an image was pasted, `Ok(None)` if no image
/// data was on the clipboard (caller can fall through to text-path
/// resolution or show "nothing to paste"). The written file is always
/// PNG — we re-encode whatever the clipboard format was so callers don't
/// need to sniff. Filename collisions get the same `(1)`, `(2)` … suffix
/// scheme `copy_entry` uses.
#[tauri::command]
pub async fn paste_clipboard_image_into(dst_dir: String) -> Result<Option<String>, String> {
    let dst_root = Path::new(&dst_dir);
    validate_writable_path(dst_root)?;
    // Auto-create the destination directory so callers (e.g. the chat / explorer
    // paste-into-uploads flow) don't have to pre-create `<project>/.rustic/
    // uploaded/<date>/` themselves. Mirrors `write_file_base64`'s behaviour.
    if !dst_root.exists() {
        std::fs::create_dir_all(dst_root).map_err(|e| {
            format!(
                "Couldn't create destination directory {}: {}",
                dst_root.display(),
                e
            )
        })?;
    } else if !dst_root.is_dir() {
        return Err(format!(
            "Destination path exists but is not a directory: {}",
            dst_root.display()
        ));
    }

    // Filename convention: `pasted-image.png` on first paste in a folder,
    // then `pasted-image-1.png`, `pasted-image-2.png` … on subsequent pastes.
    // Plain dash-N is what the user asked for and matches what most file
    // managers do when they auto-rename.
    let final_path = unique_pasted_image_path(dst_root);

    #[cfg(target_os = "windows")]
    {
        // PowerShell-based pull: `Get-Clipboard -Format Image` is what the
        // clipboard viewer uses. Saving as PNG via System.Drawing keeps the
        // result lossless and gives us a single canonical on-disk format
        // regardless of which clipboard payload the source app put there
        // (CF_DIBV5, CF_BITMAP, etc.).
        //
        // We pass the destination path as a single-quoted PowerShell literal.
        // PS-quote each ' by doubling it so filenames with apostrophes don't
        // break the script — same convention `write_clipboard_files` uses.
        let path_str = final_path.to_string_lossy().to_string();
        let ps_path = path_str.replace('\'', "''");
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$img = [System.Windows.Forms.Clipboard]::GetImage()
if ($img -eq $null) {{ Write-Output 'NO_IMAGE'; exit 0 }}
$img.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
$img.Dispose()
Write-Output 'OK'
"#,
            ps_path
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", &script])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("powershell launch failed: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("clipboard image read failed: {}", stderr.trim()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("NO_IMAGE") {
            return Ok(None);
        }
        if !final_path.exists() {
            // Defensive: PS reported OK but the file isn't there. Probably a
            // permissions / antivirus interception — surface a clear error
            // instead of silently returning Ok(None) (which the caller
            // interprets as "nothing was on the clipboard").
            return Err(format!(
                "clipboard image read reported OK but {} was not written",
                final_path.display()
            ));
        }
        return Ok(Some(final_path.to_string_lossy().into_owned()));
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: AppleScript can fetch PNG data off the pasteboard. Writing
        // the bytes via `write» a chunk of data at a time keeps the script
        // short. Returns empty string when there's no PNG on the clipboard.
        let path_str = final_path.to_string_lossy().to_string();
        // Escape backslashes and double quotes for safe inclusion in the
        // AppleScript string literal.
        let osa_path = path_str.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"
try
    set imgData to the clipboard as «class PNGf»
    set f to open for access POSIX file "{}" with write permission
    set eof of f to 0
    write imgData to f
    close access f
    return "OK"
on error
    try
        close access f
    end try
    return ""
end try
"#,
            osa_path
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("osascript launch failed: {}", e))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains("OK") {
            return Ok(None);
        }
        if !final_path.exists() {
            return Err(format!(
                "clipboard image read reported OK but {} was not written",
                final_path.display()
            ));
        }
        return Ok(Some(final_path.to_string_lossy().into_owned()));
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: xclip can stream the `image/png` clipboard MIME directly.
        // We pipe stdout straight into the destination file. `xclip -o`
        // exits non-zero when the requested target isn't available, which
        // we translate to "no image on clipboard" rather than an error.
        let path_str = final_path.to_string_lossy().to_string();
        let file = match std::fs::File::create(&path_str) {
            Ok(f) => f,
            Err(e) => return Err(format!("create dst file failed: {}", e)),
        };
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .stdout(std::process::Stdio::from(file))
            .output();
        match output {
            Ok(o) if o.status.success() => {
                // Empty file means xclip succeeded with zero bytes — treat
                // as "no image" and clean up the empty placeholder.
                if std::fs::metadata(&path_str).map(|m| m.len()).unwrap_or(0) == 0 {
                    let _ = std::fs::remove_file(&path_str);
                    return Ok(None);
                }
                return Ok(Some(path_str));
            }
            _ => {
                let _ = std::fs::remove_file(&path_str);
                return Ok(None);
            }
        }
    }
}

/// Pick `<dst_dir>/pasted-image.png` if free, otherwise `pasted-image-N.png`
/// with N counting up from 1. Used by both the OS-clipboard paste path and
/// the in-app base64 paste path so file managers and the agent chat agree
/// on a single naming convention.
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
    // Extreme fallback — should never happen in practice.
    dst_dir.join(format!("pasted-image-{}.png", uuid_like_suffix()))
}

/// Decode a base64 image payload (no data URL prefix) and write it under
/// `<dst_dir>/pasted-image[-N].png`, returning the absolute path. Mirrors
/// `paste_clipboard_image_into` so the in-app prompt-box paste path lands
/// on the same filenames as the OS-level explorer paste path. Auto-creates
/// `dst_dir` if it doesn't exist yet.
#[tauri::command]
pub async fn save_pasted_image_base64(
    dst_dir: String,
    data: String,
) -> Result<String, String> {
    use base64::Engine as _;
    let dst_root = Path::new(&dst_dir);
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
        return Err(format!(
            "Destination path exists but is not a directory: {}",
            dst_root.display()
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("invalid base64: {e}"))?;
    if bytes.len() as u64 > 100 * 1024 * 1024 {
        return Err("Refusing to write file larger than 100MB".to_string());
    }
    let final_path = unique_pasted_image_path(dst_root);
    std::fs::write(&final_path, &bytes).map_err(|e| e.to_string())?;
    Ok(final_path.to_string_lossy().into_owned())
}

/// Local-time `YYYYMMDD-HHMMSS` stamp for default paste filenames. Local
/// (not UTC) so the timestamp matches what the user sees on their wall
/// clock when they paste — surprising filenames are worse than wrong
/// timezones in this UI.
#[allow(dead_code)]
fn local_timestamp_compact() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // No chrono dependency just for this — derive a date from secs-since-epoch
    // adjusted for local offset. Falls back to the raw nanos if anything in
    // the chain fails (which would just mean an unusual but unique filename).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Local offset from the platform. On unsupported platforms we degrade to
    // UTC, which is fine for ensuring uniqueness; the filename's just a hint.
    let offset_secs = local_offset_seconds();
    let local = now + offset_secs;
    let (y, mo, d, h, mi, s) = civil_from_unix_seconds(local);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, mo, d, h, mi, s)
}

#[cfg(target_os = "windows")]
fn local_offset_seconds() -> i64 {
    // GetTimeZoneInformation gives bias in minutes; positive means *behind*
    // UTC (per Win32 convention), so negate.
    unsafe {
        #[repr(C)]
        struct TimeZoneInformation {
            bias: i32,
            standard_name: [u16; 32],
            standard_date: [u8; 16],
            standard_bias: i32,
            daylight_name: [u16; 32],
            daylight_date: [u8; 16],
            daylight_bias: i32,
        }
        extern "system" {
            fn GetTimeZoneInformation(tzi: *mut TimeZoneInformation) -> u32;
        }
        let mut tzi: TimeZoneInformation = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tzi);
        let extra = match r {
            2 => tzi.daylight_bias, // TIME_ZONE_ID_DAYLIGHT
            _ => tzi.standard_bias,
        };
        -((tzi.bias + extra) as i64) * 60
    }
}

#[cfg(not(target_os = "windows"))]
fn local_offset_seconds() -> i64 {
    // POSIX: use libc::localtime_r's tm_gmtoff. We avoid a libc dependency by
    // shelling out only if needed — but for simplicity, fall back to 0 (UTC).
    // The filename is decorative; uniqueness is what matters, and we already
    // have the collision-suffix loop in unique_destination as the safety net.
    0
}

/// Civil date components (year, month, day, hour, minute, second) from a
/// Unix-epoch second count. Public-domain Howard Hinnant algorithm —
/// avoids pulling chrono in just to format a filename.
fn civil_from_unix_seconds(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let z = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let hour = (sod / 3600) as u32;
    let minute = ((sod % 3600) / 60) as u32;
    let second = (sod % 60) as u32;
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hour, minute, second)
}

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

/// Move a file or directory into `dst_dir`, preserving its name.
///
/// Tries an atomic `std::fs::rename` first (instant on same filesystem).
/// Falls back to copy + delete when the source and destination are on
/// different drives/filesystems so cross-device moves work transparently.
/// Collision avoidance uses the same `(1)`, `(2)` … suffix scheme as
/// `copy_entry` so a paste never silently overwrites an existing entry.
#[tauri::command]
pub async fn move_entry(src_path: String, dst_dir: String) -> Result<String, String> {
    let src = Path::new(&src_path);
    validate_writable_path(src)?;
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

    if src.is_dir() {
        let src_can = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        let dst_can =
            std::fs::canonicalize(dst_root).unwrap_or_else(|_| dst_root.to_path_buf());
        if dst_can.starts_with(&src_can) {
            return Err("Cannot move a folder into itself".to_string());
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

        // IMPORTANT: build the command line with `raw_arg` rather than `arg`.
        // `arg` quotes the WHOLE argument, so `/select,C:\path with space` became
        // `"/select,C:\path with space"` on the wire — explorer.exe couldn't
        // parse the `/select,` switch when it was inside the quotes and silently
        // fell back to opening the default location (the user's Documents
        // folder). With `raw_arg` we keep `/select,` outside the quotes and quote
        // only the path (which we've already validated cannot contain a `"`),
        // so explorer reliably selects the target instead of opening Documents.
        use std::os::windows::process::CommandExt;
        let spawn_result = if canon.is_dir() {
            std::process::Command::new("explorer.exe")
                .raw_arg(format!("\"{}\"", final_path))
                .spawn()
        } else {
            std::process::Command::new("explorer.exe")
                .raw_arg(format!("/select,\"{}\"", final_path))
                .spawn()
        };
        spawn_result.map_err(|e| e.to_string())?;
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
