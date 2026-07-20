//! Cloud sync: full-environment push/pull between a local desktop app and a
//! deployed rustic-server.
//!
//! One archive (`.tar.zst` — zstd-compressed tar; gzip archives from older
//! builds are still readable) carries the whole environment: a consistent
//! SQLite snapshot (`VACUUM INTO`), the file-history blob store, an exported
//! secrets map, and every non-archived project's files (minus heavy
//! build/dependency folders — see [`SYNC_EXCLUDED_DIRS`]).
//!
//! Semantics are deliberately destructive-replace, no merging:
//! * push  = the receiving side's environment becomes a copy of the sender's.
//! * pull  = the local environment becomes a copy of the cloud's.
//!
//! Import happens **in-process** on both transports: the live DB connection is
//! swapped under its mutex (via a temporary in-memory handle so the file can
//! be replaced on Windows), watchers are stopped and restarted, and project
//! root paths are rewritten for the receiving machine.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::bootstrap::{self, GIT_TOKEN_ACCOUNT};
use crate::context::EventEmitter;
use crate::secrets::{provider_account, SecretStore};
use crate::state::AppState;
use crate::sync_ext::MutexExt;
use rustic_db::Database;

/// Directory names never carried by a sync archive, at any depth. This is the
/// heavy build-artifact / dependency subset of `file_tree::EXCLUDED_DIRS`:
/// unlike the agent's tree view, sync DOES include `.git` (full repo state
/// travels) and `.rustic` (project memory travels).
pub const SYNC_EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    ".nuxt",
    ".cache",
    ".turbo",
    ".parcel-cache",
    "coverage",
];

/// Bump when the archive layout changes incompatibly.
/// v2: per-project `files_skipped` (incremental sync) + top-level `sync_id`.
pub const SYNC_MANIFEST_VERSION: u32 = 2;

/// Staging directory (under the data dir) used while applying an archive.
const STAGING_DIR: &str = "sync-staging";

/// Sidecar file (under the data dir, NOT inside the archive — it must survive
/// the DB swap) remembering each project's fingerprint at the last successful
/// sync. Both sides hold one; matching `sync_id`s + clean fingerprints on both
/// ends prove a project's content is already identical and its files can be
/// skipped.
const SYNC_STATE_FILE: &str = "sync-state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProjectEntry {
    pub id: String,
    pub name: String,
    /// Directory of this project's files inside the archive (`projects/<id>`).
    pub dir: String,
    /// The project's absolute root path on the machine that built the archive.
    pub origin_root_path: String,
    /// True when this project's files are NOT in the archive because both
    /// sides already hold identical content (incremental sync). The receiver
    /// must leave the project's on-disk files untouched.
    #[serde(default)]
    pub files_skipped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncManifest {
    pub version: u32,
    pub created_at: String,
    /// `windows` / `linux` / `macos` — the OS that built the archive.
    pub origin_os: String,
    /// Random id for this sync generation. Both sides record it per project;
    /// a later sync may skip a project only when the ids still match.
    #[serde(default)]
    pub sync_id: String,
    pub projects: Vec<SyncProjectEntry>,
}

/// Per-project fingerprint stored in the sync-state sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProjectState {
    pub hash: String,
    pub sync_id: String,
}

/// The sync-state sidecar: project id → fingerprint at last successful sync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub projects: HashMap<String, SyncProjectState>,
}

/// What one side tells the other about a project before a sync, so the sender
/// can decide which project trees to skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerProjectState {
    pub id: String,
    /// The sync generation this side recorded for the project.
    pub sync_id: String,
    /// True when the project's current on-disk fingerprint still matches the
    /// recorded one (nothing changed here since that sync).
    pub clean: bool,
}

/// Decides where an imported project's files land on the receiving machine.
/// Receives the manifest entry plus the root path this machine's *previous*
/// DB had for the same project id (if any).
pub type ProjectRootResolver<'a> =
    &'a (dyn Fn(&SyncProjectEntry, Option<&str>) -> PathBuf + Send + Sync);

/// Refuse to sync while any agent task is mid-turn — a running executor holds
/// live references (DB writes, file locks, terminals) that a swap would strand.
fn ensure_no_running_tasks(state: &AppState) -> Result<(), String> {
    use rustic_agent::TaskStatus;
    let agent = state.agent.lock_safe();
    let busy = agent.tasks.values().any(|t| {
        matches!(
            t.info.status,
            TaskStatus::Preparing | TaskStatus::Running | TaskStatus::WaitingOnSubagents
        )
    });
    if busy {
        return Err(
            "An agent task is currently running. Wait for it to finish (or stop it) before syncing.".into(),
        );
    }
    Ok(())
}

fn os_name() -> String {
    std::env::consts::OS.to_string()
}

/// Fast per-project fingerprint: SHA-256 over the sorted list of
/// `(relative_path, size, mtime_ns)` for every synced file (same exclusions as
/// the archive walk). Metadata-only — no file contents are read — so it is
/// only comparable against hashes computed on the SAME machine; the sync
/// protocol never compares hashes across machines, only "did MY side change
/// since the sync generation both sides share?".
pub fn project_tree_hash(root: &Path) -> String {
    use sha2::{Digest, Sha256};

    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64, i128)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if SYNC_EXCLUDED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                walk(&path, root, out);
            } else if meta.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| name.clone());
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i128)
                    .unwrap_or(-1);
                out.push((rel, meta.len(), mtime));
            }
        }
    }

    if !root.is_dir() {
        return "missing".to_string();
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for (rel, size, mtime) in files {
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(size.to_le_bytes());
        hasher.update(mtime.to_le_bytes());
    }
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Load the sync-state sidecar (missing/corrupt → empty state, which simply
/// disables skipping until the next successful sync).
pub fn load_sync_state(data_dir: &Path) -> SyncState {
    std::fs::read(data_dir.join(SYNC_STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_sync_state(data_dir: &Path, state: &SyncState) {
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let path = data_dir.join(SYNC_STATE_FILE);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Recompute fingerprints for the given project roots and persist them under
/// `sync_id`. Called at the end of a successful build (sender) and apply
/// (receiver) so both sides remember the shared sync generation.
fn record_sync_state(data_dir: &Path, sync_id: &str, roots: &[(String, PathBuf)]) {
    let mut state = load_sync_state(data_dir);
    state.projects.clear();
    for (id, root) in roots {
        state.projects.insert(
            id.clone(),
            SyncProjectState {
                hash: project_tree_hash(root),
                sync_id: sync_id.to_string(),
            },
        );
    }
    save_sync_state(data_dir, &state);
}

/// Answer "what do you hold?" for the other side of a sync: every DB project's
/// recorded sync generation plus whether its files are still untouched since.
pub fn compute_peer_state(state: &AppState, data_dir: &Path) -> Vec<PeerProjectState> {
    let sync_state = load_sync_state(data_dir);
    let projects = {
        let db = state.db.lock_safe();
        db.list_projects().unwrap_or_default()
    };
    projects
        .iter()
        .filter_map(|p| {
            let recorded = sync_state.projects.get(&p.id)?;
            let clean = project_tree_hash(Path::new(&p.root_path)) == recorded.hash;
            Some(PeerProjectState {
                id: p.id.clone(),
                sync_id: recorded.sync_id.clone(),
                clean,
            })
        })
        .collect()
}

/// Decide which projects' files can be omitted from the archive: the receiver
/// reported the same sync generation AND clean files, and this side's files
/// are also unchanged since that generation.
pub fn decide_skips(
    state: &AppState,
    data_dir: &Path,
    peer: &[PeerProjectState],
) -> std::collections::HashSet<String> {
    let local = load_sync_state(data_dir);
    let projects = {
        let db = state.db.lock_safe();
        db.list_projects().unwrap_or_default()
    };
    let peer_by_id: HashMap<&str, &PeerProjectState> =
        peer.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut skips = std::collections::HashSet::new();
    for p in &projects {
        let Some(mine) = local.projects.get(&p.id) else {
            continue;
        };
        let Some(theirs) = peer_by_id.get(p.id.as_str()) else {
            continue;
        };
        if !theirs.clean || theirs.sync_id != mine.sync_id {
            continue;
        }
        if project_tree_hash(Path::new(&p.root_path)) == mine.hash {
            skips.insert(p.id.clone());
        }
    }
    skips
}

/// Export every known secret to a plain map: per-provider API keys (from the
/// hydrated in-memory config, falling back to the secret store) plus the git
/// token. The map travels INSIDE the tar.gz and is imported into the receiving
/// side's own secret backend (keychain on desktop, secrets file on server).
fn export_secrets(state: &AppState, secrets: &dyn SecretStore) -> HashMap<String, String> {
    let mut out = HashMap::new();
    {
        let agent = state.agent.lock_safe();
        for entry in agent.ai_config.providers.iter() {
            let acct = provider_account(entry.provider_type.as_str(), entry.name.as_deref());
            let val = if !entry.api_key.is_empty() {
                Some(entry.api_key.clone())
            } else {
                secrets.get(&acct).ok().flatten()
            };
            if let Some(v) = val.filter(|v| !v.is_empty()) {
                out.insert(acct, v);
            }
        }
    }
    let git_tok = state
        .git_token
        .lock_safe()
        .clone()
        .or_else(|| secrets.get(GIT_TOKEN_ACCOUNT).ok().flatten());
    if let Some(tok) = git_tok.filter(|t| !t.is_empty()) {
        out.insert(GIT_TOKEN_ACCOUNT.to_string(), tok);
    }
    out
}

/// Recursively append `src` under `prefix` in the tar, skipping
/// [`SYNC_EXCLUDED_DIRS`] names and symlinks.
fn append_dir_filtered<W: Write>(
    tar: &mut tar::Builder<W>,
    src: &Path,
    prefix: &str,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("read_dir {} failed: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue; // symlinks don't round-trip portably across OSes
        }
        let arch_path = format!("{prefix}/{name}");
        if meta.is_dir() {
            if SYNC_EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            tar.append_dir(&arch_path, &path)
                .map_err(|e| format!("tar dir {arch_path}: {e}"))?;
            append_dir_filtered(tar, &path, &arch_path)?;
        } else if meta.is_file() {
            tar.append_path_with_name(&path, &arch_path)
                .map_err(|e| format!("tar file {arch_path}: {e}"))?;
        }
    }
    Ok(())
}

/// Build a full sync archive at `out_path`. Projects whose id is in
/// `skip_files` travel manifest-only (no file entries) — use [`decide_skips`]
/// to compute that set safely. Returns the manifest describing the archive.
pub fn build_sync_archive(
    state: &AppState,
    data_dir: &Path,
    secrets: &dyn SecretStore,
    out_path: &Path,
    skip_files: &std::collections::HashSet<String>,
) -> Result<SyncManifest, String> {
    ensure_no_running_tasks(state)?;

    // 1. Consistent DB snapshot + project list, under the DB lock.
    let db_snapshot = data_dir.join("sync-db-snapshot.db");
    let _ = std::fs::remove_file(&db_snapshot);
    let projects = {
        let db = state.db.lock_safe();
        let _ = db.checkpoint_truncate();
        db.conn()
            .execute(
                "VACUUM INTO ?1",
                [db_snapshot.to_string_lossy().to_string()],
            )
            .map_err(|e| format!("DB snapshot failed: {e}"))?;
        db.list_projects().map_err(|e| e.to_string())?
    };

    let manifest = SyncManifest {
        version: SYNC_MANIFEST_VERSION,
        created_at: chrono::Utc::now().to_rfc3339(),
        origin_os: os_name(),
        sync_id: uuid::Uuid::new_v4().to_string(),
        projects: projects
            .iter()
            .map(|p| SyncProjectEntry {
                id: p.id.clone(),
                name: p.name.clone(),
                dir: format!("projects/{}", p.id),
                origin_root_path: p.root_path.clone(),
                files_skipped: skip_files.contains(&p.id),
            })
            .collect(),
    };

    let secrets_map = export_secrets(state, secrets);

    // 2. Stream everything into the tar.zst.
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::create(out_path)
            .map_err(|e| format!("create {} failed: {e}", out_path.display()))?;
        // Level 3: near-gzip-fast compression with a clearly better ratio, so
        // the CPU keeps ahead of the network while uploads shrink.
        let enc = zstd::stream::write::Encoder::new(file, 3)
            .map_err(|e| format!("zstd init failed: {e}"))?;
        let mut tar = tar::Builder::new(enc);
        tar.mode(tar::HeaderMode::Deterministic);

        append_bytes(
            &mut tar,
            "manifest.json",
            &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )?;
        append_bytes(
            &mut tar,
            "secrets.json",
            &serde_json::to_vec(&secrets_map).map_err(|e| e.to_string())?,
        )?;
        tar.append_path_with_name(&db_snapshot, "data/rustic.db")
            .map_err(|e| format!("tar db snapshot: {e}"))?;

        let fh_dir = data_dir.join("file-history");
        if fh_dir.is_dir() {
            tar.append_dir("data/file-history", &fh_dir)
                .map_err(|e| e.to_string())?;
            append_dir_filtered(&mut tar, &fh_dir, "data/file-history")?;
        }

        for (p, entry) in projects.iter().zip(manifest.projects.iter()) {
            if entry.files_skipped {
                continue; // receiver already holds identical content
            }
            let root = PathBuf::from(&p.root_path);
            if !root.is_dir() {
                tracing::warn!(path = %p.root_path, "sync: project root missing — skipped");
                continue;
            }
            tar.append_dir(&entry.dir, &root)
                .map_err(|e| e.to_string())?;
            append_dir_filtered(&mut tar, &root, &entry.dir)?;
        }

        let enc = tar.into_inner().map_err(|e| e.to_string())?;
        enc.finish()
            .map_err(|e| format!("zstd finish failed: {e}"))?;
        Ok(())
    })();

    let _ = std::fs::remove_file(&db_snapshot);
    result?;

    // Remember this sync generation so the next one can skip unchanged trees.
    let roots: Vec<(String, PathBuf)> = projects
        .iter()
        .map(|p| (p.id.clone(), PathBuf::from(&p.root_path)))
        .collect();
    record_sync_state(data_dir, &manifest.sync_id, &roots);
    Ok(manifest)
}

fn append_bytes<W: Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, name, bytes)
        .map_err(|e| format!("tar {name}: {e}"))
}

/// Open a sync archive for reading, sniffing the compression from its magic
/// bytes: zstd (current format) or gzip (accepted for compatibility with
/// archives built before the zstd switch).
fn open_archive_reader(path: &Path) -> Result<Box<dyn Read>, String> {
    let mut magic = [0u8; 4];
    {
        let mut f = std::fs::File::open(path)
            .map_err(|e| format!("open archive {}: {e}", path.display()))?;
        let _ = f.read(&mut magic).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if magic == [0x28, 0xB5, 0x2F, 0xFD] {
        Ok(Box::new(
            zstd::stream::read::Decoder::new(file).map_err(|e| format!("zstd open: {e}"))?,
        ))
    } else if magic[0] == 0x1F && magic[1] == 0x8B {
        Ok(Box::new(flate2::read::GzDecoder::new(file)))
    } else {
        Err("not a sync archive (unrecognized compression format)".into())
    }
}

/// Clear a read-only attribute (git object/pack files on Windows) then delete.
fn force_remove_file(path: &Path) {
    if std::fs::remove_file(path).is_ok() {
        return;
    }
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
    let _ = std::fs::remove_file(path);
}

/// `remove_dir_all` that survives read-only entries (`.git` object files).
fn force_remove_dir_all(path: &Path) {
    if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
        return;
    }
    // Fallback: strip read-only flags bottom-up, then retry.
    fn strip(path: &Path) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Ok(meta) = std::fs::symlink_metadata(&p) {
                    if meta.is_dir() {
                        strip(&p);
                    } else if meta.permissions().readonly() {
                        let mut perms = meta.permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        perms.set_readonly(false);
                        let _ = std::fs::set_permissions(&p, perms);
                    }
                }
            }
        }
    }
    strip(path);
    let _ = std::fs::remove_dir_all(path);
}

/// Copy a file over a possibly-read-only destination.
fn copy_file_force(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        if let Ok(meta) = std::fs::metadata(dst) {
            if meta.permissions().readonly() {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(dst, perms);
            }
        }
    }
    std::fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))
}

/// Make `dst` an exact mirror of `src`, except entries named in
/// [`SYNC_EXCLUDED_DIRS`] (at any depth) are left untouched in `dst`.
fn mirror_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;

    // Deletion pass: drop anything in dst that src doesn't have (or whose kind
    // changed), keeping excluded dirs (node_modules etc. stay usable locally).
    if let Ok(entries) = std::fs::read_dir(dst) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let dst_child = entry.path();
            let dst_meta = match std::fs::symlink_metadata(&dst_child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if dst_meta.is_dir() && SYNC_EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let src_child = src.join(&name);
            let src_meta = std::fs::symlink_metadata(&src_child).ok();
            match src_meta {
                None => {
                    if dst_meta.is_dir() {
                        force_remove_dir_all(&dst_child);
                    } else {
                        force_remove_file(&dst_child);
                    }
                }
                Some(sm) => {
                    // Kind mismatch: remove; the copy pass recreates it.
                    if sm.is_dir() != dst_meta.is_dir() {
                        if dst_meta.is_dir() {
                            force_remove_dir_all(&dst_child);
                        } else {
                            force_remove_file(&dst_child);
                        }
                    }
                }
            }
        }
    }

    // Copy pass.
    let entries = std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let src_child = entry.path();
        let dst_child = dst.join(&name);
        let meta = match std::fs::symlink_metadata(&src_child) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            mirror_dir(&src_child, &dst_child)?;
        } else if meta.is_file() {
            copy_file_force(&src_child, &dst_child)?;
        }
    }
    Ok(())
}

/// Apply a sync archive to this environment. Destructive: the local DB,
/// file-history store, secrets, and every manifest project's files are
/// replaced. Returns the applied manifest.
pub fn apply_sync_archive(
    state: &AppState,
    data_dir: &Path,
    secrets: &dyn SecretStore,
    archive_path: &Path,
    emitter: Arc<dyn EventEmitter>,
    resolve_root: ProjectRootResolver<'_>,
) -> Result<SyncManifest, String> {
    ensure_no_running_tasks(state)?;

    // 1. Extract to a staging dir under the data dir (same volume → cheap renames).
    let staging = data_dir.join(STAGING_DIR);
    force_remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    {
        let reader = open_archive_reader(archive_path)?;
        let mut archive = tar::Archive::new(reader);
        archive
            .unpack(&staging)
            .map_err(|e| format!("archive extraction failed: {e}"))?;
    }

    let manifest: SyncManifest = {
        let mut buf = String::new();
        std::fs::File::open(staging.join("manifest.json"))
            .map_err(|e| format!("archive has no manifest.json: {e}"))?
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&buf).map_err(|e| format!("bad manifest: {e}"))?
    };
    if manifest.version != SYNC_MANIFEST_VERSION {
        force_remove_dir_all(&staging);
        return Err(format!(
            "sync archive version {} is not supported by this build (expected {})",
            manifest.version, SYNC_MANIFEST_VERSION
        ));
    }
    if !staging.join("data/rustic.db").is_file() {
        force_remove_dir_all(&staging);
        return Err("sync archive is missing its database snapshot".into());
    }

    // 2. Remember where THIS machine previously kept each project (by id), so
    //    round-trips restore to the same local folders.
    let old_roots: HashMap<String, String> = {
        let db = state.db.lock_safe();
        db.list_projects()
            .map(|rows| rows.into_iter().map(|p| (p.id, p.root_path)).collect())
            .unwrap_or_default()
    };
    let targets: Vec<(SyncProjectEntry, PathBuf)> = manifest
        .projects
        .iter()
        .map(|e| {
            let old = old_roots.get(&e.id).map(|s| s.as_str());
            (e.clone(), resolve_root(e, old))
        })
        .collect();

    // 3. Quiesce: stop watchers, drop per-project handles and open buffers.
    {
        let project_paths: Vec<String> = state
            .workspace
            .lock_safe()
            .list_projects()
            .into_iter()
            .map(|p| p.root_path.to_string_lossy().to_string())
            .collect();
        let mut watcher = state.file_watcher.lock_safe();
        for p in project_paths {
            watcher.unwatch_project(&p);
        }
    }
    state.workspace.lock_safe().projects.clear();
    state.buffers.lock_safe().clear();
    state.highlighters.lock_safe().clear();
    state.file_history_registry.lock_safe().clear();
    {
        let mut agent = state.agent.lock_safe();
        agent.tasks.clear();
        agent.cancellation_tokens.clear();
    }

    // 4. Swap the live DB. A temporary in-memory handle releases the file so
    //    it can be replaced on Windows; reopening runs any newer migrations.
    let db_path = data_dir.join("rustic.db");
    {
        let mut db = state.db.lock_safe();
        let _ = db.checkpoint_truncate();
        *db = Database::in_memory().map_err(|e| format!("DB detach failed: {e}"))?;
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db_path.as_os_str().to_os_string();
            p.push(suffix);
            force_remove_file(Path::new(&p));
        }
        std::fs::copy(staging.join("data/rustic.db"), &db_path)
            .map_err(|e| format!("DB install failed: {e}"))?;
        *db = Database::new(&db_path).map_err(|e| format!("DB reopen failed: {e}"))?;

        // Rewrite project roots for this machine, plus a best-effort prefix
        // rewrite of file-history index paths so snapshot restore keeps
        // resolving after the move.
        for (entry, target) in &targets {
            let target_str = target.to_string_lossy().to_string();
            db.update_project_root(&entry.id, &target_str)
                .map_err(|e| e.to_string())?;
            if entry.origin_root_path != target_str {
                let _ = db.rewrite_file_history_prefix(&entry.origin_root_path, &target_str);
            }
        }
    }

    // 5. Replace the file-history blob store.
    let fh_dir = data_dir.join("file-history");
    force_remove_dir_all(&fh_dir);
    let staged_fh = staging.join("data/file-history");
    if staged_fh.is_dir() {
        if std::fs::rename(&staged_fh, &fh_dir).is_err() {
            mirror_dir(&staged_fh, &fh_dir)
                .map_err(|e| format!("file-history install failed: {e}"))?;
        }
    }

    // 6. Mirror project files into their resolved roots. Skipped projects
    //    already hold identical content — leave their files untouched.
    for (entry, target) in &targets {
        if entry.files_skipped {
            if !target.is_dir() {
                // Drift the clean-check should have caught; never leave a
                // referenced project root missing.
                tracing::warn!(project = %entry.name, "sync: skipped project missing on disk — creating empty root");
                std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
            }
            continue;
        }
        let staged = staging.join(&entry.dir);
        if staged.is_dir() {
            mirror_dir(&staged, target)?;
        } else {
            std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
        }
    }

    // 7. Import secrets into this side's own backend + refresh in-memory state.
    let staged_secrets = staging.join("secrets.json");
    if staged_secrets.is_file() {
        if let Ok(bytes) = std::fs::read(&staged_secrets) {
            if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
                for (acct, val) in &map {
                    if let Err(e) = secrets.set(acct, val) {
                        tracing::warn!(account = %acct, "sync: secret import failed: {e}");
                    }
                }
                *state.git_token.lock_safe() = map.get(GIT_TOKEN_ACCOUNT).cloned();
            }
        }
    }
    bootstrap::hydrate_config_and_secrets(state, secrets);

    // 8. Reload projects into the workspace + restart watchers.
    bootstrap::restore_projects(state, emitter.clone());

    // Remember this sync generation (post-apply fingerprints, i.e. THIS
    // machine's mtimes) so the next sync can skip unchanged trees.
    let roots: Vec<(String, PathBuf)> = targets
        .iter()
        .map(|(e, t)| (e.id.clone(), t.clone()))
        .collect();
    record_sync_state(data_dir, &manifest.sync_id, &roots);

    force_remove_dir_all(&staging);

    emitter.emit_json(
        "rustic:sync-imported",
        serde_json::json!({ "projects": manifest.projects.len() }),
    );
    Ok(manifest)
}

/// Sanitize a project name into a safe folder name for default import roots.
pub fn safe_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_hash_detects_changes() {
        let base = std::env::temp_dir().join(format!("rustic-sync-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("node_modules")).unwrap();
        std::fs::write(base.join("a.txt"), b"one").unwrap();
        std::fs::write(base.join("node_modules/ignored.js"), b"x").unwrap();

        let h1 = project_tree_hash(&base);
        assert_eq!(h1, project_tree_hash(&base), "hash must be stable");

        // Excluded dirs don't affect the fingerprint.
        std::fs::write(base.join("node_modules/more.js"), b"y").unwrap();
        assert_eq!(h1, project_tree_hash(&base));

        // A real file change does.
        std::fs::write(base.join("b.txt"), b"two").unwrap();
        assert_ne!(h1, project_tree_hash(&base));

        assert_eq!(project_tree_hash(&base.join("nope")), "missing");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn safe_dir_name_sanitizes() {
        assert_eq!(safe_dir_name("My App!/v2"), "My App--v2");
        assert_eq!(safe_dir_name("..."), "project");
        assert_eq!(safe_dir_name("ok-name_1.2"), "ok-name_1.2");
    }

    #[test]
    fn mirror_dir_replaces_and_preserves_excluded() {
        let base = std::env::temp_dir().join(format!("rustic-sync-test-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("keep.txt"), b"new").unwrap();
        std::fs::write(src.join("sub/child.txt"), b"c").unwrap();
        std::fs::create_dir_all(dst.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dst.join("stale-dir")).unwrap();
        std::fs::write(dst.join("keep.txt"), b"old").unwrap();
        std::fs::write(dst.join("stale.txt"), b"x").unwrap();
        std::fs::write(dst.join("node_modules/pkg/index.js"), b"m").unwrap();

        mirror_dir(&src, &dst).unwrap();

        assert_eq!(std::fs::read(dst.join("keep.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(dst.join("sub/child.txt")).unwrap(), b"c");
        assert!(!dst.join("stale.txt").exists());
        assert!(!dst.join("stale-dir").exists());
        assert!(dst.join("node_modules/pkg/index.js").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn archive_roundtrip_bytes() {
        // append_bytes entries unpack back to identical content, and the
        // reader sniffs both zstd (current) and gzip (legacy) archives.
        let base = std::env::temp_dir().join(format!("rustic-sync-tar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let zst_path = base.join("t.tar.zst");
        {
            let file = std::fs::File::create(&zst_path).unwrap();
            let enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
            let mut tar = tar::Builder::new(enc);
            append_bytes(&mut tar, "manifest.json", b"{\"a\":1}").unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = base.join("out-zst");
        let mut archive = tar::Archive::new(open_archive_reader(&zst_path).unwrap());
        archive.unpack(&out).unwrap();
        assert_eq!(
            std::fs::read(out.join("manifest.json")).unwrap(),
            b"{\"a\":1}"
        );

        let gz_path = base.join("t.tar.gz");
        {
            let file = std::fs::File::create(&gz_path).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            append_bytes(&mut tar, "manifest.json", b"{\"b\":2}").unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = base.join("out-gz");
        let mut archive = tar::Archive::new(open_archive_reader(&gz_path).unwrap());
        archive.unpack(&out).unwrap();
        assert_eq!(
            std::fs::read(out.join("manifest.json")).unwrap(),
            b"{\"b\":2}"
        );

        assert!(open_archive_reader(&out.join("manifest.json")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
