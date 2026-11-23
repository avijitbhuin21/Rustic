//! Keeps the main window on the app's own pages.
//!
//! Rendered content (agent output, a README, a notebook) is sanitised, and the
//! CSP sets `form-action 'none'`, but neither stops every route to a top-level
//! navigation. A page that replaced the app would be denied all commands by the
//! ACL (see `build.rs`); this guard stops it from loading at all.
//!
//! The webview reports navigations of frames as well as of the window and does
//! not say which, so the rule has to admit everything a frame legitimately
//! loads: the app's own schemes and loopback HTTP. Network hosts are refused.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Runtime, Url};

const MAIN_WINDOW: &str = "main";

/// Schemes served by the app itself or that never leave the process.
const LOCAL_SCHEMES: &[&str] = &[
    "tauri",
    "about",
    "data",
    "blob",
    "asset",
    "ipc",
    rustic_app::film_preview::SCHEME,
    crate::media_protocol::SCHEME,
];

/// Whether `url` is a page of the app itself, a local scheme, or loopback HTTP.
pub fn is_app_navigation(url: &Url) -> bool {
    let scheme = url.scheme();
    if LOCAL_SCHEMES.contains(&scheme) {
        return true;
    }
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // Windows serves the app and its custom protocols from `<name>.localhost`.
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Plugin that cancels navigations of the main window to anything but the app.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            // Remote-backend windows have their own handler and no commands.
            if webview.label() != MAIN_WINDOW || is_app_navigation(url) {
                return true;
            }
            tracing::warn!(
                scheme = url.scheme(),
                host = url.host_str().unwrap_or(""),
                "blocked a navigation of the main window away from the app"
            );
            false
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::is_app_navigation;
    use tauri::Url;

    fn allowed(url: &str) -> bool {
        is_app_navigation(&Url::parse(url).expect("test URL parses"))
    }

    #[test]
    fn app_pages_and_local_schemes_are_allowed() {
        for url in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/index.html",
            "https://tauri.localhost/",
            "http://localhost:1420/",
            "http://127.0.0.1:9222/devtools/inspector.html",
            "http://[::1]:9222/",
            "http://rustic-film.localhost/abc/shell.html",
            "rustic-film://abc/shell.html",
            "rustic-media://localhost/file",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hello",
            "blob:tauri://localhost/1234",
        ] {
            assert!(allowed(url), "{url} should be allowed");
        }
    }

    #[test]
    fn network_and_file_targets_are_refused() {
        for url in [
            "https://example.com/",
            "http://example.com/login",
            "https://localhost.example.com/",
            "https://evil.com/?x=.localhost",
            "http://localhost@evil.com/",
            "https://192.168.1.10/",
            "http://10.0.0.1:8080/",
            "file:///etc/passwd",
            "ftp://example.com/",
            "javascript:alert(1)",
        ] {
            assert!(!allowed(url), "{url} should be refused");
        }
    }
}
