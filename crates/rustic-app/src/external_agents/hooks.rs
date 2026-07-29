//! Hook configuration for the external CLI agents.
//!
//! Claude Code and Codex both support *command* hooks — local processes they
//! run on session start and on prompt submit. That is how Rustic learns a
//! conversation's id (needed to resume it later) and its first prompt (used as
//! the display title) without ever reading their internal transcript formats.
//!
//! Two hard constraints shape this module:
//!
//! 1. **A hook must print nothing.** `UserPromptSubmit` stdout is injected into
//!    the model's context, so any output would corrupt the user's conversation
//!    and burn their plan tokens.
//! 2. **Rustic must not mutate the user's own CLI config.** Claude gets a
//!    Rustic-owned settings file passed via `--settings` (it merges with, and
//!    cannot clobber, the user's settings). Codex has no such flag, so its
//!    hooks go in a project-scoped `.codex/hooks.json` that Rustic keeps out of
//!    git — never the global `~/.codex/hooks.json`.
//!
//! The hook itself copies its stdin into a spool directory, one file per event.
//! A directory (instead of one appended log) means parallel sessions can never
//! interleave writes. The correlation token lives in the *filename*, taken from
//! the `RUSTIC_PTY_ID` the PTY exported, because the payload JSON has no field
//! of ours to carry it.
//!
//! ## Why the hook is a script file, not a one-liner
//! Rustic does not get to choose the shell a CLI runs its hooks in, and the
//! candidates disagree about everything that matters. Claude Code resolves
//! `bash` off `PATH` — in practice Git Bash, Cygwin, MobaXterm or WSL, all
//! reporting as `/usr/bin/bash` — and a POSIX shell treats `\` as an escape, so
//! a native Windows path silently collapses into gibberish
//! (`C:UsersuserAppData…`, the original bug). Other tools reach for `cmd.exe` or
//! PowerShell, where an outer PowerShell expands `$env:…` before the inner
//! command sees it and 5.1 strips embedded quotes from a native command line.
//!
//! So all the logic lives in a Rustic-written script, and the hook command is
//! the single invocation every one of those shells runs identically:
//! `powershell.exe … -File <forward/slashed/script> <forward/slashed/spool>`.
//! It carries no quotes, no `$`, and no backslashes; `.exe` is explicit because
//! a WSL bash won't resolve a bare Windows program name, and the spool is an
//! *argument* so nothing depends on an environment variable surviving the
//! crossing. Since it cannot be quoted, no path in it may contain a space —
//! hence the move to a space-free root (scripts *and* spool) when app data sits
//! under a user name with a space in it.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Rustic-owned locations backing external-agent hook capture.
#[derive(Debug, Clone)]
pub struct HookPaths {
    /// Root of the Rustic-owned hook config, under app data.
    pub root: PathBuf,
    /// Directory hook invocations drop their payload files into.
    pub spool: PathBuf,
    /// Settings file handed to `claude --settings`.
    pub claude_settings: PathBuf,
    /// Directory holding the sink scripts. Equal to `root` unless that path
    /// contains a space (see the module docs).
    pub script_dir: PathBuf,
}

impl HookPaths {
    /// Derive the hook paths from the app-data directory, create them, and
    /// (re)write the sink scripts.
    pub fn ensure(app_data_dir: &Path) -> io::Result<Self> {
        let root = app_data_dir.join("external-agents");
        let script_dir = script_dir_for(&root);
        // The spool lives beside the scripts so it inherits their space-free
        // path and can be passed to the sink as a plain unquoted argument.
        let spool = script_dir.join("spool");
        std::fs::create_dir_all(&spool)?;
        let paths = Self {
            claude_settings: root.join("claude-settings.json"),
            root,
            spool,
            script_dir,
        };
        std::fs::create_dir_all(&paths.root)?;
        paths.write_sink_scripts()?;
        Ok(paths)
    }

    /// The script a hook invocation executes.
    pub fn sink_script(&self) -> PathBuf {
        self.script_dir
            .join(if cfg!(windows) { "sink.ps1" } else { "sink.sh" })
    }

    /// Write the sink script(s). Rewritten on every spawn so a moved app-data
    /// directory or an upgraded script body can't leave a stale copy behind.
    fn write_sink_scripts(&self) -> io::Result<()> {
        if cfg!(windows) {
            // A script that only calls WriteAllText keeps the hook silent:
            // `UserPromptSubmit` stdout is injected into the model's context, so
            // a single stray line would corrupt the conversation.
            std::fs::write(
                self.script_dir.join("sink.ps1"),
                concat!(
                    "param([string]$Spool)\r\n",
                    "$ErrorActionPreference = 'SilentlyContinue'\r\n",
                    "if (-not $Spool) { $Spool = [Environment]::GetEnvironmentVariable('RUSTIC_HOOK_DIR') }\r\n",
                    "if (-not $Spool) { exit 0 }\r\n",
                    "$key = [Environment]::GetEnvironmentVariable('RUSTIC_PTY_ID')\r\n",
                    "if (-not $key) { exit 0 }\r\n",
                    "$name = $key + '-' + [guid]::NewGuid().ToString('N') + '.json'\r\n",
                    "[IO.File]::WriteAllText((Join-Path $Spool $name), [Console]::In.ReadToEnd())\r\n",
                ),
            )?;
        } else {
            std::fs::write(
                self.script_dir.join("sink.sh"),
                concat!(
                    "spool=\"${1:-$RUSTIC_HOOK_DIR}\"\n",
                    "[ -n \"$spool\" ] && [ -n \"$RUSTIC_PTY_ID\" ] || exit 0\n",
                    "cat > \"$spool/$RUSTIC_PTY_ID-$$-$(date +%s).json\"\n",
                ),
            )?;
        }
        Ok(())
    }
}

/// Pick a directory for the sink scripts whose path a shell can execute
/// unquoted. Falls back to `%ProgramData%` when app data contains a space, and
/// to the app-data root itself if even that is unusable.
fn script_dir_for(root: &Path) -> PathBuf {
    if !cfg!(windows) || !root.to_string_lossy().contains(' ') {
        return root.to_path_buf();
    }
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let fallback = base.join("rustic").join("external-agents");
    if fallback.to_string_lossy().contains(' ') || std::fs::create_dir_all(&fallback).is_err() {
        return root.to_path_buf();
    }
    fallback
}

/// The command a hook runs, in the one form every shell a CLI might pick
/// executes identically.
///
/// On Windows the path is written with **forward slashes**: Claude Code invokes
/// hooks through Git Bash (`/usr/bin/bash`), which treats `\` as an escape and
/// silently collapses a native path into gibberish. `powershell.exe` accepts
/// forward slashes, so this one string survives bash, `cmd.exe` and an outer
/// PowerShell alike — provided it stays quote- and space-free, which is what
/// [`script_dir_for`] guarantees.
fn sink_command(paths: &HookPaths) -> String {
    let script = paths.sink_script();
    let script = script.to_string_lossy();
    if cfg!(windows) {
        // `.exe` is explicit because a WSL bash won't resolve a bare Windows
        // program name; the spool is an argument so nothing depends on an
        // environment variable surviving the crossing into another shell.
        format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {} {}",
            script.replace('\\', "/"),
            paths.spool.to_string_lossy().replace('\\', "/")
        )
    } else {
        format!(
            "sh '{}' '{}'",
            script.replace('\'', ""),
            paths.spool.to_string_lossy().replace('\'', "")
        )
    }
}

/// Build the `{ "hooks": { … } }` object both CLIs accept, wiring
/// `SessionStart` and `UserPromptSubmit` to the spool sink.
fn hooks_json(paths: &HookPaths) -> serde_json::Value {
    let entry = serde_json::json!([{
        "hooks": [{ "type": "command", "command": sink_command(paths) }]
    }]);
    serde_json::json!({
        "hooks": {
            "SessionStart": entry,
            "UserPromptSubmit": entry,
        }
    })
}

/// Write the settings file passed to `claude --settings`. Rewritten on every
/// spawn so a moved app-data directory can't leave a stale sink path behind.
pub fn write_claude_settings(paths: &HookPaths) -> io::Result<()> {
    std::fs::create_dir_all(&paths.root)?;
    let body = serde_json::to_vec_pretty(&hooks_json(paths))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&paths.claude_settings, body)
}

/// Write `<project>/.codex/hooks.json` — Codex's only project-scoped hook
/// location — and keep it out of git.
pub fn write_codex_hooks(project_root: &Path, paths: &HookPaths) -> io::Result<()> {
    let dir = project_root.join(".codex");
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_vec_pretty(&hooks_json(paths))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join("hooks.json"), body)?;
    exclude_from_git(project_root, "/.codex/");
    Ok(())
}

/// Add `pattern` to the repo's *local* exclude list. `.git/info/exclude` is
/// deliberate: it is never committed, so Rustic's generated config stays out of
/// both git status and the user's own `.gitignore`.
fn exclude_from_git(project_root: &Path, pattern: &str) {
    let info = project_root.join(".git").join("info");
    if !info.is_dir() {
        return;
    }
    let path = info.join("exclude");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return;
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(pattern);
    next.push('\n');
    let _ = std::fs::write(&path, next);
}

/// The subset of a hook payload Rustic consumes. Both CLIs emit the same field
/// names for these; everything else in the payload is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct HookPayload {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Condense a raw prompt into a one-line display title.
pub fn title_from_prompt(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_string();
    const MAX: usize = 120;
    if line.chars().count() <= MAX {
        return Some(line);
    }
    let truncated: String = line.chars().take(MAX).collect();
    Some(format!("{}…", truncated.trim_end()))
}

/// Recover the `RUSTIC_PTY_ID` a spool file was written for. Filenames are
/// `<ptyKey>-<unique>.json` and the key is a dashless uuid, so the prefix up to
/// the first `-` is unambiguous.
pub fn pty_key_from_spool_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".json")?;
    let key = stem.split('-').next()?;
    if key.len() == 32 && key.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(key.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_paths() -> (PathBuf, HookPaths) {
        let tmp = std::env::temp_dir().join(format!("rustic-hooks-{}", uuid::Uuid::new_v4()));
        let paths = HookPaths::ensure(&tmp).expect("ensure hook paths");
        (tmp, paths)
    }

    #[test]
    fn hooks_json_registers_both_events_with_a_silent_command() {
        let (tmp, paths) = probe_paths();
        let v = hooks_json(&paths);
        let hooks = &v["hooks"];
        for event in ["SessionStart", "UserPromptSubmit"] {
            let cmd = hooks[event][0]["hooks"][0]["command"]
                .as_str()
                .expect("command");
            assert!(!cmd.is_empty());
            assert!(
                !cmd.contains("echo") && !cmd.contains("Write-Host"),
                "hook must never write to stdout: {cmd}"
            );
            assert_eq!(hooks[event][0]["hooks"][0]["type"], "command");
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The hook command has to survive being run by *either* `cmd.exe` or
    /// PowerShell: a `$` would be expanded by an outer PowerShell before the
    /// script ran, and a quote would be stripped from the native command line.
    #[test]
    fn hook_command_is_a_bare_script_path_free_of_shell_syntax() {
        let (tmp, paths) = probe_paths();
        let cmd = sink_command(&paths);
        assert!(
            !cmd.contains('$'),
            "hook command must not contain `$`: {cmd}"
        );
        if cfg!(windows) {
            assert!(!cmd.contains('"') && !cmd.contains('\''), "unquoted: {cmd}");
            // A POSIX bash (Git Bash, Cygwin, MobaXterm) eats `\`, which is what
            // collapsed the path into gibberish before; and because nothing can
            // be quoted, no argument may contain a space.
            assert!(!cmd.contains('\\'), "no backslashes: {cmd}");
            assert!(cmd.starts_with("powershell.exe "), "{cmd}");
            let args: Vec<&str> = cmd.split(' ').collect();
            assert!(args[args.len() - 2].ends_with("sink.ps1"), "{cmd}");
            assert!(args[args.len() - 1].ends_with("/spool"), "{cmd}");
        } else {
            assert!(cmd.starts_with("sh '"), "{cmd}");
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sink_script_is_written_and_reads_the_spool_from_the_environment() {
        let (tmp, paths) = probe_paths();
        let script = paths.sink_script();
        assert!(script.is_file(), "{} missing", script.display());
        let body = std::fs::read_to_string(&script).unwrap();
        assert!(body.contains("RUSTIC_HOOK_DIR") && body.contains("RUSTIC_PTY_ID"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A space in the app-data path makes the unquoted hook command unusable, so
    /// the scripts have to land somewhere else.
    #[test]
    fn scripts_avoid_a_root_with_a_space_on_windows() {
        let dir = script_dir_for(Path::new(r"C:\Users\Ada Lovelace\AppData\external-agents"));
        if cfg!(windows) {
            assert!(!dir.to_string_lossy().contains(' '), "{}", dir.display());
        } else {
            assert!(dir.to_string_lossy().contains(' '));
        }
    }

    #[test]
    fn titles_take_the_first_line_and_truncate() {
        assert_eq!(
            title_from_prompt("\n\n  fix the auth bug  \nmore detail"),
            Some("fix the auth bug".to_string())
        );
        assert_eq!(title_from_prompt("   \n  "), None);
        let long = "x".repeat(200);
        let title = title_from_prompt(&long).unwrap();
        assert_eq!(title.chars().count(), 121);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn spool_names_yield_their_pty_key() {
        let key = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("{key}-{}.json", uuid::Uuid::new_v4().simple());
        assert_eq!(pty_key_from_spool_name(&name), Some(key));
        assert_eq!(pty_key_from_spool_name("not-a-key.json"), None);
        assert_eq!(pty_key_from_spool_name("deadbeef.txt"), None);
    }

    #[test]
    fn git_exclude_is_appended_once() {
        let tmp = std::env::temp_dir().join(format!("rustic-hook-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join(".git").join("info")).unwrap();
        exclude_from_git(&tmp, "/.codex/");
        exclude_from_git(&tmp, "/.codex/");
        let body = std::fs::read_to_string(tmp.join(".git/info/exclude")).unwrap();
        assert_eq!(body.matches("/.codex/").count(), 1);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn payload_parses_both_cli_shapes() {
        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s1","transcript_path":"/t.jsonl","hook_event_name":"SessionStart","cwd":"/w","model":"opus"}"#,
        )
        .unwrap();
        assert_eq!(p.session_id.as_deref(), Some("s1"));
        assert_eq!(p.hook_event_name.as_deref(), Some("SessionStart"));
        assert!(p.prompt.is_none());

        let p: HookPayload = serde_json::from_str(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"hi","turn_id":"t1"}"#,
        )
        .unwrap();
        assert_eq!(p.prompt.as_deref(), Some("hi"));
    }
}
