// Swap the window/taskbar icon to match the OS taskbar theme.
//
// The bundled icon is the white-R variant (good on the default dark Windows
// taskbar). When the user is on a light Windows taskbar the white-R blends
// into the background and looks invisible, so we detect that at startup and
// `set_icon()` to the black-R variant. The window icon is what shows in the
// taskbar AND Task Manager, so one swap covers both surfaces.
//
// We don't subscribe to live theme-change events — a single check at app
// startup is enough for the common case. If the user changes their taskbar
// theme mid-session they can relaunch Rustic to pick it up.

#[cfg(target_os = "windows")]
pub fn apply(window: &tauri::WebviewWindow) {
    if !windows_uses_light_taskbar() {
        return;
    }
    match tauri::image::Image::from_bytes(include_bytes!("../icons/icon-light.png")) {
        Ok(img) => {
            if let Err(e) = window.set_icon(img) {
                tracing::warn!(error = %e, "[app_icon] set_icon failed");
            } else {
                tracing::info!("[app_icon] applied light-theme variant for light taskbar");
            }
        }
        Err(e) => tracing::warn!(error = %e, "[app_icon] decode of icon-light.png failed"),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply(_window: &tauri::WebviewWindow) {
    // On macOS the dock icon is the bundled .icns; on Linux it's WM-dependent.
    // We could plumb dark/light there too later, but the bundled white-R icon
    // is fine for both common defaults.
}

#[cfg(target_os = "windows")]
fn windows_uses_light_taskbar() -> bool {
    // Read HKCU\…\Personalize\SystemUsesLightTheme — 0 = dark taskbar (default),
    // 1 = light taskbar. We use `reg query` instead of pulling in a winreg crate
    // dep; this runs once at startup so the process-spawn cost is negligible.
    use std::process::Command;
    let output = Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "SystemUsesLightTheme",
        ])
        .output();
    match output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            // Output format: "    SystemUsesLightTheme    REG_DWORD    0x1"
            s.contains("0x1")
        }
        Err(_) => false,
    }
}
