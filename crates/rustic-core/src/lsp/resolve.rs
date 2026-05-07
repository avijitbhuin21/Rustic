use std::env;
use std::path::{Path, PathBuf};

/// Resolve an LSP server command to an absolute executable path.
///
/// In dev (cargo tauri dev), the process inherits the launching shell's full
/// PATH and bare commands like `rust-analyzer` resolve fine. The bundled NSIS
/// app is launched from Explorer / the Start Menu and inherits only the user
/// + system environment registry — which often lacks the locations where
/// language servers live (rustup's `~/.cargo/bin`, npm globals, project-local
/// `node_modules/.bin`). Without this fallback, `Command::spawn` errors
/// "program not found" and the LSP feature appears broken in installed builds.
///
/// Resolution order:
///   1. Already absolute and exists → use as-is.
///   2. `<project_root>/node_modules/.bin/<cmd>` walking up to filesystem root.
///   3. Each entry in `$PATH` (with the OS-specific separator).
///   4. Common per-user toolchain dirs (`.cargo/bin`, npm global, etc.).
pub fn resolve_command(command: &str, project_root: Option<&Path>) -> Option<PathBuf> {
    let p = PathBuf::from(command);
    if p.is_absolute() {
        return if p.exists() { Some(p) } else { None };
    }

    // Walk up from project root looking for node_modules/.bin/<cmd>
    if let Some(root) = project_root {
        let mut cur = root.to_path_buf();
        loop {
            for variant in command_variants(command) {
                let candidate = cur.join("node_modules").join(".bin").join(&variant);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            if !cur.pop() {
                break;
            }
        }
    }

    // PATH search
    if let Ok(path_var) = env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            if dir.is_empty() {
                continue;
            }
            for variant in command_variants(command) {
                let candidate = PathBuf::from(dir).join(&variant);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    // Common per-user install locations not always present in PATH on a
    // GUI-launched process.
    for dir in common_install_dirs() {
        for variant in command_variants(command) {
            let candidate = dir.join(&variant);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn command_variants(command: &str) -> Vec<String> {
    if cfg!(windows) {
        let lower = command.to_lowercase();
        if lower.ends_with(".exe") || lower.ends_with(".cmd") || lower.ends_with(".bat") {
            return vec![command.to_string()];
        }
        // Order matters: prefer .exe over .cmd because npm-installed JS tools
        // ship both (.cmd is a shim that re-launches node — slower and adds
        // a console window flash on Windows).
        vec![
            format!("{}.exe", command),
            format!("{}.cmd", command),
            format!("{}.bat", command),
            command.to_string(),
        ]
    } else {
        vec![command.to_string()]
    }
}

fn common_install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join("go").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        dirs.push(home.join(".local").join("bin"));

        #[cfg(windows)]
        {
            if let Ok(appdata) = env::var("APPDATA") {
                // npm installs `<cmd>.cmd` shims directly into %APPDATA%\npm
                dirs.push(PathBuf::from(appdata).join("npm"));
            }
            if let Ok(localappdata) = env::var("LOCALAPPDATA") {
                // pnpm global / Volta shim location
                dirs.push(PathBuf::from(&localappdata).join("Volta").join("bin"));
                dirs.push(PathBuf::from(&localappdata).join("pnpm"));
            }
        }
        #[cfg(not(windows))]
        {
            dirs.push(PathBuf::from("/usr/local/bin"));
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/bin"));
        }
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME").map(PathBuf::from)
    }
}
