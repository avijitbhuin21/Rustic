//! Worktree-per-task + serialized merge queue (docs/plans/worktree-merge-queue.md).
//!
//! Each isolated task runs in its own detached `git worktree` under
//! `<repo>/.rustic/worktrees/<task_id>` (same drive as the checkout; a `*`
//! .gitignore keeps the dir invisible to git and to gitignore-respecting
//! walkers). Deleting a task deletes its worktree, and a startup sweep
//! prunes orphans in both the per-repo base and the legacy app-data base.
//! One merge worker
//! per repository root serializes landings: rebase onto current main →
//! squash → optional validation hook → compare-and-swap fast-forward.
//! Conflicts park the item as `needs-reconciliation` (rebase left mid-flight
//! so conflict markers stay visible for manual or agent-assisted resolution)
//! and the worker moves on — no head-of-line blocking.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustic_db::TaskWorktreeRow;
use rustic_git::GitRepo;

use crate::context::{EventEmitter, EventEmitterExt};
use crate::state::AppState;
use crate::sync_ext::MutexExt;

/// Shared handle to the SQLite database — the one Arc field every worktree
/// operation needs. Both hosts own `AppState.db` as exactly this type.
pub type Db = Arc<Mutex<rustic_db::Database>>;

/// Event emitted on every worktree state transition. Payload is the full row.
pub const WORKTREE_EVENT: &str = "worktree-state-changed";

/// Settings key holding the optional validation command (JSON string).
pub const VALIDATION_CMD_KEY: &str = "worktree_validation_command";
/// JSON object mapping project id → validation command; overrides the global
/// command for merges of that project's worktrees.
pub const PROJECT_VALIDATION_CMDS_KEY: &str = "worktree_validation_commands_by_project";
/// Settings key holding the validation timeout in seconds (JSON number).
pub const VALIDATION_TIMEOUT_KEY: &str = "worktree_validation_timeout_secs";
/// Settings key holding directories to link from the main checkout into new
/// worktrees. Re-exported from the shared setup module (rustic-agent) so
/// existing `crate::worktree::SYMLINK_DIRS_KEY` references keep resolving.
pub use rustic_agent::worktree_setup::SYMLINK_DIRS_KEY;
/// Settings key holding an optional worktree-create hook (JSON string) — the
/// non-git VCS escape hatch, mirroring Claude Code's WorktreeCreate hook.
/// Invoked as `<command> <task_id>` with cwd = the project root; must print
/// the created workspace's ABSOLUTE path as the last non-empty stdout line.
/// Hook-based worktrees are isolation-only: the merge queue never touches
/// them (no rebase/squash/land) — `base_oid` stays empty as the marker.
pub const CREATE_HOOK_KEY: &str = "worktree_create_hook";
/// Matching remove hook (JSON string): invoked as `<command> <worktree_path>`
/// with cwd = the project root. When unset, hook-based worktree directories
/// are left in place on discard (we can't know how to tear them down).
pub const REMOVE_HOOK_KEY: &str = "worktree_remove_hook";
const DEFAULT_VALIDATION_TIMEOUT_SECS: u64 = 600;

/// Legacy base directory for task worktrees (`<data_dir>/worktrees`).
/// New worktrees land in-repo (see [`project_worktree_base`]); this path is
/// still swept by [`prune_orphans`] so pre-existing worktrees get reclaimed.
pub fn worktree_base_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("worktrees")
}

/// Per-repo base directory for task worktrees: `<repo>/.rustic/worktrees`.
/// In-repo keeps worktrees on the same drive as the checkout (cheap
/// `git worktree add`, no cross-drive path confusion); a `*` .gitignore
/// written alongside keeps the dir out of `git status` and out of every
/// gitignore-respecting walker (file tree, search, index, file history).
pub fn project_worktree_base(project_root: &Path) -> PathBuf {
    project_root.join(".rustic").join("worktrees")
}

/// Create the per-repo worktree base and its self-ignoring `.gitignore`.
fn ensure_worktree_base(project_root: &Path) -> Result<PathBuf, String> {
    let base = project_worktree_base(project_root);
    std::fs::create_dir_all(&base)
        .map_err(|e| format!("cannot create worktree base {}: {e}", base.display()))?;
    let gitignore = base.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "*\n")
            .map_err(|e| format!("cannot write worktree .gitignore: {e}"))?;
    }
    Ok(base)
}

fn emit_row(emitter: &dyn EventEmitter, row: &TaskWorktreeRow) {
    emitter.emit(WORKTREE_EVENT, row);
}

fn fetch_row(db: &Db, task_id: &str) -> Result<TaskWorktreeRow, String> {
    db.lock_safe()
        .wt_get(task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree registered for task {task_id}"))
}

fn emit_current(db: &Db, emitter: &dyn EventEmitter, task_id: &str) {
    if let Ok(Some(row)) = db.lock_safe().wt_get(task_id) {
        emit_row(emitter, &row);
    }
}

/// Read the worktree settings (validation command/timeout, linked dirs) as
/// one JSON object for the settings UI.
pub fn get_worktree_settings(db: &Db) -> serde_json::Value {
    let db = db.lock_safe();
    let command = db
        .get_setting(VALIDATION_CMD_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok().or(Some(v)))
        .unwrap_or_default();
    let timeout = db
        .get_setting(VALIDATION_TIMEOUT_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<u64>(&v).ok())
        .unwrap_or(DEFAULT_VALIDATION_TIMEOUT_SECS);
    let dirs: Vec<String> = db
        .get_setting(SYMLINK_DIRS_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let read_str = |key: &str| -> String {
        db.get_setting(key)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<String>(&v).ok().or(Some(v)))
            .unwrap_or_default()
    };
    let create_hook = read_str(CREATE_HOOK_KEY);
    let remove_hook = read_str(REMOVE_HOOK_KEY);
    let per_project: serde_json::Value = db
        .get_setting(PROJECT_VALIDATION_CMDS_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    serde_json::json!({
        "validation_command": command,
        "validation_timeout_secs": timeout,
        "symlink_directories": dirs,
        "create_hook": create_hook,
        "remove_hook": remove_hook,
        "project_validation_commands": per_project,
    })
}

/// Persist the worktree settings from the settings UI. An empty/blank
/// validation command deletes the key (validation disabled).
pub fn set_worktree_settings(db: &Db, settings: &serde_json::Value) -> Result<(), String> {
    let db = db.lock_safe();
    if let Some(cmd) = settings.get("validation_command").and_then(|v| v.as_str()) {
        if cmd.trim().is_empty() {
            db.delete_setting(VALIDATION_CMD_KEY)
                .map_err(|e| e.to_string())?;
        } else {
            db.set_setting(
                VALIDATION_CMD_KEY,
                &serde_json::to_string(cmd.trim()).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if let Some(secs) = settings
        .get("validation_timeout_secs")
        .and_then(|v| v.as_u64())
    {
        db.set_setting(VALIDATION_TIMEOUT_KEY, &secs.to_string())
            .map_err(|e| e.to_string())?;
    }
    if let Some(dirs) = settings
        .get("symlink_directories")
        .and_then(|v| v.as_array())
    {
        let clean: Vec<String> = dirs
            .iter()
            .filter_map(|d| d.as_str())
            .map(|s| s.trim().trim_matches(['/', '\\']).to_string())
            .filter(|s| !s.is_empty())
            .collect();
        db.set_setting(
            SYMLINK_DIRS_KEY,
            &serde_json::to_string(&clean).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    for (field, key) in [
        ("create_hook", CREATE_HOOK_KEY),
        ("remove_hook", REMOVE_HOOK_KEY),
    ] {
        if let Some(cmd) = settings.get(field).and_then(|v| v.as_str()) {
            if cmd.trim().is_empty() {
                db.delete_setting(key).map_err(|e| e.to_string())?;
            } else {
                db.set_setting(
                    key,
                    &serde_json::to_string(cmd.trim()).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    if let Some(map) = settings
        .get("project_validation_commands")
        .and_then(|v| v.as_object())
    {
        let clean: std::collections::BTreeMap<String, String> = map
            .iter()
            .filter_map(|(k, v)| {
                let cmd = v.as_str()?.trim().to_string();
                let key = k.trim().to_string();
                (!key.is_empty() && !cmd.is_empty()).then_some((key, cmd))
            })
            .collect();
        if clean.is_empty() {
            db.delete_setting(PROJECT_VALIDATION_CMDS_KEY)
                .map_err(|e| e.to_string())?;
        } else {
            db.set_setting(
                PROJECT_VALIDATION_CMDS_KEY,
                &serde_json::to_string(&clean).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Returns true when the project root can host an isolated worktree: either
/// a WorktreeCreate hook is configured, or it is a git repository with at
/// least one commit. Used by hosts to silently fall back to a non-isolated
/// task instead of failing task creation.
pub fn can_isolate(db: &Db, project_root: &str) -> bool {
    if get_hook(db, CREATE_HOOK_KEY).is_some() {
        return true;
    }
    match GitRepo::open(Path::new(project_root)) {
        Ok(repo) => repo.has_commits(),
        Err(_) => false,
    }
}

/// True when `row` was created by the user's WorktreeCreate hook (non-git).
/// Hook-based rows never enter the merge queue.
pub fn is_hook_based(row: &TaskWorktreeRow) -> bool {
    row.base_oid.is_empty()
}

/// Read a hook command from settings; `None` when unset or blank.
fn get_hook(db: &Db, key: &str) -> Option<String> {
    db.lock_safe()
        .get_setting(key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok().or(Some(v)))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run a hook command with one quoted argument, cwd'd to the project root.
/// Returns captured stdout on success, stderr text on failure.
///
/// The argument is passed via the `RUSTIC_HOOK_ARG` environment variable and
/// referenced from the command line as a shell variable — never interpolated
/// into the command string itself — so a value containing quotes or shell
/// metacharacters cannot break out of the quoting (on sh, `"$VAR"` is
/// expanded after parsing; naive `format!("{cmd} \"{arg}\"")` was not). Hook
/// scripts keep receiving the value as `$1` and can also read
/// `RUSTIC_HOOK_ARG` directly.
fn run_hook(command: &str, arg: &str, cwd: &Path) -> Result<String, String> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let full = format!("{command} \"%RUSTIC_HOOK_ARG%\"");
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(&full).creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let full = format!("{command} \"$RUSTIC_HOOK_ARG\"");
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(&full);
        c
    };
    let out = cmd
        .env("RUSTIC_HOOK_ARG", arg)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to spawn worktree hook: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "worktree hook failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Create the isolated worktree for a task: check out the project's HEAD as
/// a DETACHED worktree under `<repo>/.rustic/worktrees/<id>` (no branch —
/// task isolation stays invisible to `git branch` and to pushes),
/// run post-create setup (`.env*` allowlist, `.worktreeinclude`, linked
/// dirs, hooksPath), and persist the row (state `active`).
/// Blocking — call from `spawn_blocking`.
pub fn create_task_worktree(
    db: &Db,
    _data_dir: &Path,
    project_id: &str,
    project_root: &str,
    task_id: &str,
) -> Result<TaskWorktreeRow, String> {
    if let Ok(Some(existing)) = db.lock_safe().wt_get(task_id) {
        return Ok(existing);
    }

    // Non-git escape hatch: a configured WorktreeCreate hook takes precedence
    // (the user explicitly wired their own VCS). Isolation-only — empty
    // branch/base fields mark the row so the merge queue skips it.
    if let Some(hook) = get_hook(db, CREATE_HOOK_KEY) {
        let out = run_hook(&hook, task_id, Path::new(project_root))?;
        let path_line = out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .ok_or("worktree create hook printed no path on stdout")?
            .trim()
            .to_string();
        let p = PathBuf::from(&path_line);
        if !p.is_absolute() || !p.exists() {
            return Err(format!(
                "worktree create hook must print an existing absolute path as its last stdout line; got '{path_line}'"
            ));
        }
        db.lock_safe()
            .wt_insert(
                task_id,
                project_id,
                project_root,
                &p.to_string_lossy(),
                "",
                "",
                "",
            )
            .map_err(|e| e.to_string())?;
        return fetch_row(db, task_id);
    }

    let repo = GitRepo::open(Path::new(project_root))
        .map_err(|e| format!("not a git repository ({project_root}): {e}"))?;
    if !repo.has_commits() {
        return Err("cannot isolate task: repository has no commits yet".into());
    }

    let base_branch = repo.head_branch().map_err(|e| e.to_string())?;
    if base_branch.contains("detached") {
        return Err("cannot isolate task: repository HEAD is detached".into());
    }
    let base_oid = repo.rev_parse("HEAD").map_err(|e| e.to_string())?;

    let wt_path = ensure_worktree_base(Path::new(project_root))?.join(task_id);
    repo.add_worktree_detached(&wt_path, &base_oid)
        .map_err(|e| format!("git worktree add failed: {e}"))?;

    rustic_agent::worktree_setup::post_create_setup(Some(db), Path::new(project_root), &wt_path);
    rustic_agent::worktree_setup::seed_uncommitted(Path::new(project_root), &wt_path);

    {
        let db = db.lock_safe();
        db.wt_insert(
            task_id,
            project_id,
            project_root,
            &wt_path.to_string_lossy(),
            "",
            &base_branch,
            &base_oid,
        )
        .map_err(|e| e.to_string())?;
    }

    fetch_row(db, task_id)
}

/// Discard a task's worktree: force-remove the checkout, delete the branch,
/// prune its shadow file-history repo, and drop the row. Perfect revert of
/// everything the task did (pre-merge). Returns the repo root when a row
/// existed so callers can kick the merge worker (a discarded parked head
/// unblocks the halted FIFO).
/// Blocking — call from `spawn_blocking`. Best-effort: never fails on
/// already-missing pieces.
pub fn discard_task_worktree(
    db: &Db,
    data_dir: &Path,
    task_id: &str,
) -> Result<Option<String>, String> {
    let Some(row) = db.lock_safe().wt_get(task_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    remove_worktree_and_branch(db, &row, data_dir);
    db.lock_safe()
        .wt_delete(task_id)
        .map_err(|e| e.to_string())?;
    Ok(Some(row.project_root))
}

/// Best-effort disk + branch cleanup for a row. Tolerates missing repo,
/// missing worktree dir, and missing branch. Prunes the worktree's shadow
/// file-history repo FIRST (its path derives from the canonicalized worktree
/// path, which stops resolving once the directory is deleted). Hook-based
/// rows delegate to the WorktreeRemove hook; without one the directory is
/// left in place (we can't know how to tear down a foreign VCS workspace).
fn remove_worktree_and_branch(db: &Db, row: &TaskWorktreeRow, data_dir: &Path) {
    let p = Path::new(&row.worktree_path);
    rustic_agent::file_history::shadow::remove_shadow_for_worktree(p, data_dir);
    if is_hook_based(row) {
        match get_hook(db, REMOVE_HOOK_KEY) {
            Some(hook) => {
                if let Err(e) = run_hook(&hook, &row.worktree_path, Path::new(&row.project_root)) {
                    tracing::warn!(task = %row.task_id, %e, "worktree remove hook failed");
                }
            }
            None => tracing::warn!(
                task = %row.task_id,
                path = %row.worktree_path,
                "no worktree_remove_hook configured; hook-based worktree left in place"
            ),
        }
        return;
    }
    if let Ok(repo) = GitRepo::open(Path::new(&row.project_root)) {
        if repo.remove_worktree(&row.task_id, true).is_err() {
            if p.exists() {
                let _ = std::fs::remove_dir_all(p);
            }
            let _ = repo.prune_worktrees();
        }
        if !row.branch.is_empty() {
            let _ = repo.delete_branch(&row.branch);
        }
    }
    if p.exists() {
        let _ = std::fs::remove_dir_all(p);
    }
}

/// Task-deletion hook: reclaim the worktree (any state). The DB row cascades
/// away with the task; this only handles disk + branch + shadow history.
/// Returns the repo root when a row existed so callers can kick the merge
/// worker — deleting a parked task must resume the halted FIFO, or the
/// remaining queued rows sit forever.
pub fn cleanup_for_task_delete(db: &Db, data_dir: &Path, task_id: &str) -> Option<String> {
    let row = db.lock_safe().wt_get(task_id).ok().flatten()?;
    remove_worktree_and_branch(db, &row, data_dir);
    let _ = db.lock_safe().wt_delete(task_id);
    Some(row.project_root)
}

/// Startup sweep: reset interrupted `merging` rows to `queued`, drop rows
/// whose worktree directory vanished, and delete orphan directories that no
/// row references — in the legacy `<data_dir>/worktrees` base AND in every
/// known project's `.rustic/worktrees` base.
pub fn prune_orphans(db: &Db, data_dir: &Path) {
    let (rows, project_roots) = {
        let db = db.lock_safe();
        let _ = db.wt_reset_interrupted();
        (
            db.wt_list_all().unwrap_or_default(),
            db.list_projects()
                .map(|ps| ps.into_iter().map(|p| p.root_path).collect::<Vec<_>>())
                .unwrap_or_default(),
        )
    };

    let mut referenced: HashSet<String> = HashSet::new();
    for row in &rows {
        let terminal = matches!(row.state.as_str(), "merged" | "discarded");
        let exists = Path::new(&row.worktree_path).exists();
        if terminal || !exists {
            remove_worktree_and_branch(db, row, data_dir);
            if row.state != "merged" {
                let _ = db.lock_safe().wt_delete(&row.task_id);
            }
        } else {
            migrate_branch_to_detached(db, row);
            referenced.insert(row.task_id.clone());
        }
    }

    let mut bases: Vec<PathBuf> = vec![worktree_base_dir(data_dir)];
    for root in &project_roots {
        bases.push(project_worktree_base(Path::new(root)));
    }
    for row in &rows {
        bases.push(project_worktree_base(Path::new(&row.project_root)));
    }
    bases.sort();
    bases.dedup();

    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if referenced.contains(&name) {
                continue;
            }
            // Fail-closed (Claude Code convention): an unreferenced dir that
            // is a git worktree with uncommitted changes is NOT swept — it
            // may be a kept sub-agent worktree whose changes the orchestrator
            // hasn't integrated yet. Clean or unreadable dirs are reclaimed.
            if let Ok(wt) = GitRepo::open(&entry.path()) {
                match wt.status_limited(Some(1)) {
                    Ok(s) if !s.files.is_empty() => {
                        tracing::info!(
                            path = %entry.path().display(),
                            "orphan sweep: skipping dirty worktree (unintegrated changes)"
                        );
                        continue;
                    }
                    Err(_) => continue,
                    _ => {}
                }
            }
            rustic_agent::file_history::shadow::remove_shadow_for_worktree(&entry.path(), data_dir);
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// One-time migration for pre-detached worktrees: older versions backed each
/// task worktree with a visible `rustic/task/<id>` branch. Detaches the
/// worktree's HEAD in place and deletes the branch so task isolation leaves
/// no trace in `git branch` or on any remote; skips worktrees mid-rebase.
fn migrate_branch_to_detached(db: &Db, row: &TaskWorktreeRow) {
    if row.branch.is_empty() {
        return;
    }
    let Ok(wt) = GitRepo::open(Path::new(&row.worktree_path)) else {
        return;
    };
    if wt.rebase_in_progress().unwrap_or(true) {
        return;
    }
    if wt.detach_head().is_err() {
        return;
    }
    if let Ok(main) = GitRepo::open(Path::new(&row.project_root)) {
        let _ = main.delete_branch(&row.branch);
    }
    let _ = db.lock_safe().wt_set_branch(&row.task_id, "");
}

/// Compose the review diff (task's own changes: `merge-base(base_branch,
/// branch)..HEAD` inside the worktree). Blocking.
pub fn review_diff(row: &TaskWorktreeRow) -> Result<Vec<rustic_git::FileDiff>, String> {
    if is_hook_based(row) {
        return Err("hook-based worktree (non-git): diffs are unavailable".into());
    }
    let wt = GitRepo::open(Path::new(&row.worktree_path)).map_err(|e| e.to_string())?;
    let base = wt
        .merge_base(&row.base_branch, "HEAD")
        .unwrap_or_else(|_| row.base_oid.clone());
    wt.diff_range(&base, "HEAD").map_err(|e| e.to_string())
}

/// Per-file review diff for the editor's diff tab. Blocking.
pub fn review_file_diff(row: &TaskWorktreeRow, path: &str) -> Result<rustic_git::FileDiff, String> {
    if is_hook_based(row) {
        return Err("hook-based worktree (non-git): diffs are unavailable".into());
    }
    let wt = GitRepo::open(Path::new(&row.worktree_path)).map_err(|e| e.to_string())?;
    let base = wt
        .merge_base(&row.base_branch, "HEAD")
        .unwrap_or_else(|_| row.base_oid.clone());
    wt.diff_range_file(&base, "HEAD", path)
        .map_err(|e| e.to_string())
}

/// Commit everything pending in a task's worktree as a turn checkpoint.
/// No-op when the tree is clean or the task has no worktree. Blocking.
pub fn turn_checkpoint_commit(db: &Db, task_id: &str) {
    let Ok(Some(row)) = db.lock_safe().wt_get(task_id) else {
        return;
    };
    if is_hook_based(&row) {
        return;
    }
    let Ok(wt) = GitRepo::open(Path::new(&row.worktree_path)) else {
        return;
    };
    if wt.rebase_in_progress().unwrap_or(false) {
        return;
    }
    match wt.status_limited(Some(1)) {
        Ok(s) if !s.files.is_empty() => {
            let _ = wt.stage_all();
            let _ = wt.commit(&format!(
                "rustic checkpoint: task {}",
                &task_id[..task_id.len().min(8)]
            ));
        }
        _ => {}
    }
}

/// Build the agent prompt for resolving a parked rebase. Includes structured
/// conflict data from the worktree. Blocking.
pub fn conflict_resolution_prompt(row: &TaskWorktreeRow) -> Result<String, String> {
    if is_hook_based(row) {
        return Err("hook-based worktree (non-git): nothing to rebase".into());
    }
    let wt = GitRepo::open(Path::new(&row.worktree_path)).map_err(|e| e.to_string())?;
    let conflicts = wt.get_conflicts().map_err(|e| e.to_string())?;
    let mut prompt = String::from(
        "The merge queue tried to land this task's changes but the rebase onto \
         the base branch hit conflicts. The rebase is PAUSED mid-flight in this \
         worktree with conflict markers in place.\n\n",
    );
    prompt.push_str(&format!("Rebasing onto: {}\n", row.base_branch));
    if let Some(err) = &row.last_error {
        prompt.push_str(&format!("Queue error: {err}\n"));
    }
    if conflicts.is_empty() {
        prompt.push_str(
            "\nNo conflict markers remain. Verify the working tree, then run \
             `git rebase --continue` (repeat if further conflicts appear) until \
             the rebase completes.\n",
        );
    } else {
        prompt.push_str(&format!("\nConflicted files ({}):\n", conflicts.len()));
        for c in &conflicts {
            prompt.push_str(&format!(
                "- {} ({} conflict hunk(s))\n",
                c.path,
                c.hunks.len()
            ));
        }
        prompt.push_str(
            "\nResolve every conflict by editing the files (remove ALL \
             <<<<<<< / ======= / >>>>>>> markers, keeping the correct combination \
             of both sides), `git add` each resolved file, then run \
             `git rebase --continue` (with GIT_EDITOR=true to skip the message \
             editor). Repeat until the rebase completes. Do NOT abort the rebase \
             and do NOT force-push anything.\n",
        );
    }
    prompt.push_str(
        "\nWhen the rebase has fully completed, the task will be re-queued for \
         merging automatically at the end of your turn.\n",
    );
    Ok(prompt)
}

/// What a parked (`needs-reconciliation`) worktree actually needs.
pub enum Reconcile {
    /// A rebase is paused mid-flight (conflict markers or a resolvable pause):
    /// hand the agent the resolution prompt.
    AgentPrompt(String),
    /// No rebase in progress and no conflicts — the park is stale (e.g. an
    /// interrupted/cancelled merge). Nothing to resolve; just re-queue.
    AlreadyClean,
}

/// Decide how to reconcile a parked worktree WITHOUT starting an agent turn on
/// a clean tree. Blocking. Prevents the `git rebase --continue` → "no rebase in
/// progress" dead-end when the park wasn't an actual conflict.
pub fn reconcile_plan(row: &TaskWorktreeRow) -> Result<Reconcile, String> {
    if is_hook_based(row) {
        return Err("hook-based worktree (non-git): nothing to reconcile".into());
    }
    let wt = GitRepo::open(Path::new(&row.worktree_path)).map_err(|e| e.to_string())?;
    let mid_rebase = wt.rebase_in_progress().unwrap_or(false);
    let conflicted = wt.has_conflicts().unwrap_or(false);
    if !mid_rebase && !conflicted {
        return Ok(Reconcile::AlreadyClean);
    }
    Ok(Reconcile::AgentPrompt(conflict_resolution_prompt(row)?))
}

/// Pre-revert guard for isolated tasks. A parked mid-rebase worktree must
/// abort its rebase BEFORE the tracker restores files (`git rebase --abort`
/// hard-resets the tree and would wipe the freshly reverted contents), and a
/// `merging` worktree may not be reverted at all — the merge worker owns it.
/// No-op for non-isolated tasks and every other state. Blocking.
pub fn prepare_revert(db: &Db, task_id: &str) -> Result<(), String> {
    let Some(row) = db.lock_safe().wt_get(task_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if is_hook_based(&row) {
        return Ok(());
    }
    match row.state.as_str() {
        "merging" => Err(
            "This task is being merged right now — wait for the merge to finish (or park) before reverting."
                .into(),
        ),
        "needs-reconciliation" => {
            let Ok(wt) = GitRepo::open(Path::new(&row.worktree_path)) else {
                return Ok(());
            };
            if wt.rebase_in_progress().unwrap_or(false) {
                wt.rebase_abort().map_err(|e| {
                    format!("could not abort the parked rebase before reverting: {e}")
                })?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// End-of-turn hook outcome, decided under the blocking git checks.
enum EndTurnOutcome {
    NoWorktree,
    /// Parked reconciliation whose rebase now completes cleanly, OR a normal
    /// turn that left new commits on the branch — both re-enter the queue.
    Enqueue,
    Idle,
}

/// Shared end-of-turn hook for isolated tasks: commit a turn checkpoint and
/// AUTO-ENQUEUE the merge when the branch carries anything new (auto-merge
/// model — no manual review gate). Parked reconciliations re-enqueue once
/// their rebase completes cleanly. Call right after `record_final_state`.
pub async fn end_of_turn(state: &AppState, emitter: &Arc<dyn EventEmitter>, task_id: &str) {
    let db = state.db.clone();
    let tid = task_id.to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        let Ok(Some(row)) = db.lock_safe().wt_get(&tid) else {
            return EndTurnOutcome::NoWorktree;
        };
        // Hook-based worktrees (non-git) never auto-merge.
        if is_hook_based(&row) {
            return EndTurnOutcome::Idle;
        }
        turn_checkpoint_commit(&db, &tid);
        let Ok(wt) = GitRepo::open(Path::new(&row.worktree_path)) else {
            return EndTurnOutcome::Idle;
        };
        if row.state == "needs-reconciliation" {
            let mid_rebase = wt.rebase_in_progress().unwrap_or(true);
            let conflicted = wt.has_conflicts().unwrap_or(true);
            if !mid_rebase && !conflicted {
                return EndTurnOutcome::Enqueue;
            }
            return EndTurnOutcome::Idle;
        }
        if row.state != "active" {
            return EndTurnOutcome::Idle;
        }
        // Anything to merge? The checkpoint commit above folded any dirty
        // files into the branch, so "branch tip moved off the fork point"
        // is the complete signal.
        match wt.rev_parse("HEAD") {
            Ok(tip) if tip != row.base_oid => EndTurnOutcome::Enqueue,
            _ => EndTurnOutcome::Idle,
        }
    })
    .await
    .unwrap_or(EndTurnOutcome::NoWorktree);

    match outcome {
        EndTurnOutcome::NoWorktree | EndTurnOutcome::Idle => {}
        EndTurnOutcome::Enqueue => {
            if let Err(e) = enqueue_merge(state, emitter, task_id) {
                tracing::warn!(task = %task_id, %e, "auto-merge enqueue failed");
            }
        }
    }
}

/// Returns the worktree root to run a turn in when the task is isolated.
/// `Err` when the task is mid-merge (the turn must not run); `Ok(None)` for
/// non-isolated tasks and merged/discarded worktrees (fall back to the main
/// checkout). Reactivates queued/review rows — a new user turn means the
/// user kept working.
pub fn turn_root_override(
    db_arc: &Db,
    emitter: &Arc<dyn EventEmitter>,
    task_id: &str,
) -> Result<Option<String>, String> {
    let db = db_arc.lock_safe();
    let Some(row) = db.wt_get(task_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    match row.state.as_str() {
        "merged" | "discarded" => Ok(None),
        "merging" => Err(
            "This task is being merged right now — wait for the merge to finish (or for it to park) before sending another message."
                .into(),
        ),
        _ => {
            let _ = db.wt_reactivate(task_id);
            drop(db);
            emit_current(db_arc, emitter.as_ref(), task_id);
            Ok(Some(row.worktree_path))
        }
    }
}

/// Like `turn_root_override`, but a task in the brief `merging` window waits
/// (bounded) for the merge worker to release the row instead of failing the
/// send outright.
pub async fn turn_root_override_wait(
    db_arc: &Db,
    emitter: &Arc<dyn EventEmitter>,
    task_id: &str,
) -> Result<Option<String>, String> {
    const POLL: std::time::Duration = std::time::Duration::from_millis(500);
    const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
    let start = std::time::Instant::now();
    loop {
        match turn_root_override(db_arc, emitter, task_id) {
            Err(e) => {
                let merging = {
                    let db = db_arc.lock_safe();
                    matches!(db.wt_get(task_id), Ok(Some(ref r)) if r.state == "merging")
                };
                if !merging {
                    return Err(e);
                }
                if start.elapsed() >= MAX_WAIT {
                    return Err(
                        "This task's merge has been running for over 5 minutes — wait for it to finish (or for it to park) before sending another message."
                            .into(),
                    );
                }
                tokio::time::sleep(POLL).await;
            }
            ok => return ok,
        }
    }
}

/// Enqueue a task's worktree for merging and make sure the repo's merge
/// worker is running. Async-context only (spawns tokio tasks).
pub fn enqueue_merge(
    state: &AppState,
    emitter: &Arc<dyn EventEmitter>,
    task_id: &str,
) -> Result<(), String> {
    let row = fetch_row(&state.db, task_id)?;
    if is_hook_based(&row) {
        return Err(
            "hook-based worktree (non-git): auto-merge is unavailable — integrate the changes manually"
                .into(),
        );
    }
    match row.state.as_str() {
        "merging" | "merged" | "discarded" => {
            return Err(format!("cannot enqueue from state '{}'", row.state));
        }
        _ => {}
    }
    state
        .db
        .lock_safe()
        .wt_enqueue(task_id)
        .map_err(|e| e.to_string())?;
    emit_current(&state.db, emitter.as_ref(), task_id);
    state
        .merge_queues
        .clone()
        .ensure_worker(state.db.clone(), emitter.clone(), row.project_root);
    Ok(())
}

/// One merge worker per repository root, strictly serialized. The queue is
/// derived from `task_worktrees` (`state='queued' ORDER BY queued_at`), so
/// restarts recover for free.
pub struct MergeQueues {
    active_roots: Mutex<HashSet<String>>,
}

impl MergeQueues {
    pub fn new() -> Self {
        Self {
            active_roots: Mutex::new(HashSet::new()),
        }
    }

    /// Spawn the per-repo worker loop if it isn't already running. The worker
    /// runs on its OWN thread with a dedicated single-threaded tokio runtime
    /// so it survives the teardown of whatever runtime enqueued the merge —
    /// the desktop host runs each turn on a short-lived runtime, and a worker
    /// spawned onto it via `tokio::spawn` was killed mid-merge when the turn
    /// finished, wedging rows in `merging` and leaking the root in
    /// `active_roots`.
    pub fn ensure_worker(
        self: Arc<Self>,
        db: Db,
        emitter: Arc<dyn EventEmitter>,
        project_root: String,
    ) {
        {
            let mut active = self.active_roots.lock_safe();
            if !active.insert(project_root.clone()) {
                return;
            }
        }
        let queues = self.clone();
        let root_for_cleanup = project_root.clone();
        let spawned = std::thread::Builder::new()
            .name("rustic-merge-worker".into())
            .spawn(move || {
                let _guard = RootGuard {
                    queues: self.clone(),
                    root: project_root.clone(),
                };
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(%e, "merge worker: failed to build runtime");
                        return;
                    }
                };
                rt.block_on(async {
                    loop {
                        // Strict FIFO: a parked (needs-reconciliation) row is
                        // the head of this root's queue mid-resolution. Halt
                        // instead of landing later items past it — every land
                        // moves the base tip and would invalidate the
                        // resolution in flight. The queue resumes when the
                        // parked task re-enqueues (end_of_turn / reconcile)
                        // or is discarded — both call ensure_worker.
                        {
                            let mut active = self.active_roots.lock_safe();
                            let parked =
                                db.lock_safe().wt_has_parked(&project_root).unwrap_or(false);
                            if parked {
                                active.remove(&project_root);
                                break;
                            }
                        }
                        let next = db.lock_safe().wt_next_queued(&project_root).ok().flatten();
                        match next {
                            Some(row) => process_queue_item(&db, &emitter, row).await,
                            None => {
                                let mut active = self.active_roots.lock_safe();
                                let still_queued = db
                                    .lock_safe()
                                    .wt_next_queued(&project_root)
                                    .ok()
                                    .flatten()
                                    .is_some();
                                if still_queued {
                                    continue;
                                }
                                active.remove(&project_root);
                                break;
                            }
                        }
                    }
                });
            });
        if let Err(e) = spawned {
            tracing::error!(%e, "merge worker: failed to spawn thread");
            queues.active_roots.lock_safe().remove(&root_for_cleanup);
        }
    }

    /// Startup kick: resume workers for any repo that has queued items.
    pub fn resume_pending(self: &Arc<Self>, db: &Db, emitter: &Arc<dyn EventEmitter>) {
        let rows = db.lock_safe().wt_list_all().unwrap_or_default();
        let mut roots: HashSet<String> = HashSet::new();
        for row in rows {
            if row.state == "queued" {
                roots.insert(row.project_root);
            }
        }
        for root in roots {
            self.clone()
                .ensure_worker(db.clone(), emitter.clone(), root);
        }
    }
}

/// Releases a repo root from `active_roots` when its worker thread exits —
/// panics included — so a future enqueue can respawn the worker instead of
/// stalling forever behind a dead "active" entry.
struct RootGuard {
    queues: Arc<MergeQueues>,
    root: String,
}

impl Drop for RootGuard {
    fn drop(&mut self) {
        self.queues.active_roots.lock_safe().remove(&self.root);
    }
}

enum PrepOutcome {
    Prepared { new_tip: String, main_tip: String },
    NothingToMerge { main_tip: String },
    Parked(String),
}

/// RAII guard for a claimed (`merging`) queue item: re-queues it if the
/// worker dies before recording a terminal outcome, so no row can wedge in
/// `merging` mid-session.
struct MergeClaim {
    db: Db,
    task_id: String,
    done: bool,
}

impl Drop for MergeClaim {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.db.lock_safe().wt_enqueue(&self.task_id);
        }
    }
}

/// Agent-state handle used to resolve the AI commit-message config
/// (Settings → Source Control) when a merge lands. Registered once from
/// `AppState::new`; the config is read live on every merge.
static COMMIT_MSG_AGENT: std::sync::OnceLock<Arc<Mutex<crate::state::AgentState>>> =
    std::sync::OnceLock::new();

/// Register the agent-state handle backing AI commit-message generation for
/// landed merges. First registration wins.
pub(crate) fn set_commit_message_source(agent: Arc<Mutex<crate::state::AgentState>>) {
    let _ = COMMIT_MSG_AGENT.set(agent);
}

/// Best-effort AI commit message for a landing squash: diffs the worktree's
/// delta from its fork point and routes it through the shared generator
/// using the model configured in Settings → Source Control. Returns `None`
/// (caller falls back to the title-based message) when no model is
/// configured or anything fails; capped at 60s so a hung provider can't
/// stall the merge queue.
async fn ai_commit_message(row: &TaskWorktreeRow) -> Option<String> {
    let agent = COMMIT_MSG_AGENT.get()?;
    let req = {
        let agent = agent.lock_safe();
        let cfg = agent.ai_config.source_control.clone()?;
        let entry = agent.ai_config.find_by_key(&cfg.provider_key)?;
        rustic_agent::commit_message::CommitMessageRequest {
            provider_key: entry.provider_key(),
            model: cfg.model.clone(),
            api_key: entry.api_key.clone(),
            base_url: entry.base_url.clone(),
            capabilities: agent.ai_config.capabilities_for(&cfg.model),
            allowed_providers: agent.ai_config.allowed_providers_for(&cfg.model),
        }
    };
    let (path, base) = (row.worktree_path.clone(), row.base_oid.clone());
    let diff = tokio::task::spawn_blocking(move || {
        let wt = GitRepo::open(Path::new(&path)).ok()?;
        wt.diff_unified(&base, "HEAD").ok()
    })
    .await
    .ok()
    .flatten()?;
    if diff.trim().is_empty() {
        return None;
    }
    let generate = rustic_agent::commit_message::generate_commit_message(req, diff);
    match tokio::time::timeout(std::time::Duration::from_secs(60), generate).await {
        Ok(Ok(msg)) => Some(msg),
        Ok(Err(e)) => {
            tracing::warn!(%e, "merge queue: AI commit message failed; using fallback");
            None
        }
        Err(_) => {
            tracing::warn!("merge queue: AI commit message timed out; using fallback");
            None
        }
    }
}

async fn process_queue_item(db: &Db, emitter: &Arc<dyn EventEmitter>, row: TaskWorktreeRow) {
    let task_id = row.task_id.clone();
    {
        let db = db.lock_safe();
        match db.wt_try_start_merging(&task_id) {
            Ok(true) => {}
            _ => return,
        }
    }
    let mut claim = MergeClaim {
        db: db.clone(),
        task_id: task_id.clone(),
        done: false,
    };
    emit_current(db, emitter.as_ref(), &task_id);

    let title = db
        .lock_safe()
        .get_task(&task_id)
        .ok()
        .flatten()
        .map(|t| t.title)
        .unwrap_or_default();
    let short = &task_id[..task_id.len().min(8)];
    let fallback_msg = if title.is_empty() || title == "New Task" {
        format!("Rustic task {short}")
    } else {
        format!("{title} (rustic task {short})")
    };
    let commit_msg = ai_commit_message(&row).await.unwrap_or(fallback_msg);

    let mut parked: Option<String> = None;
    // Some(oid): a real squash commit landed on the base branch (toast-worthy).
    // The worktree is KEPT either way — the task keeps working on the same
    // branch, which the squash left exactly at the landed commit.
    let mut landed: Option<String> = None;
    let mut resynced: Option<String> = None;
    // Set when a spawn_blocking phase was CANCELLED (runtime dropped it, e.g.
    // hot-reload / shutdown) rather than panicking. A cancel is not a merge
    // failure — re-queue so the item retries instead of parking with a
    // confusing "panicked" reason or wedging in `merging`.
    let mut cancelled = false;

    for _attempt in 0..3 {
        let prep_row = row.clone();
        let msg = commit_msg.clone();
        let prep = match tokio::task::spawn_blocking(move || prepare_branch(&prep_row, &msg)).await
        {
            Ok(o) => o,
            Err(e) if e.is_cancelled() => {
                cancelled = true;
                break;
            }
            Err(e) => PrepOutcome::Parked(format!("merge worker panicked: {e}")),
        };

        let (new_tip, main_tip) = match prep {
            PrepOutcome::Parked(reason) => {
                parked = Some(reason);
                break;
            }
            PrepOutcome::NothingToMerge { main_tip } => {
                resynced = Some(main_tip);
                break;
            }
            PrepOutcome::Prepared { new_tip, main_tip } => (new_tip, main_tip),
        };

        if let Err(output) = run_validation(db, &row).await {
            parked = Some(format!("validation failed:\n{output}"));
            break;
        }

        let land_row = row.clone();
        let (tip, base) = (new_tip.clone(), main_tip.clone());
        let land =
            match tokio::task::spawn_blocking(move || land_branch(&land_row, &tip, &base)).await {
                Ok(r) => r,
                Err(e) if e.is_cancelled() => {
                    cancelled = true;
                    break;
                }
                Err(e) => Err(format!("merge worker panicked: {e}")),
            };
        match land {
            Ok(true) => {
                landed = Some(new_tip);
                break;
            }
            Ok(false) => continue,
            Err(e) => {
                parked = Some(e);
                break;
            }
        }
    }

    if cancelled {
        claim.done = true;
        let _ = db.lock_safe().wt_enqueue(&task_id);
        emit_current(db, emitter.as_ref(), &task_id);
        return;
    }

    if let Some(merged_oid) = landed {
        let _ = db.lock_safe().wt_record_merge(&task_id, &merged_oid);
    } else if let Some(base_oid) = resynced {
        let _ = db.lock_safe().wt_reset_active(&task_id, &base_oid);
    } else {
        let reason = parked.unwrap_or_else(|| {
            "merge did not converge after 3 attempts (base branch kept moving)".into()
        });
        let _ = db.lock_safe().wt_park(&task_id, &reason);
    }
    claim.done = true;
    emit_current(db, emitter.as_ref(), &task_id);
}

/// Blocking phase 1: bring the task branch up to date with the base branch
/// and squash it to a single commit. Leaves conflicted rebases mid-flight.
fn prepare_branch(row: &TaskWorktreeRow, commit_msg: &str) -> PrepOutcome {
    let wt_path = Path::new(&row.worktree_path);
    if !wt_path.exists() {
        return PrepOutcome::Parked("worktree directory is missing".into());
    }
    let wt = match GitRepo::open(wt_path) {
        Ok(r) => r,
        Err(e) => return PrepOutcome::Parked(format!("cannot open worktree: {e}")),
    };
    let main = match GitRepo::open(Path::new(&row.project_root)) {
        Ok(r) => r,
        Err(e) => return PrepOutcome::Parked(format!("cannot open main repo: {e}")),
    };

    if wt.rebase_in_progress().unwrap_or(false) {
        if wt.has_conflicts().unwrap_or(true) {
            return PrepOutcome::Parked(park_conflicts(&wt, "rebase paused on conflicts"));
        }
        if let Err(e) = wt.rebase_continue() {
            return PrepOutcome::Parked(park_conflicts(
                &wt,
                &format!("rebase --continue failed: {e}"),
            ));
        }
    }

    if let Ok(status) = wt.status_limited(Some(1)) {
        if !status.files.is_empty() {
            let _ = wt.stage_all();
            if let Err(e) = wt.commit("rustic checkpoint: pre-merge") {
                return PrepOutcome::Parked(format!("could not commit pending changes: {e}"));
            }
        }
    }

    let main_tip = match main.rev_parse(&row.base_branch) {
        Ok(oid) => oid,
        Err(e) => {
            return PrepOutcome::Parked(format!(
                "cannot resolve base branch '{}': {e}",
                row.base_branch
            ))
        }
    };

    // Rebase unconditionally: for a branch with no new work this is a plain
    // fast-forward that keeps the kept-alive worktree in sync with the base.
    if let Err(e) = wt.rebase(&row.base_branch) {
        if wt.has_conflicts().unwrap_or(false) {
            return PrepOutcome::Parked(park_conflicts(&wt, "rebase onto base branch conflicted"));
        }
        let _ = wt.rebase_abort();
        return PrepOutcome::Parked(format!("rebase failed: {e}"));
    }

    let rebased_tip = match wt.rev_parse("HEAD") {
        Ok(oid) => oid,
        Err(e) => return PrepOutcome::Parked(format!("cannot resolve rebased tip: {e}")),
    };
    if rebased_tip == main_tip {
        return PrepOutcome::NothingToMerge { main_tip };
    }

    // Net-zero branch (e.g. the user reverted the whole task): the rebased
    // commits leave the tree byte-identical to the base tip. Squashing would
    // die on "nothing to commit" and park the queue — drop the no-op commits
    // and report nothing to merge instead.
    let trees_equal = match (
        wt.rev_parse(&format!("{rebased_tip}^{{tree}}")),
        wt.rev_parse(&format!("{main_tip}^{{tree}}")),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if trees_equal {
        if let Err(e) = wt.reset_hard(&main_tip) {
            return PrepOutcome::Parked(format!("net-zero branch reset failed: {e}"));
        }
        return PrepOutcome::NothingToMerge { main_tip };
    }

    let new_tip = match wt.squash_to_one(&main_tip, commit_msg) {
        Ok(oid) => oid,
        Err(e) => return PrepOutcome::Parked(format!("squash failed: {e}")),
    };

    PrepOutcome::Prepared { new_tip, main_tip }
}

fn park_conflicts(wt: &GitRepo, prefix: &str) -> String {
    let files = wt
        .get_conflicts()
        .map(|cs| cs.iter().map(|c| c.path.clone()).collect::<Vec<_>>())
        .unwrap_or_default();
    if files.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {}", files.join(", "))
    }
}

/// Blocking phase 2: land `new_tip` on the base branch. Returns Ok(true) on
/// success, Ok(false) when the base branch moved (caller retries), Err to park.
fn land_branch(
    row: &TaskWorktreeRow,
    new_tip: &str,
    expected_main_tip: &str,
) -> Result<bool, String> {
    let main = GitRepo::open(Path::new(&row.project_root))
        .map_err(|e| format!("cannot open main repo: {e}"))?;

    let current_tip = main
        .rev_parse(&row.base_branch)
        .map_err(|e| format!("cannot resolve base branch: {e}"))?;
    if current_tip != expected_main_tip {
        return Ok(false);
    }

    let head_is_base = main
        .head_branch()
        .map(|b| b == row.base_branch)
        .unwrap_or(false);

    let result: Result<(), String> = if head_is_base {
        match main.merge_ff_only(new_tip) {
            Ok(()) => Ok(()),
            Err(ff_err) => absorb_dirty_land(&main, row, new_tip, expected_main_tip)
                .map_err(|e| format!("{e} (fast-forward: {ff_err})")),
        }
    } else {
        main.update_branch_ref(&row.base_branch, new_tip, Some(expected_main_tip))
            .map_err(|e| format!("fast-forward failed: {e}"))
    };

    match result {
        Ok(()) => Ok(true),
        Err(e) => {
            let moved = main
                .rev_parse(&row.base_branch)
                .map(|tip| tip != expected_main_tip)
                .unwrap_or(false);
            if moved {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// Land `new_tip` on a checked-out base branch whose working tree is dirty.
/// Every path the landing changes must be either clean, already holding
/// exactly the landing content, or still holding the SEEDED baseline the
/// task's worktree started from (seed manifest) — the task deliberately
/// changed those, so they are overwritten. Any real divergence (the user
/// edited a file after seeding) aborts before the ref moves. Paths from git
/// are repo-relative, so filesystem joins use the repo work dir.
fn absorb_dirty_land(
    main: &GitRepo,
    row: &TaskWorktreeRow,
    new_tip: &str,
    expected_main_tip: &str,
) -> Result<(), String> {
    let root = main
        .work_dir()
        .map_err(|e| format!("cannot resolve repo work dir: {e}"))?;
    let changed = main
        .changed_paths_status(expected_main_tip, new_tip)
        .map_err(|e| format!("cannot diff landing commit: {e}"))?;
    let dirty: std::collections::HashSet<String> = main
        .status()
        .map_err(|e| format!("cannot read main checkout status: {e}"))?
        .files
        .into_iter()
        .map(|f| f.path)
        .collect();
    let seed = seed_manifest(&row.worktree_path);

    let mut mismatched: Vec<String> = Vec::new();
    let mut restore: Vec<String> = Vec::new();
    let mut deletes: Vec<String> = Vec::new();
    for (st, path) in &changed {
        let abs = root.join(path);
        let is_dirty = dirty.contains(path);
        if *st == 'D' {
            if !is_dirty {
                deletes.push(path.clone());
                continue;
            }
            if !abs.exists() {
                continue;
            }
            let wt_hash = main.hash_object(path).ok();
            if abs.is_file()
                && wt_hash.is_some()
                && wt_hash.as_deref() == seed.get(path).map(String::as_str)
            {
                deletes.push(path.clone());
            } else {
                mismatched.push(path.clone());
            }
        } else {
            if !is_dirty {
                restore.push(path.clone());
                continue;
            }
            if !abs.is_file() {
                mismatched.push(path.clone());
                continue;
            }
            let wt_hash = main.hash_object(path).ok();
            if wt_hash.is_none() {
                mismatched.push(path.clone());
                continue;
            }
            if wt_hash == main.rev_parse(&format!("{new_tip}:{path}")).ok() {
                continue;
            }
            if wt_hash.as_deref() == seed.get(path).map(String::as_str) {
                restore.push(path.clone());
            } else {
                mismatched.push(path.clone());
            }
        }
    }
    if !mismatched.is_empty() {
        return Err(format!(
            "cannot land: uncommitted files in the main checkout differ from what this merge \
             writes — {}. Commit, stash, or revert them, then re-queue.",
            mismatched.join(", ")
        ));
    }

    main.update_branch_ref(&row.base_branch, new_tip, Some(expected_main_tip))
        .map_err(|e| format!("ref update failed: {e}"))?;
    if let Err(e) = main.reset_mixed("HEAD") {
        tracing::warn!(%e, "absorb land: index reset failed");
    }
    if let Err(e) = main.checkout_paths_from_index(&restore) {
        tracing::warn!(%e, "absorb land: working-tree restore failed");
    }
    for p in &deletes {
        let _ = std::fs::remove_file(root.join(p));
    }
    Ok(())
}

/// Load the seed manifest written at worktree creation (repo-relative path →
/// seeded blob hash); empty when absent.
fn seed_manifest(worktree_path: &str) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(Path::new(worktree_path).join(".rustic/seed-manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Run the configured validation command inside the worktree. Ok(()) when no
/// command is configured or it exits 0; Err(tail-of-output) otherwise.
async fn run_validation(db: &Db, row: &TaskWorktreeRow) -> Result<(), String> {
    let (cmd, timeout_secs) = {
        let db = db.lock_safe();
        let global = db
            .get_setting(VALIDATION_CMD_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<String>(&v).ok().or(Some(v)))
            .unwrap_or_default();
        let per_project: std::collections::HashMap<String, String> = db
            .get_setting(PROJECT_VALIDATION_CMDS_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let cmd = per_project
            .get(&row.project_id)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(global);
        let timeout = db
            .get_setting(VALIDATION_TIMEOUT_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<u64>(&v).ok())
            .unwrap_or(DEFAULT_VALIDATION_TIMEOUT_SECS);
        (cmd.trim().to_string(), timeout)
    };
    if cmd.is_empty() {
        return Ok(());
    }

    let mut command = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", &cmd]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", &cmd]);
        c
    };
    command.current_dir(&row.worktree_path);
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output(),
    )
    .await
    .map_err(|_| format!("validation command timed out after {timeout_secs}s: {cmd}"))?
    .map_err(|e| format!("validation command failed to start: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail: String = text
        .chars()
        .rev()
        .take(4000)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Err(format!("`{cmd}` exited with {}\n{tail}", output.status))
}
