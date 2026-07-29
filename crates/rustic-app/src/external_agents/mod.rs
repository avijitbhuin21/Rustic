//! Detection and launch planning for the external CLI coding agents
//! (Claude Code, Codex, Antigravity) that Rustic hosts inside a PTY tab.
//!
//! Rustic never drives these tools programmatically — no SDK, no `-p`/`exec`
//! headless mode, no API calls. It only ever *spawns* them and hands the
//! terminal to the user, so every turn is billed against the user's own
//! subscription exactly as if they had typed the command themselves. Resuming
//! is therefore limited to spawn-time flags (`--resume`, `codex resume`,
//! `agy --conversation`).
//!
//! ## Why the CLI is launched through a shell
//! `portable_pty` resolves a bare program name against `PATH` + `PATHEXT` and
//! hands the result straight to `CreateProcessW`, which cannot execute the
//! `.cmd`/`.ps1` shims that npm-installed CLIs actually are. Writing the
//! command line into a normal shell session sidesteps that (the shell resolves
//! shims natively), keeps argument quoting under our control, and leaves the
//! user at a usable prompt when the CLI exits.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod antigravity;
pub mod hooks;
pub mod launcher;
pub mod service;
pub mod spool;

/// Which external CLI agent a session belongs to. The wire value doubles as the
/// `agent` column in `external_agent_sessions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Antigravity,
}

impl AgentKind {
    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity];

    /// Stable identifier used in the DB, the IPC payloads and the frontend.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Antigravity => "agy",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "agy" | "antigravity" => Some(AgentKind::Antigravity),
            _ => None,
        }
    }

    /// Human-readable name for the UI.
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Antigravity => "Antigravity",
        }
    }

    /// Executable base name to look for on `PATH`.
    pub fn program(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Antigravity => "agy",
        }
    }

    /// npm package this CLI publishes under, when it has one. Drives the
    /// "update available" hint; `None` means Rustic can't check (Antigravity
    /// ships through its own installer, not a registry).
    pub fn registry_package(self) -> Option<&'static str> {
        match self {
            AgentKind::Claude => Some("@anthropic-ai/claude-code"),
            AgentKind::Codex => Some("@openai/codex"),
            AgentKind::Antigravity => None,
        }
    }
}

/// One installed CLI agent, as reported to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgent {
    pub agent: String,
    pub label: String,
    /// Absolute path of the copy Rustic will launch — the newest one installed,
    /// which is not necessarily the first one on `PATH`.
    pub path: String,
    /// Version that copy reported, when it could be probed.
    pub version: Option<String>,
    /// Other copies on `PATH` this one takes precedence over. Non-empty means a
    /// duplicate install, which is worth showing: it is the usual reason a CLI
    /// appears to update itself and then come back stale.
    pub shadowed: Vec<ShadowedInstall>,
    /// Newest version on the registry, set only when it is newer than `version`.
    pub latest_version: Option<String>,
}

/// A copy of a CLI that lost the version comparison.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowedInstall {
    pub path: String,
    pub version: Option<String>,
}

/// Locate every supported CLI agent on `PATH`, resolving each to its newest
/// installed copy.
///
/// Blocking: probes each candidate's `--version` (cached against the file's
/// mtime, so repeat calls are cheap and a self-update is noticed immediately).
pub fn detect_agents() -> Vec<DetectedAgent> {
    AgentKind::ALL
        .iter()
        .filter_map(|kind| {
            let resolved = launcher::resolve(kind.program())?;
            if !resolved.shadowed.is_empty() {
                tracing::info!(
                    target: "rustic::external_agents",
                    agent = kind.as_str(),
                    using = %resolved.path.display(),
                    version = resolved.version.as_deref().unwrap_or("unknown"),
                    shadowed = resolved.shadowed.len(),
                    "multiple copies installed — launching the newest"
                );
            }
            Some(DetectedAgent {
                agent: kind.as_str().to_string(),
                label: kind.label().to_string(),
                path: resolved.path.to_string_lossy().into_owned(),
                version: resolved.version,
                shadowed: resolved
                    .shadowed
                    .into_iter()
                    .map(|install| ShadowedInstall {
                        path: install.path.to_string_lossy().into_owned(),
                        version: install.version,
                    })
                    .collect(),
                latest_version: None,
            })
        })
        .collect()
}

/// Fill in `latest_version` for the agents that publish to a registry.
///
/// Best effort and non-fatal: an offline machine, a rate limit or an unreadable
/// response just leaves the hint off. Results are cached for hours, so calling
/// this on every detection is fine.
pub async fn annotate_updates(agents: &mut [DetectedAgent]) {
    for agent in agents.iter_mut() {
        let Some(kind) = AgentKind::from_str(&agent.agent) else {
            continue;
        };
        let (Some(package), Some(installed)) = (kind.registry_package(), agent.version.as_deref())
        else {
            continue;
        };
        let Some(latest) = launcher::latest_published_version(package).await else {
            continue;
        };
        if launcher::is_newer(&latest, installed) {
            agent.latest_version = Some(latest);
        }
    }
}

/// Which shell dialect the launch command line will be written into. Only the
/// quoting rules differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    PowerShell,
    Posix,
}

impl ShellKind {
    /// Infer the dialect from a resolved shell executable path.
    pub fn from_program(program: Option<&str>) -> Self {
        let Some(program) = program else {
            return if cfg!(windows) {
                ShellKind::PowerShell
            } else {
                ShellKind::Posix
            };
        };
        let low = program.to_lowercase();
        let low = Path::new(&low)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(low);
        match low.as_str() {
            "powershell" | "pwsh" => ShellKind::PowerShell,
            _ => ShellKind::Posix,
        }
    }

    /// Quote a single argument so the shell passes it through verbatim.
    pub fn quote(self, arg: &str) -> String {
        match self {
            ShellKind::PowerShell => format!("'{}'", arg.replace('\'', "''")),
            ShellKind::Posix => format!("'{}'", arg.replace('\'', r"'\''")),
        }
    }

    /// Statements that run on the *same input line* as the CLI, so the tab opens
    /// on the agent itself rather than on a shell banner.
    ///
    /// Two jobs: make npm's `.ps1`/`.cmd` shims runnable under a Restricted
    /// execution policy, and erase the screen *and* the scrollback (`\e[3J`)
    /// right before the CLI starts painting. It has to be one line because the
    /// shell echoes each line it receives — clearing from a previous line would
    /// leave the echo of this one behind.
    pub fn launch_prelude(self) -> &'static str {
        match self {
            ShellKind::PowerShell => {
                "Set-ExecutionPolicy -Scope Process Bypass -Force; \
                 [Console]::Write([char]27+'[H'+[char]27+'[2J'+[char]27+'[3J'); "
            }
            ShellKind::Posix => "printf '\\033[H\\033[2J\\033[3J'; ",
        }
    }
}

/// How a session should be started: fresh, or continuing an existing
/// conversation by the CLI's own id.
#[derive(Debug, Clone)]
pub enum LaunchMode {
    New {
        /// Optional first prompt, passed at spawn time so the very first turn
        /// (and therefore the title-capturing hook) fires without the user
        /// having to retype it.
        prompt: Option<String>,
    },
    Resume {
        external_session_id: String,
    },
}

/// How much autonomy an external CLI agent launches with, mapped from Rustic's
/// own permission model (Chat / Edit / Auto, plus the "grant access to all
/// files" toggle). Each CLI expresses this through its own flags; `Edit` is
/// every CLI's interactive default and adds no flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPermissionMode {
    /// Read-only: explore and answer, never edit (Rustic "Chat").
    ReadOnly,
    /// Ask before edits — each CLI's interactive default (Rustic "Edit").
    #[default]
    Edit,
    /// Auto-apply edits without per-action prompts, still sandboxed (Rustic "Auto").
    Auto,
    /// Skip all permission checks / sandbox (Rustic "Auto" + grant-all files).
    Bypass,
}

impl AgentPermissionMode {
    /// Parse the wire value the frontend sends. Anything unrecognised (or
    /// absent) falls back to the safe interactive default, `Edit`.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "read-only" | "readonly" | "chat" => AgentPermissionMode::ReadOnly,
            "auto" => AgentPermissionMode::Auto,
            "bypass" => AgentPermissionMode::Bypass,
            _ => AgentPermissionMode::Edit,
        }
    }
}

/// Everything a host needs to start an external agent in a PTY.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Command line to write into the shell, without the trailing newline.
    pub command_line: String,
    /// Extra environment for the PTY. Inherited by the CLI and by the hook
    /// processes it spawns, which is how a hook callback identifies its tab.
    pub env: Vec<(String, String)>,
    /// Session id Rustic assigned up front, when the CLI supports being told
    /// one. Lets the row be resumable before any hook has fired.
    pub preassigned_session_id: Option<String>,
}

/// Build the command line + PTY environment for one external agent launch.
///
/// `pty_key` is the correlation token written into `RUSTIC_PTY_ID`;
/// `hook_dir` is the spool directory hook callbacks drop their payloads into.
/// `launcher` is the absolute path of the copy to run — pass it so the launch
/// can't land on a stale duplicate the shell happens to find first. `None` falls
/// back to the bare program name.
pub fn build_launch_plan(
    kind: AgentKind,
    mode: &LaunchMode,
    perm: AgentPermissionMode,
    shell: ShellKind,
    pty_key: &str,
    hook_dir: &Path,
    claude_settings_path: Option<&Path>,
    launcher: Option<&Path>,
) -> LaunchPlan {
    let q = |s: &str| shell.quote(s);
    let mut parts: Vec<String> = vec![invocation(kind, shell, launcher)];
    let mut preassigned_session_id = None;

    // Rustic-spawned sessions never run the CLI's own startup update check. The
    // check offers an update, the user takes it, the CLI replaces itself and
    // exits — losing the session. Updating from inside the terminal still works,
    // and `launcher::resolve` guarantees the next launch picks up the new copy.
    if matches!(kind, AgentKind::Codex) {
        parts.push("-c".into());
        parts.push(q("check_for_update_on_startup=false"));
    }

    // Autonomy / permission flags, mapped from Rustic's mode to each CLI's own
    // controls. `Edit` is every CLI's interactive default and adds nothing.
    // Antigravity has no startup read-only flag, so `ReadOnly` there is also a
    // no-op (its default already asks before acting). These are global flags,
    // pushed before any subcommand/prompt so they apply to resume too.
    match (kind, perm) {
        (AgentKind::Claude, AgentPermissionMode::ReadOnly) => {
            parts.push("--permission-mode".into());
            parts.push(q("plan"));
        }
        (AgentKind::Claude, AgentPermissionMode::Auto) => {
            parts.push("--permission-mode".into());
            parts.push(q("acceptEdits"));
        }
        (AgentKind::Claude, AgentPermissionMode::Bypass) => {
            parts.push("--dangerously-skip-permissions".into());
        }
        (AgentKind::Codex, AgentPermissionMode::ReadOnly) => {
            parts.push("-s".into());
            parts.push(q("read-only"));
        }
        (AgentKind::Codex, AgentPermissionMode::Auto) => {
            parts.push("-s".into());
            parts.push(q("workspace-write"));
            parts.push("-a".into());
            parts.push(q("never"));
        }
        (AgentKind::Codex, AgentPermissionMode::Bypass) => {
            parts.push("--dangerously-bypass-approvals-and-sandbox".into());
        }
        (AgentKind::Antigravity, AgentPermissionMode::Auto) => {
            parts.push("--mode=accept-edits".into());
        }
        (AgentKind::Antigravity, AgentPermissionMode::Bypass) => {
            parts.push("--dangerously-skip-permissions".into());
        }
        _ => {}
    }

    match (kind, mode) {
        (AgentKind::Claude, LaunchMode::New { prompt }) => {
            // Rustic picks the session id so the row is resumable immediately,
            // instead of waiting for a hook to disclose it.
            let sid = uuid::Uuid::new_v4().to_string();
            parts.push("--session-id".into());
            parts.push(q(&sid));
            preassigned_session_id = Some(sid);
            if let Some(settings) = claude_settings_path {
                parts.push("--settings".into());
                parts.push(q(&settings.to_string_lossy()));
            }
            if let Some(prompt) = prompt.as_deref().filter(|p| !p.trim().is_empty()) {
                parts.push(q(prompt));
            }
        }
        (
            AgentKind::Claude,
            LaunchMode::Resume {
                external_session_id,
            },
        ) => {
            parts.push("--resume".into());
            parts.push(q(external_session_id));
            if let Some(settings) = claude_settings_path {
                parts.push("--settings".into());
                parts.push(q(&settings.to_string_lossy()));
            }
        }
        (AgentKind::Codex, LaunchMode::New { prompt }) => {
            // Codex hooks only fire once a turn starts, so a spawn-time prompt
            // is what makes a brand-new session discoverable.
            if let Some(prompt) = prompt.as_deref().filter(|p| !p.trim().is_empty()) {
                parts.push(q(prompt));
            }
        }
        (
            AgentKind::Codex,
            LaunchMode::Resume {
                external_session_id,
            },
        ) => {
            parts.push("resume".into());
            parts.push(q(external_session_id));
        }
        (AgentKind::Antigravity, LaunchMode::New { prompt }) => {
            if let Some(prompt) = prompt.as_deref().filter(|p| !p.trim().is_empty()) {
                parts.push("-i".into());
                parts.push(q(prompt));
            }
        }
        (
            AgentKind::Antigravity,
            LaunchMode::Resume {
                external_session_id,
            },
        ) => {
            parts.push("--conversation".into());
            parts.push(q(external_session_id));
        }
    }

    let mut env = vec![
        ("RUSTIC_PTY_ID".to_string(), pty_key.to_string()),
        (
            "RUSTIC_HOOK_DIR".to_string(),
            hook_dir.to_string_lossy().into_owned(),
        ),
        ("RUSTIC_AGENT_KIND".to_string(), kind.as_str().to_string()),
    ];
    // Claude Code's equivalent of Codex's `-c` override. Only the background
    // check is disabled — an explicit `claude update` still works.
    if matches!(kind, AgentKind::Claude) {
        env.push(("DISABLE_AUTOUPDATER".to_string(), "1".to_string()));
    }

    LaunchPlan {
        command_line: parts.join(" "),
        env,
        preassigned_session_id,
    }
}

/// How the CLI is named on the command line.
///
/// An absolute path has to be quoted, and PowerShell won't execute a quoted
/// string without the call operator.
fn invocation(kind: AgentKind, shell: ShellKind, launcher: Option<&Path>) -> String {
    let Some(path) = launcher else {
        return kind.program().to_string();
    };
    let quoted = shell.quote(&path.to_string_lossy());
    match shell {
        ShellKind::PowerShell => format!("& {quoted}"),
        ShellKind::Posix => quoted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: AgentKind, mode: LaunchMode, shell: ShellKind) -> LaunchPlan {
        build_launch_plan(
            kind,
            &mode,
            AgentPermissionMode::Edit,
            shell,
            "pty-1",
            Path::new("/spool"),
            Some(Path::new("/cfg/claude.json")),
            None,
        )
    }

    fn plan_perm(kind: AgentKind, perm: AgentPermissionMode) -> LaunchPlan {
        build_launch_plan(
            kind,
            &LaunchMode::New { prompt: None },
            perm,
            ShellKind::Posix,
            "pty-1",
            Path::new("/spool"),
            None,
            None,
        )
    }

    fn plan_with_launcher(kind: AgentKind, shell: ShellKind, launcher: &str) -> LaunchPlan {
        build_launch_plan(
            kind,
            &LaunchMode::New { prompt: None },
            AgentPermissionMode::Edit,
            shell,
            "pty-1",
            Path::new("/spool"),
            None,
            Some(Path::new(launcher)),
        )
    }

    #[test]
    fn permission_mode_maps_to_each_cli_flags() {
        // Edit is every CLI's interactive default and adds no permission flag
        // (Claude still preassigns a session id, so it isn't bare "claude").
        let claude_edit = plan_perm(AgentKind::Claude, AgentPermissionMode::Edit).command_line;
        assert!(
            !claude_edit.contains("--permission-mode") && !claude_edit.contains("dangerously"),
            "unexpected perm flag: {claude_edit}"
        );
        // Claude: plan / acceptEdits / dangerous bypass.
        assert!(plan_perm(AgentKind::Claude, AgentPermissionMode::ReadOnly)
            .command_line
            .contains("--permission-mode 'plan'"));
        assert!(plan_perm(AgentKind::Claude, AgentPermissionMode::Auto)
            .command_line
            .contains("--permission-mode 'acceptEdits'"));
        assert!(plan_perm(AgentKind::Claude, AgentPermissionMode::Bypass)
            .command_line
            .contains("--dangerously-skip-permissions"));
        // Codex: sandbox + approval policy, or full bypass. The startup
        // update-check override is always present and precedes these.
        assert!(plan_perm(AgentKind::Codex, AgentPermissionMode::ReadOnly)
            .command_line
            .contains("-s 'read-only'"));
        let codex_auto = plan_perm(AgentKind::Codex, AgentPermissionMode::Auto).command_line;
        assert!(codex_auto.contains("-s 'workspace-write'") && codex_auto.contains("-a 'never'"));
        assert!(plan_perm(AgentKind::Codex, AgentPermissionMode::Bypass)
            .command_line
            .contains("--dangerously-bypass-approvals-and-sandbox"));
        // Antigravity: no read-only flag exists, so ReadOnly is a no-op.
        assert_eq!(
            plan_perm(AgentKind::Antigravity, AgentPermissionMode::ReadOnly).command_line,
            "agy"
        );
        assert!(plan_perm(AgentKind::Antigravity, AgentPermissionMode::Auto)
            .command_line
            .contains("--mode=accept-edits"));
        assert!(
            plan_perm(AgentKind::Antigravity, AgentPermissionMode::Bypass)
                .command_line
                .contains("--dangerously-skip-permissions")
        );
    }

    #[test]
    fn wire_values_parse_to_modes() {
        assert_eq!(
            AgentPermissionMode::from_wire("read-only"),
            AgentPermissionMode::ReadOnly
        );
        assert_eq!(
            AgentPermissionMode::from_wire("auto"),
            AgentPermissionMode::Auto
        );
        assert_eq!(
            AgentPermissionMode::from_wire("bypass"),
            AgentPermissionMode::Bypass
        );
        // Unknown / edit fall back to the safe default.
        assert_eq!(
            AgentPermissionMode::from_wire("edit"),
            AgentPermissionMode::Edit
        );
        assert_eq!(
            AgentPermissionMode::from_wire("garbage"),
            AgentPermissionMode::Edit
        );
    }

    #[test]
    fn claude_new_preassigns_a_session_id_and_passes_settings() {
        let p = plan(
            AgentKind::Claude,
            LaunchMode::New { prompt: None },
            ShellKind::PowerShell,
        );
        let sid = p.preassigned_session_id.clone().expect("session id");
        assert!(uuid::Uuid::parse_str(&sid).is_ok());
        assert_eq!(
            p.command_line,
            format!("claude --session-id '{sid}' --settings '/cfg/claude.json'")
        );
    }

    /// Codex's startup update check is off in every Rustic-spawned session.
    const CODEX_NO_UPDATE: &str = "-c 'check_for_update_on_startup=false'";

    #[test]
    fn resume_uses_each_cli_own_flag() {
        let mode = || LaunchMode::Resume {
            external_session_id: "abc-123".into(),
        };
        assert!(plan(AgentKind::Claude, mode(), ShellKind::Posix)
            .command_line
            .starts_with("claude --resume 'abc-123'"));
        assert_eq!(
            plan(AgentKind::Codex, mode(), ShellKind::Posix).command_line,
            format!("codex {CODEX_NO_UPDATE} resume 'abc-123'")
        );
        assert_eq!(
            plan(AgentKind::Antigravity, mode(), ShellKind::Posix).command_line,
            "agy --conversation 'abc-123'"
        );
        // Resuming never invents a new id — that would fork the conversation.
        assert!(plan(AgentKind::Claude, mode(), ShellKind::Posix)
            .preassigned_session_id
            .is_none());
    }

    #[test]
    fn prompts_are_quoted_per_shell_dialect() {
        let mode = || LaunchMode::New {
            prompt: Some("it's a \"test\" & more".into()),
        };
        assert_eq!(
            plan(AgentKind::Codex, mode(), ShellKind::PowerShell).command_line,
            format!("codex {CODEX_NO_UPDATE} 'it''s a \"test\" & more'")
        );
        assert_eq!(
            plan(AgentKind::Codex, mode(), ShellKind::Posix).command_line,
            format!(r#"codex {CODEX_NO_UPDATE} 'it'\''s a "test" & more'"#)
        );
        assert_eq!(
            plan(AgentKind::Antigravity, mode(), ShellKind::Posix).command_line,
            r#"agy -i 'it'\''s a "test" & more'"#
        );
    }

    #[test]
    fn blank_prompt_is_dropped() {
        let p = plan(
            AgentKind::Codex,
            LaunchMode::New {
                prompt: Some("   ".into()),
            },
            ShellKind::Posix,
        );
        assert_eq!(p.command_line, format!("codex {CODEX_NO_UPDATE}"));
    }

    #[test]
    fn the_startup_update_check_is_suppressed_per_agent() {
        // Codex takes a config override, Claude an env var, and `agy` has no
        // documented opt-out — so it must be left untouched rather than guessed at.
        let codex = plan(
            AgentKind::Codex,
            LaunchMode::New { prompt: None },
            ShellKind::Posix,
        );
        assert!(codex.command_line.contains(CODEX_NO_UPDATE));
        assert!(!codex.env.iter().any(|(k, _)| k == "DISABLE_AUTOUPDATER"));

        let claude = plan(
            AgentKind::Claude,
            LaunchMode::New { prompt: None },
            ShellKind::Posix,
        );
        assert!(claude
            .env
            .contains(&("DISABLE_AUTOUPDATER".to_string(), "1".to_string())));
        assert!(!claude.command_line.contains("check_for_update"));

        let agy = plan(
            AgentKind::Antigravity,
            LaunchMode::New { prompt: None },
            ShellKind::Posix,
        );
        assert_eq!(agy.command_line, "agy");
        assert!(!agy.env.iter().any(|(k, _)| k == "DISABLE_AUTOUPDATER"));
    }

    #[test]
    fn a_resolved_launcher_is_invoked_by_absolute_path() {
        // PowerShell needs the call operator, because the path gets quoted.
        let ps = plan_with_launcher(
            AgentKind::Claude,
            ShellKind::PowerShell,
            r"C:\Users\me\.bun\bin\claude.exe",
        );
        assert!(
            ps.command_line
                .starts_with(r"& 'C:\Users\me\.bun\bin\claude.exe' --session-id "),
            "{}",
            ps.command_line
        );

        let posix = plan_with_launcher(
            AgentKind::Codex,
            ShellKind::Posix,
            "/home/me/.bun/bin/codex",
        );
        assert_eq!(
            posix.command_line,
            format!("'/home/me/.bun/bin/codex' {CODEX_NO_UPDATE}")
        );

        // Without a resolved path the bare name is used and the shell resolves it.
        assert!(plan(
            AgentKind::Codex,
            LaunchMode::New { prompt: None },
            ShellKind::Posix
        )
        .command_line
        .starts_with("codex "));
    }

    #[test]
    fn env_carries_the_correlation_token() {
        let p = plan(
            AgentKind::Claude,
            LaunchMode::New { prompt: None },
            ShellKind::Posix,
        );
        assert!(p
            .env
            .contains(&("RUSTIC_PTY_ID".to_string(), "pty-1".to_string())));
        assert!(p.env.iter().any(|(k, _)| k == "RUSTIC_HOOK_DIR"));
    }

    #[test]
    fn shell_dialect_is_inferred_from_the_program_path() {
        assert_eq!(
            ShellKind::from_program(Some(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
            )),
            ShellKind::PowerShell
        );
        assert_eq!(
            ShellKind::from_program(Some("/usr/bin/pwsh")),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::from_program(Some("/bin/bash")), ShellKind::Posix);
        assert_eq!(ShellKind::from_program(Some("zsh")), ShellKind::Posix);
    }

    #[test]
    fn the_prelude_clears_the_screen_and_the_scrollback() {
        for shell in [ShellKind::PowerShell, ShellKind::Posix] {
            let prelude = shell.launch_prelude();
            assert!(
                prelude.contains("[2J") && prelude.contains("[3J"),
                "{prelude}"
            );
            // Same line as the command, so the shell's echo of it is cleared too.
            assert!(prelude.ends_with("; "), "{prelude}");
            assert!(
                !prelude.contains('\r') && !prelude.contains('\n'),
                "{prelude}"
            );
        }
        assert!(ShellKind::PowerShell
            .launch_prelude()
            .contains("Set-ExecutionPolicy"));
    }

    #[test]
    fn agent_kind_round_trips() {
        for kind in AgentKind::ALL {
            assert_eq!(AgentKind::from_str(kind.as_str()), Some(kind));
        }
        assert_eq!(
            AgentKind::from_str("antigravity"),
            Some(AgentKind::Antigravity)
        );
        assert_eq!(AgentKind::from_str("nope"), None);
    }
}
