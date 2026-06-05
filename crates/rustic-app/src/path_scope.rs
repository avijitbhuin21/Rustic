use std::path::{Path, PathBuf};

/// Reject writes to system directories and sensitive home subdirectories
/// (SSH keys, credentials, browser profiles, etc.).
pub fn validate_writable_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("Empty path".to_string());
    }

    let canon = canonicalize_existing_prefix(path)
        .ok_or_else(|| format!("Cannot resolve path: {}", path.display()))?;

    if path_is_in_system_root(&canon) {
        return Err(format!(
            "Refusing to modify system path: {}",
            canon.display()
        ));
    }

    if path_is_sensitive_home_subpath(&canon) {
        return Err(format!(
            "Refusing to modify sensitive path: {}",
            canon.display()
        ));
    }

    Ok(())
}

/// Reject reads of system/sensitive paths to prevent exfiltration through IPC.
pub fn validate_readable_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("Empty path".to_string());
    }

    let canon = canonicalize_existing_prefix(path)
        .ok_or_else(|| format!("Cannot resolve path: {}", path.display()))?;

    if path_is_in_system_root(&canon) {
        return Err(format!(
            "Refusing to read system path: {}",
            canon.display()
        ));
    }

    if path_is_sensitive_home_subpath(&canon) {
        return Err(format!(
            "Refusing to read sensitive path: {}",
            canon.display()
        ));
    }

    Ok(())
}

/// Validate a name that will be joined into a filesystem path. Rejects
/// empty values, separators, `..`, control chars, and quoting metacharacters.
pub fn validate_simple_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Empty name".to_string());
    }
    if name == "." || name == ".." {
        return Err("Invalid name".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("Name must not contain path separators".to_string());
    }
    if name.contains("..") {
        return Err("Name must not contain '..'".to_string());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("Name must not contain control characters".to_string());
    }
    if name.starts_with(' ') || name.ends_with(' ') || name.ends_with('.') {
        return Err("Name must not start/end with whitespace or end with '.'".to_string());
    }
    Ok(())
}

/// Canonicalize as much of `path` as exists, then re-attach any leftover
/// non-existent components. This lets us validate paths to files we are about
/// to create.
fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut probe = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = probe.canonicalize() {
            // Re-attach the tail segments that did not exist.
            let mut full = c;
            for seg in tail.iter().rev() {
                full.push(seg);
            }
            return Some(full);
        }
        let popped = match probe.file_name().map(|s| s.to_os_string()) {
            Some(s) => s,
            None => return None,
        };
        tail.push(popped);
        if !probe.pop() {
            return None;
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// Returns true if `canon` is an absolute path inside one of the well-known
/// sensitive subdirectories of the user's home directory. Comparisons are
/// case-insensitive — Windows users can have mixed-case `C:\Users\...`, and
/// macOS HFS+ is case-insensitive by default.
fn path_is_sensitive_home_subpath(canon: &Path) -> bool {
    let home = match home_dir().and_then(|h| h.canonicalize().ok()) {
        Some(h) => h,
        None => return false,
    };
    let canon_lc = canon.to_string_lossy().to_lowercase();
    let home_lc = home.to_string_lossy().to_lowercase();
    if !canon_lc.starts_with(&home_lc) {
        return false;
    }
    let rest = canon_lc[home_lc.len()..].trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        return false;
    }
    // Each entry is matched as either an exact relative path or a path-prefix
    // (separator-aware) so e.g. ".ssh" matches ".ssh/id_rsa" but not ".ssh-old".
    const SENSITIVE: &[&str] = &[
        // Cross-platform creds / keys
        ".ssh",
        ".aws",
        ".gnupg",
        ".gpg",
        ".azure",
        ".kube",
        ".config/gcloud",
        ".config/google-chrome",
        ".config/chromium",
        ".config/microsoft-edge",
        ".config/brave",
        ".mozilla/firefox",
        ".docker/config.json",
        ".npmrc",
        ".pypirc",
        ".netrc",
        ".password-store",
        ".bitcoin",
        ".electrum",
        ".ethereum",
        // Windows browser profiles + creds
        "appdata/local/google/chrome/user data",
        "appdata/local/microsoft/edge/user data",
        "appdata/local/brave software",
        "appdata/local/chromium/user data",
        "appdata/roaming/mozilla/firefox",
        "appdata/roaming/microsoft/credentials",
        "appdata/roaming/microsoft/protect",
        // macOS browser profiles + keychains
        "library/keychains",
        "library/application support/google/chrome",
        "library/application support/firefox",
        "library/application support/microsoft edge",
        "library/application support/brave-browser",
        "library/cookies",
    ];
    for s in SENSITIVE {
        let s_lc = s.to_lowercase();
        if rest == s_lc
            || rest.starts_with(&format!("{}/", s_lc))
            || rest.starts_with(&format!("{}\\", s_lc))
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn path_is_in_system_root(path: &Path) -> bool {
    let lc = path.to_string_lossy().to_ascii_lowercase();
    // Match common system roots regardless of drive letter via heuristic
    // patterns. Splits on the path separator then checks the suffix structure.
    const BANNED_SUFFIXES: &[&str] = &[
        ":\\windows",
        ":/windows",
        ":\\program files",
        ":/program files",
        ":\\program files (x86)",
        ":/program files (x86)",
        ":\\programdata",
        ":/programdata",
    ];
    for needle in BANNED_SUFFIXES {
        if lc.contains(needle) {
            // Make sure the match is a real path component start and not a
            // suffix of something like "C:\WindowsApps" (which is also system
            // but we want to be inclusive). The `:`/path-sep prefix makes it
            // start-of-path-in-drive — close enough.
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn path_is_in_system_root(path: &Path) -> bool {
    const BANNED: &[&str] = &[
        "/etc", "/sys", "/proc", "/boot", "/dev", "/var/log",
        "/usr/bin", "/usr/sbin", "/usr/local/bin", "/usr/local/sbin",
        "/bin", "/sbin",
        // macOS
        "/System", "/private/etc", "/private/var",
    ];
    let s = path.to_string_lossy();
    BANNED.iter().any(|root| {
        s == *root || s.starts_with(&format!("{}/", root))
    })
}

#[cfg(not(any(target_os = "windows", unix)))]
fn path_is_in_system_root(_path: &Path) -> bool {
    false
}
