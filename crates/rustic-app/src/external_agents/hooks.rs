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
//!    git — never the global `~/.codex/hooks.json`. Antigravity likewise gets
//!    `<project>/.agents/hooks.json`, and only when the project has no hooks
//!    file of its own.
//!
//! On CLI versions the extra events were verified against, the same sink also
//! receives tool-call, approval and turn-end events, which feed the activity
//! list (see [`super::activity`]).
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

use super::launcher::parse_version;
use super::AgentKind;

/// Rustic-owned locations backing external-agent hook capture.
#[derive(Debug, Clone)]
pub struct HookPaths {
    /// Root of the Rustic-owned hook config, under app data.
    pub root: PathBuf,
    /// Directory hook invocations drop their payload files into.
    pub spool: PathBuf,
    /// Settings file handed to `claude --settings`.
    pub claude_settings: PathBuf,
    /// Directory holding one activity log per session (see [`super::activity`]).
    pub activity: PathBuf,
    /// Directory holding the sink scripts. Equal to `root` unless that path
    /// contains a space (see the module docs).
    pub script_dir: PathBuf,
}

/// Where the per-session activity logs live, without creating anything.
pub fn activity_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("external-agents").join("activity")
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
            activity: activity_dir(app_data_dir),
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
                    // Write under another extension, then rename: tool payloads
                    // can be large, and the drain must never read half of one.
                    "f=\"$spool/$RUSTIC_PTY_ID-$$-$(date +%s)\"\n",
                    "cat > \"$f.part\" && mv \"$f.part\" \"$f.json\"\n",
                ),
            )?;
        }
        Ok(())
    }
}

/// True when `path` contains only characters every shell passes through
/// unquoted and uninterpreted (no spaces, quotes, `&`, `;`, `$`, parens…).
fn is_shell_safe(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '\\' | ':' | '.' | '_' | '-'))
}

/// POSIX single-quote `s` so any byte survives `sh` unchanged.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Pick a directory for the sink scripts whose path a shell can execute
/// unquoted. Falls back to `%ProgramData%` when app data contains a space or
/// shell metacharacter, and to the app-data root itself if even that is unusable.
fn script_dir_for(root: &Path) -> PathBuf {
    if !cfg!(windows) || is_shell_safe(root) {
        return root.to_path_buf();
    }
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let fallback = base.join("rustic").join("external-agents");
    if !is_shell_safe(&fallback) || std::fs::create_dir_all(&fallback).is_err() {
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
            "sh {} {}",
            sh_quote(&script),
            sh_quote(&paths.spool.to_string_lossy())
        )
    }
}

/// Whether the installed CLI is new enough for the activity hooks (tool calls,
/// approvals, turn end) on top of the two session hooks.
///
/// The floor is the release each hook set was verified against, not the release
/// that introduced it. Guessing lower is not safe: Claude Code blocks startup on
/// a dialog when it meets an event name it doesn't know, and how an older Codex
/// or Antigravity reacts is untested. An unknown version counts as too old.
pub fn supports_activity_hooks(agent: AgentKind, version: Option<&str>) -> bool {
    // On Windows every hook starts a PowerShell process, and the tool events
    // fire on each tool call. That cost has not been measured there, so only
    // the two session hooks are registered.
    if cfg!(windows) {
        return false;
    }
    let floor = match agent {
        AgentKind::Claude => (2, 1, 325),
        AgentKind::Codex => (0, 170, 0),
        AgentKind::Antigravity => (1, 5, 11),
    };
    version
        .and_then(parse_version)
        .is_some_and(|found| found >= floor)
}

/// Hook events registered for Claude Code or Codex, with whether each one is a
/// per-tool event.
fn hook_events(agent: AgentKind, activity: bool) -> Vec<(&'static str, bool)> {
    let mut events = vec![("SessionStart", false), ("UserPromptSubmit", false)];
    if activity {
        events.extend([
            ("PreToolUse", true),
            ("PostToolUse", true),
            ("PermissionRequest", true),
            ("Stop", false),
        ]);
        if matches!(agent, AgentKind::Claude) {
            events.push(("PostToolUseFailure", true));
        }
    }
    events
}

/// Build the `{ "hooks": { … } }` object Claude Code and Codex accept, wiring
/// each event to the spool sink.
fn hooks_json(paths: &HookPaths, agent: AgentKind, activity: bool) -> serde_json::Value {
    let command = serde_json::json!({ "type": "command", "command": sink_command(paths) });
    let mut hooks = serde_json::Map::new();
    for (event, per_tool) in hook_events(agent, activity) {
        // Claude Code's tool events take a matcher; an empty one means every tool.
        let entry = if per_tool && matches!(agent, AgentKind::Claude) {
            serde_json::json!([{ "matcher": "", "hooks": [command] }])
        } else {
            serde_json::json!([{ "hooks": [command] }])
        };
        hooks.insert(event.to_string(), entry);
    }
    serde_json::json!({ "hooks": hooks })
}

/// Write the settings file passed to `claude --settings`. Rewritten on every
/// spawn so a moved app-data directory can't leave a stale sink path behind.
pub fn write_claude_settings(paths: &HookPaths, activity: bool) -> io::Result<()> {
    std::fs::create_dir_all(&paths.root)?;
    let body = serde_json::to_vec_pretty(&hooks_json(paths, AgentKind::Claude, activity))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&paths.claude_settings, body)
}

/// Write `<project>/.codex/hooks.json` — Codex's only project-scoped hook
/// location — and keep it out of git.
pub fn write_codex_hooks(project_root: &Path, paths: &HookPaths, activity: bool) -> io::Result<()> {
    let dir = project_root.join(".codex");
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_vec_pretty(&hooks_json(paths, AgentKind::Codex, activity))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join("hooks.json"), body)?;
    exclude_from_git(project_root, "/.codex/");
    Ok(())
}

/// Name of Rustic's entry in Antigravity's `hooks.json`.
const ANTIGRAVITY_HOOK_NAME: &str = "rustic";

/// Antigravity's hook file body: one named hook wiring the spool sink.
///
/// `PreToolUse` is left out on purpose. It is Antigravity's permission gate and
/// expects a decision on stdout, which a silent sink can't give; `PostToolUse`
/// reports the same call once it has run.
fn antigravity_hooks_json(paths: &HookPaths) -> serde_json::Value {
    let command = serde_json::json!({ "type": "command", "command": sink_command(paths) });
    serde_json::json!({
        ANTIGRAVITY_HOOK_NAME: {
            "SessionStart": [command],
            "PreInvocation": [command],
            "PostToolUse": [{ "matcher": "*", "hooks": [command] }],
            "Stop": [command],
        }
    })
}

/// Write `<project>/.agents/hooks.json` for Antigravity and keep it out of git.
///
/// Returns `false` without touching anything when the project already has a
/// hooks file with entries of its own: that file belongs to the user (and may be
/// committed), so Rustic's activity list simply stays empty there.
pub fn write_antigravity_hooks(project_root: &Path, paths: &HookPaths) -> io::Result<bool> {
    let dir = project_root.join(".agents");
    let file = dir.join("hooks.json");
    if let Ok(existing) = std::fs::read_to_string(&file) {
        let ours_only = serde_json::from_str::<serde_json::Value>(&existing)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .is_some_and(|map| map.keys().all(|k| k == ANTIGRAVITY_HOOK_NAME));
        if !ours_only {
            return Ok(false);
        }
    }
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_vec_pretty(&antigravity_hooks_json(paths))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&file, body)?;
    exclude_from_git(project_root, "/.agents/hooks.json");
    Ok(true)
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
    // Antigravity writes its payload keys in camelCase and calls a session a
    // conversation.
    #[serde(default, alias = "conversationId")]
    pub session_id: Option<String>,
    #[serde(default, alias = "transcriptPath")]
    pub transcript_path: Option<String>,
    #[serde(default, alias = "hookEventName")]
    pub hook_event_name: Option<String>,
    // Antigravity has no prompt event; each payload repeats the turn's prompt.
    #[serde(default, alias = "lastUserInput")]
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
        let v = hooks_json(&paths, AgentKind::Claude, false);
        let hooks = &v["hooks"];
        assert_eq!(hooks.as_object().unwrap().len(), 2, "session hooks only");
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

    #[test]
    fn activity_hooks_add_tool_approval_and_stop_events_per_cli() {
        let (tmp, paths) = probe_paths();
        let claude = hooks_json(&paths, AgentKind::Claude, true);
        for event in [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "Stop",
        ] {
            assert!(claude["hooks"][event].is_array(), "claude missing {event}");
        }
        assert_eq!(claude["hooks"]["PreToolUse"][0]["matcher"], "");
        assert!(claude["hooks"]["Stop"][0].get("matcher").is_none());

        let codex = hooks_json(&paths, AgentKind::Codex, true);
        assert!(codex["hooks"]["PermissionRequest"].is_array());
        assert!(codex["hooks"]["PreToolUse"][0].get("matcher").is_none());
        assert!(
            codex["hooks"].get("PostToolUseFailure").is_none(),
            "Codex has no such event"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn activity_hooks_need_a_verified_cli_version() {
        assert!(supports_activity_hooks(AgentKind::Claude, Some("2.1.325")));
        assert!(supports_activity_hooks(AgentKind::Claude, Some("2.2.0")));
        assert!(!supports_activity_hooks(AgentKind::Claude, Some("2.1.324")));
        assert!(supports_activity_hooks(AgentKind::Codex, Some("0.170.0")));
        assert!(!supports_activity_hooks(AgentKind::Codex, Some("0.125.0")));
        assert!(supports_activity_hooks(
            AgentKind::Antigravity,
            Some("1.5.11")
        ));
        assert!(!supports_activity_hooks(AgentKind::Antigravity, None));
    }

    #[test]
    fn antigravity_hooks_never_gate_tools_and_respect_a_user_file() {
        let (tmp, paths) = probe_paths();
        let project = tmp.join("project");
        std::fs::create_dir_all(project.join(".git").join("info")).unwrap();

        assert!(write_antigravity_hooks(&project, &paths).unwrap());
        let file = project.join(".agents").join("hooks.json");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let ours = &v[ANTIGRAVITY_HOOK_NAME];
        assert!(ours.get("PreToolUse").is_none(), "must not gate tool calls");
        assert_eq!(ours["PostToolUse"][0]["matcher"], "*");
        assert!(ours["PreInvocation"][0]["command"].is_string());
        let exclude = std::fs::read_to_string(project.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/.agents/hooks.json"));

        // Our own file is refreshed on the next spawn.
        assert!(write_antigravity_hooks(&project, &paths).unwrap());

        // A file with the user's own hooks is left exactly as it is.
        let user = r#"{"lint":{"PostToolUse":[]}}"#;
        std::fs::write(&file, user).unwrap();
        assert!(!write_antigravity_hooks(&project, &paths).unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), user);
        std::fs::write(&file, "not json").unwrap();
        assert!(!write_antigravity_hooks(&project, &paths).unwrap());
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
    fn shell_metacharacters_force_a_safe_script_dir_and_quote_cleanly() {
        assert!(is_shell_safe(Path::new(
            r"C:\Users\ada\AppData\external-agents"
        )));
        for bad in [
            r"C:\Users\A&B\x",
            r"C:\Users\a;b\x",
            r"C:\Users\$x\x",
            r"C:\Users\a(1)\x",
        ] {
            assert!(!is_shell_safe(Path::new(bad)), "{bad}");
        }
        assert_eq!(sh_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
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
