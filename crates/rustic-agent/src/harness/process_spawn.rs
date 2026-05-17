//! Cross-platform spawning of harness CLI processes.
//!
//! On Windows: routes through `cmd.exe /C` (Claude Code installs as `.cmd`
//! which Rust's PATH lookup doesn't resolve), sets `CREATE_NO_WINDOW`, and
//! attaches a Job Object so the full process tree dies when our handle drops.

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
    // F-13 (Medium): resolve to an absolute `.cmd` / `.bat` / `.exe` path
    // ourselves and spawn the binary directly, rather than going through
    // `cmd.exe /C <program>`. The cmd.exe-wrapper path re-parses each arg a
    // second time under cmd.exe's own escaping rules, which differ from
    // CreateProcess's — meaning Rust's stdlib quoting doesn't cover the
    // surface. Resolving via PATHEXT once at spawn time also lets modern
    // Rust apply its `.cmd`/`.bat`-specific arg escaping (CVE-2024-24576 /
    // "BatBadButt" fix in Rust 1.77+).
    //
    // Fall back to the old cmd.exe shape only if PATHEXT resolution fails
    // entirely (extremely unlikely — `claude`/`codex` ship as `.cmd` shims
    // and Rust's PATH walker finds them); the fallback is at least no worse
    // than the pre-fix state.
    if let Some(resolved) = resolve_via_pathext(&spec.program) {
        let mut cmd = Command::new(resolved);
        for a in &spec.args {
            cmd.arg(a);
        }
        cmd
    } else {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C");
        cmd.arg(&spec.program);
        for a in &spec.args {
            cmd.arg(a);
        }
        cmd
    }
}

/// F-13: walk PATH+PATHEXT to find an absolute path for `program` so we can
/// spawn it directly without the `cmd.exe /C` re-parse. Returns the first
/// match.
#[cfg(windows)]
fn resolve_via_pathext(program: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    // Absolute paths bypass the search.
    let p = Path::new(program);
    if p.is_absolute() && p.exists() {
        return Some(p.to_path_buf());
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_|
        ".COM;.EXE;.BAT;.CMD".to_string());
    let extensions: Vec<String> = pathext
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        // If the program already includes an extension, try it as-is first.
        let direct = dir.join(program);
        if direct.is_file() {
            return Some(direct);
        }
        for ext in &extensions {
            let candidate: PathBuf = dir.join(format!("{}{}", program, ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
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
