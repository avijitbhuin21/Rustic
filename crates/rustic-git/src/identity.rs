//! Discover a GitHub token the machine already uses for git, so the UI can
//! show the signed-in user even when Rustic itself never stored a token
//! (push/pull work through the system credential helper — issue #13).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hard cap per helper so a misbehaving credential manager can't hang the UI.
const HELPER_TIMEOUT: Duration = Duration::from_secs(6);

/// Hide the console window on Windows for helper processes.
fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Run `cmd` with optional stdin, returning stdout when it exits 0 within the timeout.
fn run_with_timeout(mut cmd: Command, stdin: Option<&str>) -> Option<String> {
    no_window(&mut cmd);
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(input.as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if started.elapsed() < HELPER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Token from the GitHub CLI (`gh auth token`), if installed and signed in.
fn gh_cli_token() -> Option<String> {
    let mut cmd = Command::new("gh");
    cmd.args(["auth", "token", "--hostname", "github.com"]);
    let out = run_with_timeout(cmd, None)?;
    let t = out.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Parse the `password=` line from `git credential fill` output.
pub fn parse_credential_password(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("password="))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

/// Token from the configured git credential helper (e.g. Git Credential
/// Manager), queried non-interactively so no sign-in window can pop up.
fn git_credential_token() -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    let out = run_with_timeout(cmd, Some("protocol=https\nhost=github.com\n\n"))?;
    parse_credential_password(&out)
}

/// First GitHub token found on this machine outside Rustic: `gh` CLI, then
/// the git credential helper. Blocking — call from a blocking task.
pub fn discover_github_token() -> Option<String> {
    gh_cli_token().or_else(git_credential_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_password_line() {
        let out = "protocol=https\nhost=github.com\nusername=octo\npassword=gho_abc123\n";
        assert_eq!(
            parse_credential_password(out).as_deref(),
            Some("gho_abc123")
        );
    }

    #[test]
    fn missing_or_empty_password_is_none() {
        assert_eq!(
            parse_credential_password("protocol=https\nhost=github.com\n"),
            None
        );
        assert_eq!(parse_credential_password("password=\n"), None);
    }
}
