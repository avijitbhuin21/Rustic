//! Cross-platform spawning of harness CLI processes.
//!
//! Plain `Command::new("claude").spawn()` does the wrong thing on Windows for
//! several non-obvious reasons (plan §8.1):
//!
//! * Claude Code installs as `claude.cmd` (a Node shim), and Rust's PATH
//!   resolution doesn't auto-append `.cmd` the way `cmd.exe` does. We route
//!   through `cmd.exe /C` so the shell does the resolution, mirroring T3 Code.
//! * Without `CREATE_NO_WINDOW`, every spawn flashes a console window.
//! * When we kill the parent `cmd.exe`, the Node child can survive. We attach
//!   a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so the entire
//!   process tree dies when our handle drops.
//!
//! This module isolates all of that. `claude_code.rs` and `codex.rs` (future
//! chunks) just call `SpawnedHarnessChild::spawn(...)` and read/write the
//! returned stdio handles.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// Specification for spawning a harness CLI.
#[derive(Debug, Clone)]
pub struct HarnessSpawnSpec {
    /// Binary name (resolved via PATH) or absolute path. Examples: `"claude"`,
    /// `"codex"`, `"C:\\Users\\me\\AppData\\Local\\..\\claude.cmd"`.
    pub program: String,
    /// CLI arguments. Do not include the program itself.
    pub args: Vec<String>,
    /// Working directory the CLI should run in.
    pub cwd: PathBuf,
    /// Extra environment variables (merged on top of the parent env).
    pub env: Vec<(String, String)>,
}

/// A live child plus its stdio. The child is killed (and on Windows, the
/// whole job tree is terminated) when this struct is dropped.
///
/// Stdio handles are stored as `Option<...>` so callers can `take()` them
/// out to spawn dedicated reader/writer tasks while the parent struct keeps
/// owning the child handle (and the Job Object on Windows).
pub struct SpawnedHarnessChild {
    child: Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    #[cfg(windows)]
    _job: windows_job::JobHandle,
}

impl SpawnedHarnessChild {
    /// Spawn the CLI with platform-appropriate flags. Stdin / stdout / stderr
    /// are all piped so the caller can drive the NDJSON protocol on stdin and
    /// read structured events from stdout (stderr is for crash diagnostics).
    pub fn spawn(spec: HarnessSpawnSpec) -> Result<Self> {
        let mut cmd = build_command(&spec);
        cmd.current_dir(&spec.cwd);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(windows)]
        apply_windows_flags(&mut cmd);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", spec.program))?;

        let stdin = child.stdin.take().context("child stdin was not piped")?;
        let stdout = child.stdout.take().context("child stdout was not piped")?;
        let stderr = child.stderr.take().context("child stderr was not piped")?;

        #[cfg(windows)]
        let _job = {
            let pid = child.id().context("spawned child has no PID yet")?;
            windows_job::attach_kill_on_close(pid)
                .context("failed to attach Job Object to harness child")?
        };

        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
            #[cfg(windows)]
            _job,
        })
    }

    /// Best-effort kill. The Drop impl already handles this for normal teardown
    /// (`kill_on_drop(true)` plus the Job Object); call this only when you
    /// need to await exit.
    pub async fn kill(&mut self) -> Result<()> {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        Ok(())
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
}

#[cfg(windows)]
fn build_command(spec: &HarnessSpawnSpec) -> Command {
    // Route through cmd.exe so `.cmd` shim resolution works the same way it
    // would in an interactive shell. Direct `Command::new("claude")` fails
    // because Rust's CreateProcess lookup does not append `.cmd`.
    let mut cmd = Command::new("cmd.exe");
    cmd.arg("/C");
    cmd.arg(&spec.program);
    for a in &spec.args {
        cmd.arg(a);
    }
    cmd
}

#[cfg(not(windows))]
fn build_command(spec: &HarnessSpawnSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    for a in &spec.args {
        cmd.arg(a);
    }
    cmd
}

#[cfg(windows)]
fn apply_windows_flags(cmd: &mut Command) {
    // CREATE_NO_WINDOW = 0x0800_0000 — suppresses the console flash.
    // tokio::process::Command exposes `creation_flags` as an inherent method
    // on Windows, so no `std::os::windows::process::CommandExt` import needed.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(windows)]
mod windows_job {
    //! Job Object plumbing. We create one job per spawned harness child,
    //! assign the child to it, and set `KILL_ON_JOB_CLOSE` so when the job
    //! handle drops (i.e. our `SpawnedHarnessChild` is dropped) the whole
    //! descendant tree is terminated by the kernel — no zombie Node processes.

    use anyhow::{anyhow, Context, Result};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// RAII wrapper. Closing the handle is what triggers the kernel to kill
    /// every process in the job (per `KILL_ON_JOB_CLOSE`).
    pub struct JobHandle(HANDLE);

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    // SAFETY: a Win32 HANDLE is just an opaque pointer; the kernel object
    // behind it is process-global and refcounted, so moving the wrapper
    // across threads is fine.
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    pub fn attach_kill_on_close(pid: u32) -> Result<JobHandle> {
        unsafe {
            let job = CreateJobObjectW(None, None)
                .context("CreateJobObjectW failed")?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            };

            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .context("SetInformationJobObject failed")?;

            let proc = OpenProcess(PROCESS_TERMINATE | PROCESS_SET_QUOTA, false, pid)
                .with_context(|| format!("OpenProcess({pid}) failed"))?;

            let assign_res = AssignProcessToJobObject(job, proc);
            let _ = CloseHandle(proc);
            assign_res.map_err(|e| anyhow!("AssignProcessToJobObject failed: {e}"))?;

            Ok(JobHandle(job))
        }
    }
}
