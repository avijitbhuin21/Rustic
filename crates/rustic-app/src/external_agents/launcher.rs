//! Which copy of a CLI agent Rustic actually runs.
//!
//! Resolving a bare program name against `PATH` and taking the first hit is not
//! good enough for these tools, because they update *themselves*. A CLI that was
//! installed twice (say an old npm global shim plus a fresh `bun install -g`)
//! self-updates whichever copy its own installer owns, then exits asking to be
//! restarted — and if the stale copy still comes first on `PATH`, the next
//! launch is the old version again and it asks to update forever.
//!
//! So Rustic enumerates *every* copy on `PATH`, asks each one its version, and
//! launches the newest. `PATH` order only breaks ties. Probes are cached against
//! the file's mtime and size, which is what makes an in-terminal update take
//! effect on the very next launch: updating rewrites the binary, the cache key
//! changes, and the new version is picked up without restarting Rustic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Executable extensions worth trying on Windows, most preferred first.
///
/// A real `.exe` beats a shim, and a shim `cmd.exe` can run beats a `.ps1` that
/// needs an execution-policy waiver. npm also drops an extensionless POSIX shell
/// script next to its shims; Windows can't execute it, so it is never a
/// candidate there.
#[cfg(windows)]
const WINDOWS_EXTS: [&str; 5] = ["exe", "com", "cmd", "bat", "ps1"];

/// How long a `--version` probe may take before it is killed and treated as
/// unknown. Generous because these are Node-backed shims on a cold cache.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_POLL: Duration = Duration::from_millis(50);

/// One installed copy of a CLI agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub path: PathBuf,
    /// `major.minor.patch` as the tool reported it, when it could be probed.
    pub version: Option<String>,
}

/// The copy Rustic will launch, plus the ones it takes precedence over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLauncher {
    pub path: PathBuf,
    pub version: Option<String>,
    /// Other copies found on `PATH`, in `PATH` order. Non-empty means the
    /// machine has a duplicate install — worth telling the user about, since it
    /// is the usual cause of a CLI that keeps asking to update.
    pub shadowed: Vec<Install>,
}

/// Resolve `program` to the newest copy installed on `PATH`.
///
/// Returns `None` when the program isn't installed at all.
pub fn resolve(program: &str) -> Option<ResolvedLauncher> {
    let mut installs: Vec<Install> = candidates(program)
        .into_iter()
        .map(|path| {
            let version = cached_version(&path);
            Install { path, version }
        })
        .collect();
    if installs.is_empty() {
        return None;
    }

    let winner = installs.remove(pick_best(&installs)?);
    Some(ResolvedLauncher {
        path: winner.path,
        version: winner.version,
        shadowed: installs,
    })
}

/// Index of the copy that should be launched.
///
/// Highest version wins; `Reverse(index)` breaks ties toward the earlier `PATH`
/// entry, which also means a set nothing could be probed in resolves exactly the
/// way the shell would have.
fn pick_best(installs: &[Install]) -> Option<usize> {
    installs
        .iter()
        .enumerate()
        .max_by_key(|(index, install)| {
            let parsed = install.version.as_deref().and_then(parse_version);
            (parsed, std::cmp::Reverse(*index))
        })
        .map(|(index, _)| index)
}

/// Every executable copy of `program` on `PATH`, at most one per directory.
///
/// One per directory matters on Windows, where a single npm install lays down
/// `foo`, `foo.cmd` and `foo.ps1` side by side — three files, one install.
pub fn candidates(program: &str) -> Vec<PathBuf> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen_dirs: Vec<PathBuf> = Vec::new();

    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() || seen_dirs.contains(&dir) {
            continue;
        }
        if let Some(hit) = best_in_dir(&dir, program) {
            seen_dirs.push(dir);
            found.push(hit);
        }
    }
    found
}

#[cfg(windows)]
fn best_in_dir(dir: &Path, program: &str) -> Option<PathBuf> {
    // Honour PATHEXT when it is set, but always in our own preference order:
    // the point is to pick the most directly executable file, not the first one
    // the shell would have tried.
    let pathext = std::env::var("PATHEXT").unwrap_or_default().to_lowercase();
    let allowed: Vec<&str> = if pathext.is_empty() {
        WINDOWS_EXTS.to_vec()
    } else {
        WINDOWS_EXTS
            .into_iter()
            .filter(|ext| {
                pathext
                    .split(';')
                    .any(|p| p.trim_start_matches('.') == *ext)
            })
            .collect()
    };
    allowed
        .into_iter()
        .map(|ext| dir.join(format!("{program}.{ext}")))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(windows))]
fn best_in_dir(dir: &Path, program: &str) -> Option<PathBuf> {
    let candidate = dir.join(program);
    candidate.is_file().then_some(candidate)
}

/// Version of the binary at `path`, remembered until the file changes.
fn cached_version(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = cache_key(path);
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
    }

    let probed = probe_version(path);
    if let Ok(mut map) = cache.lock() {
        map.insert(key, probed.clone());
    }
    probed
}

/// Identity of a file for caching: a self-update rewrites the binary, so mtime
/// and length together are enough to notice and re-probe.
type CacheKey = (PathBuf, Option<SystemTime>, u64);

fn cache_key(path: &Path) -> CacheKey {
    let meta = path.metadata().ok();
    (
        path.to_path_buf(),
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.as_ref().map(|m| m.len()).unwrap_or(0),
    )
}

/// Ask a launcher its version, killing it if it hangs.
fn probe_version(path: &Path) -> Option<String> {
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::debug!(
                target: "rustic::external_agents",
                path = %path.display(),
                "version probe could not start: {e}"
            );
            return None;
        }
    };

    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                tracing::warn!(
                    target: "rustic::external_agents",
                    path = %path.display(),
                    "version probe timed out"
                );
                return None;
            }
            Ok(None) => std::thread::sleep(PROBE_POLL),
            Err(_) => return None,
        }
    }

    let out = child.wait_with_output().ok()?;
    // Some CLIs print the version banner on stderr.
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    extract_version(&text)
}

/// First `major.minor[.patch]` looking token in a `--version` banner.
///
/// Has to cope with `codex-cli 0.125.0`, a bare `0.146.0`, and
/// `2.1.4 (Claude Code)`.
pub fn extract_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let token = text[start..i].trim_end_matches('.');
        if parse_version(token).is_some() {
            return Some(token.to_string());
        }
    }
    None
}

/// Order a version string. Needs at least `major.minor` so a stray year or
/// exit code can't pass for a version.
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse::<u64>().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Is `latest` newer than `installed`? Unparseable input is never "newer", so a
/// version Rustic can't read never nags the user.
pub fn is_newer(latest: &str, installed: &str) -> bool {
    match (parse_version(latest), parse_version(installed)) {
        (Some(l), Some(i)) => l > i,
        _ => false,
    }
}

/// How long a registry answer (or a failure) is reused before asking again.
const REGISTRY_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(5);

/// A registry answer and when it arrived. `None` is a cached failure.
type RegistryCache = HashMap<String, (Instant, Option<String>)>;

/// Newest version `package` has published to the npm registry.
///
/// Cached with a long TTL — including negative results, so an offline machine
/// makes one failed request every few hours rather than one per detection.
pub async fn latest_published_version(package: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<RegistryCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some((fetched, version)) = map.get(package) {
            if fetched.elapsed() < REGISTRY_TTL {
                return version.clone();
            }
        }
    }

    let fetched = fetch_latest(package).await;
    if let Ok(mut map) = cache.lock() {
        map.insert(package.to_string(), (Instant::now(), fetched.clone()));
    }
    fetched
}

async fn fetch_latest(package: &str) -> Option<String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REGISTRY_TIMEOUT)
            .user_agent("rustic-ide")
            .build()
            .unwrap_or_default()
    });

    let url = format!("https://registry.npmjs.org/{package}/latest");
    let body: serde_json::Value = match client.get(&url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => response.json().await.ok()?,
            Err(e) => {
                tracing::debug!(target: "rustic::external_agents", "{package}: registry said {e}");
                return None;
            }
        },
        Err(e) => {
            tracing::debug!(target: "rustic::external_agents", "{package}: registry unreachable: {e}");
            return None;
        }
    };
    body.get("version")
        .and_then(|v| v.as_str())
        .and_then(extract_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_versions_from_real_banners() {
        assert_eq!(extract_version("codex-cli 0.125.0"), Some("0.125.0".into()));
        assert_eq!(extract_version("0.146.0\n"), Some("0.146.0".into()));
        assert_eq!(extract_version("2.1.4 (Claude Code)"), Some("2.1.4".into()));
        assert_eq!(extract_version("v1.2.3"), Some("1.2.3".into()));
        assert_eq!(extract_version("no version here"), None);
        // A lone integer is not a version.
        assert_eq!(extract_version("built in 2026"), None);
    }

    #[test]
    fn orders_versions_numerically_not_lexically() {
        assert!(is_newer("0.146.0", "0.125.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.125.0", "0.146.0"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        // Unreadable versions never claim to be newer.
        assert!(!is_newer("weird", "1.0.0"));
        assert!(!is_newer("2.0.0", "weird"));
    }

    #[test]
    fn parse_rejects_non_versions() {
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
    }

    fn installs(entries: &[(&str, Option<&str>)]) -> Vec<Install> {
        entries
            .iter()
            .map(|(path, version)| Install {
                path: PathBuf::from(path),
                version: version.map(String::from),
            })
            .collect()
    }

    #[test]
    fn the_newest_copy_wins_over_path_order() {
        // `/npm` is earlier on PATH but older — the whole point of resolving.
        let found = installs(&[
            ("/npm/codex", Some("0.125.0")),
            ("/bun/codex", Some("0.146.0")),
        ]);
        assert_eq!(pick_best(&found), Some(1));
    }

    #[test]
    fn equal_versions_keep_path_order() {
        let found = installs(&[
            ("/first/codex", Some("1.2.3")),
            ("/second/codex", Some("1.2.3")),
        ]);
        assert_eq!(pick_best(&found), Some(0));
    }

    #[test]
    fn unknown_versions_fall_back_to_path_order() {
        let found = installs(&[("/first/codex", None), ("/second/codex", None)]);
        assert_eq!(pick_best(&found), Some(0));
    }

    #[test]
    fn a_probed_version_beats_an_unprobed_one() {
        let found = installs(&[("/first/codex", None), ("/second/codex", Some("0.1.0"))]);
        assert_eq!(pick_best(&found), Some(1));
    }

    #[test]
    fn nothing_installed_resolves_to_nothing() {
        assert_eq!(pick_best(&[]), None);
        assert!(candidates("definitely-not-installed-xyzzy").is_empty());
        assert!(resolve("definitely-not-installed-xyzzy").is_none());
    }
}
