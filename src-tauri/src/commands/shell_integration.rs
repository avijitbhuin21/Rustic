//! Windows Explorer "Open with Rustic" context-menu integration.
//!
//! Registers per-user (HKCU — no admin needed) shell verbs for directories,
//! the directory background, and files, mirroring what VS Code's installer
//! does. Uses `reg.exe` so we don't pull in a registry crate.

/// Registry subkeys that receive the "Open with Rustic" verb, paired with the
/// argument placeholder Explorer substitutes (%1 = clicked item, %V = folder
/// background's own path).
#[cfg(target_os = "windows")]
const SHELL_KEYS: &[(&str, &str)] = &[
    (r"Software\Classes\Directory\shell\Rustic", "%1"),
    (r"Software\Classes\Directory\Background\shell\Rustic", "%V"),
    (r"Software\Classes\*\shell\Rustic", "%1"),
];

#[cfg(target_os = "windows")]
fn run_reg(args: &[&str]) -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let out = std::process::Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed to run reg.exe: {e}"))?;
    Ok(out.status.success())
}

/// Enable or disable the Explorer context-menu entry for the current user.
#[tauri::command]
pub fn set_open_with_rustic(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot resolve Rustic executable path: {e}"))?;
        let exe = exe.to_string_lossy().to_string();
        for (key, arg) in SHELL_KEYS {
            let root = format!(r"HKCU\{key}");
            let cmd_key = format!(r"{root}\command");
            if enabled {
                let ok1 = run_reg(&["add", &root, "/ve", "/d", "Open with Rustic", "/f"])?;
                let _ = run_reg(&["add", &root, "/v", "Icon", "/d", &exe, "/f"])?;
                let cmd = format!("\"{exe}\" \"{arg}\"");
                let ok2 = run_reg(&["add", &cmd_key, "/ve", "/d", &cmd, "/f"])?;
                if !(ok1 && ok2) {
                    return Err(format!("reg.exe failed writing {root}"));
                }
            } else {
                // Deleting a missing key fails — that's fine for disable.
                let _ = run_reg(&["delete", &root, "/f"])?;
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Err("Explorer integration is only available on Windows".to_string())
    }
}

/// True when the context-menu entry is currently registered.
#[tauri::command]
pub fn get_open_with_rustic() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        run_reg(&["query", r"HKCU\Software\Classes\Directory\shell\Rustic"])
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(false)
    }
}

/// Path argument this instance was launched with ("Open with Rustic" passes
/// the clicked file/folder as the first positional arg). None in normal runs.
#[tauri::command]
pub fn get_startup_path() -> Option<serde_json::Value> {
    let p = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-') && std::path::Path::new(a).exists())?;
    let is_dir = std::path::Path::new(&p).is_dir();
    Some(serde_json::json!({ "path": p, "is_dir": is_dir }))
}
