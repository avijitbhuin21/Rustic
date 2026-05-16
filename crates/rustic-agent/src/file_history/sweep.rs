//! Background sweep worker (R.1 / Day 4 — shadow-backed).
//!
//! Bash tools push a `SweepJob` after each foreground invocation. A single
//! consumer drains the channel, coalesces bursts, and runs each refresh on
//! a `spawn_blocking` thread so the agent's tokio runtime is never blocked
//! by the worktree walk + blob hashing work libgit2 does inside
//! `shadow.track()`.
//!
//! Burst coalescing rule: when multiple jobs for the same `(task_id,
//! message_id)` arrive within the debounce window, dedup them into one
//! refresh. Pre-R.1 we also tracked the earliest `bash_start` to feed the
//! mtime-based candidate walker; the shadow always re-tracks the whole
//! worktree, so the timestamp is preserved on the `SweepJob` for API
//! compatibility but no longer affects what gets recorded.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::tracker::FileHistory;

/// Default debounce window. Public so tests can shrink it.
pub const DEFAULT_DEBOUNCE_MS: u64 = 50;

#[derive(Debug, Clone)]
pub struct SweepJob {
    pub task_id: String,
    pub message_id: String,
    pub bash_start: SystemTime,
}

/// Callback fired after a sweep finishes processing a snapshot. Args are
/// `(task_id, message_id, newly_recorded_paths)`. `paths` are project-relative
/// (forward slashes) — what the UI should add to the changed-files panel.
pub type ChangeCallback =
    Arc<dyn Fn(&str, &str, &[String]) + Send + Sync + 'static>;

/// Handle to enqueue jobs and (in tests) await worker completion.
pub struct SweepWorker {
    tx: mpsc::UnboundedSender<SweepJob>,
    /// Held so callers can `abort()` the worker on shutdown. We don't
    /// currently expose abort, but keeping the handle prevents the task from
    /// being detached entirely.
    _join: JoinHandle<()>,
}

impl SweepWorker {
    /// Spawn a worker bound to `history`. `on_changes` fires once per
    /// (message_id, sweep) pair after the apply_sweep transaction commits.
    ///
    /// `runtime` is the tokio runtime the worker (and its `tokio::time::sleep`,
    /// `spawn_blocking`, mpsc machinery) runs on. Required because callers
    /// frequently come from synchronous Tauri commands where no ambient tokio
    /// runtime is in scope.
    pub fn spawn(
        runtime: Handle,
        history: FileHistory,
        on_changes: ChangeCallback,
    ) -> Self {
        Self::spawn_with_debounce(
            runtime,
            history,
            on_changes,
            Duration::from_millis(DEFAULT_DEBOUNCE_MS),
        )
    }

    /// Test-friendly variant that lets callers shrink the debounce window.
    pub fn spawn_with_debounce(
        runtime: Handle,
        history: FileHistory,
        on_changes: ChangeCallback,
        debounce: Duration,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<SweepJob>();
        let join = runtime.spawn(async move {
            while let Some(first) = rx.recv().await {
                // Coalescing window: collect every (task_id, message_id)
                // that arrives within `debounce` of the first job, dedup,
                // and refresh each one exactly once.
                let mut pending: HashSet<(String, String)> = HashSet::new();
                pending.insert((first.task_id, first.message_id));
                if !debounce.is_zero() {
                    tokio::time::sleep(debounce).await;
                }
                while let Ok(job) = rx.try_recv() {
                    pending.insert((job.task_id, job.message_id));
                }

                // Run each refresh on its own blocking thread so a
                // single big-worktree walk can't starve other pending
                // pairs. They're independent (different snapshots),
                // and the shadow's mutex still serializes the actual
                // libgit2 calls — but the walk itself happens off the
                // tokio runtime's worker pool.
                let mut joins = Vec::with_capacity(pending.len());
                for (task_id, message_id) in pending {
                    let history = history.clone();
                    let cb = Arc::clone(&on_changes);
                    joins.push(tokio::task::spawn_blocking(move || {
                        let result = history
                            .record_post_bash_state(&message_id)
                            .map(|paths| (task_id, message_id, paths));
                        (result, cb)
                    }));
                }
                for jh in joins {
                    match jh.await {
                        Ok((Ok((task_id, message_id, paths)), cb)) => {
                            if !paths.is_empty() {
                                cb(&task_id, &message_id, &paths);
                            }
                        }
                        Ok((Err(e), _)) => {
                            tracing::warn!(?e, "sweep refresh failed");
                        }
                        Err(join_err) => {
                            tracing::warn!(?join_err, "sweep worker task panicked");
                        }
                    }
                }
            }
        });
        Self { tx, _join: join }
    }

    /// Enqueue a sweep job. Returns Err only if the worker has been dropped.
    pub fn enqueue(&self, job: SweepJob) -> Result<(), SweepEnqueueError> {
        self.tx
            .send(job)
            .map_err(|_| SweepEnqueueError::WorkerStopped)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SweepEnqueueError {
    #[error("sweep worker has stopped")]
    WorkerStopped,
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_history::tracker::FileHistory;
    use rustic_db::Database;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex as StdMutex;
    use std::time::Instant;

    struct Fixture {
        _cfg_dir: tempfile::TempDir,
        _proj_dir: tempfile::TempDir,
        history: FileHistory,
        project_root: PathBuf,
    }

    fn fixture() -> Fixture {
        let cfg_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let project_root = proj_dir.path().canonicalize().unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        let db = Arc::new(StdMutex::new(Database::in_memory().unwrap()));
        {
            let g = db.lock().unwrap();
            g.conn()
                .execute(
                    "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                    [],
                )
                .unwrap();
            g.conn()
                .execute(
                    "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                     VALUES ('t', 'p', 'title', 'created', 'native', 'm')",
                    [],
                )
                .unwrap();
        }
        let history = FileHistory::new(db, project_root.clone(), cfg_dir.path()).unwrap();
        Fixture {
            _cfg_dir: cfg_dir,
            _proj_dir: proj_dir,
            history,
            project_root,
        }
    }

    fn write(p: &Path, content: &[u8]) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(content).unwrap();
    }

    /// Spin until the changes-collected vector accumulates `at_least`
    /// callbacks or `timeout` elapses. Counts entries in the vector (one
    /// per callback fire), NOT total paths — otherwise a single callback
    /// reporting two paths would satisfy `at_least=2` and let the test
    /// proceed before a second callback had a chance to fire.
    async fn wait_for_paths(
        collected: Arc<StdMutex<Vec<(String, Vec<String>)>>>,
        at_least: usize,
        timeout: Duration,
    ) -> Vec<(String, Vec<String>)> {
        let started = Instant::now();
        loop {
            {
                let g = collected.lock().unwrap();
                if g.len() >= at_least {
                    return g.clone();
                }
            }
            if started.elapsed() > timeout {
                return collected.lock().unwrap().clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn single_job_triggers_sweep() {
        let f = fixture();
        f.history.open_snapshot("msg-a", "t").unwrap();

        // R.1 semantics: the sweep reports anything that changed between the
        // snapshot's tree and the current worktree, regardless of when it
        // happened relative to `bash_start`. The legacy mtime-filter test
        // (only post-bash files reported) doesn't apply — see Day 4 notes.
        let pre = f.project_root.join("pre.txt");
        write(&pre, b"written between open_snapshot and bash_start");
        std::thread::sleep(Duration::from_millis(20));

        let bash_start = SystemTime::now();
        let post = f.project_root.join("post.txt");
        write(&post, b"written during bash");

        let collected: Arc<StdMutex<Vec<(String, Vec<String>)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let collected_cl = collected.clone();
        let cb: ChangeCallback = Arc::new(move |_task_id, msg, paths| {
            collected_cl
                .lock()
                .unwrap()
                .push((msg.to_string(), paths.to_vec()));
        });

        let worker = SweepWorker::spawn_with_debounce(
            Handle::current(),
            f.history.clone(),
            cb,
            Duration::from_millis(10),
        );
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-a".into(),
                bash_start,
            })
            .unwrap();

        let got = wait_for_paths(collected, 1, Duration::from_secs(2)).await;
        let all_paths: Vec<String> = got.into_iter().flat_map(|(_, p)| p).collect();
        assert!(
            all_paths.iter().any(|p| p == "post.txt"),
            "expected post.txt to be reported, got {all_paths:?}"
        );
        assert!(
            all_paths.iter().any(|p| p == "pre.txt"),
            "expected pre.txt to be reported (tree-diff sees both new files), got {all_paths:?}"
        );
    }

    #[tokio::test]
    async fn burst_jobs_coalesce_for_same_message() {
        let f = fixture();
        f.history.open_snapshot("msg-b", "t").unwrap();

        let early = SystemTime::now();
        std::thread::sleep(Duration::from_millis(10));
        // File written after early but before later cutoff — included only
        // because we coalesce to the EARLIEST start time.
        let mid = f.project_root.join("mid.txt");
        write(&mid, b"middle");
        std::thread::sleep(Duration::from_millis(10));
        let later = SystemTime::now();

        let collected: Arc<StdMutex<Vec<(String, Vec<String>)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let collected_cl = collected.clone();
        let cb: ChangeCallback = Arc::new(move |_task_id, msg, paths| {
            collected_cl
                .lock()
                .unwrap()
                .push((msg.to_string(), paths.to_vec()));
        });

        let worker = SweepWorker::spawn_with_debounce(
            Handle::current(),
            f.history.clone(),
            cb,
            Duration::from_millis(80),
        );
        // Two bashes ending at different times for the same message — the
        // worker should fold them and use `early` as the cutoff.
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-b".into(),
                bash_start: later,
            })
            .unwrap();
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-b".into(),
                bash_start: early,
            })
            .unwrap();

        let got = wait_for_paths(collected.clone(), 1, Duration::from_secs(2)).await;
        // Should fire exactly once for msg-b (coalesced).
        let calls_for_msg: Vec<_> =
            got.iter().filter(|(m, _)| m == "msg-b").collect();
        assert_eq!(
            calls_for_msg.len(),
            1,
            "expected one coalesced callback, got {got:?}"
        );
        let paths: &Vec<String> = &calls_for_msg[0].1;
        assert!(paths.iter().any(|p| p == "mid.txt"));
    }

    #[tokio::test]
    async fn jobs_for_distinct_messages_run_independently() {
        let f = fixture();
        f.history.open_snapshot("msg-x", "t").unwrap();
        f.history.open_snapshot("msg-y", "t").unwrap();

        let cutoff = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        write(&f.project_root.join("x.txt"), b"x");
        write(&f.project_root.join("y.txt"), b"y");

        let collected: Arc<StdMutex<Vec<(String, Vec<String>)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let collected_cl = collected.clone();
        let cb: ChangeCallback = Arc::new(move |_task_id, msg, paths| {
            collected_cl
                .lock()
                .unwrap()
                .push((msg.to_string(), paths.to_vec()));
        });

        let worker = SweepWorker::spawn_with_debounce(
            Handle::current(),
            f.history.clone(),
            cb,
            Duration::from_millis(10),
        );
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-x".into(),
                bash_start: cutoff,
            })
            .unwrap();
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-y".into(),
                bash_start: cutoff,
            })
            .unwrap();

        let got = wait_for_paths(collected, 2, Duration::from_secs(2)).await;
        let mut msgs: Vec<_> = got.iter().map(|(m, _)| m.clone()).collect();
        msgs.sort();
        assert_eq!(msgs, vec!["msg-x".to_string(), "msg-y".to_string()]);
    }

    /// Day 4: a 30 MiB file mixed in with 100 small files must not stall
    /// the worker. Doesn't measure micro-latency (that's Day 7's perf
    /// pass) — just confirms the refresh completes within a generous
    /// wall-clock budget and reports every changed path.
    #[tokio::test]
    async fn large_file_does_not_starve_sweep() {
        let f = fixture();
        f.history.open_snapshot("msg-big", "t").unwrap();

        // 100 small files…
        for i in 0..100 {
            write(
                &f.project_root.join(format!("small_{i:03}.txt")),
                format!("content #{i}").as_bytes(),
            );
        }
        // …plus one 30 MiB file. Above the 5 MiB sync soft limit but
        // well below the 50 MiB hard cap, so the shadow must capture it.
        let big = vec![b'A'; 30 * 1024 * 1024];
        write(&f.project_root.join("huge.bin"), &big);

        let collected: Arc<StdMutex<Vec<(String, Vec<String>)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let collected_cl = collected.clone();
        let cb: ChangeCallback = Arc::new(move |_task_id, msg, paths| {
            collected_cl
                .lock()
                .unwrap()
                .push((msg.to_string(), paths.to_vec()));
        });

        let worker = SweepWorker::spawn_with_debounce(
            Handle::current(),
            f.history.clone(),
            cb,
            Duration::from_millis(10),
        );

        let enqueued = Instant::now();
        worker
            .enqueue(SweepJob {
                task_id: "t".into(),
                message_id: "msg-big".into(),
                bash_start: SystemTime::now(),
            })
            .unwrap();

        let got = wait_for_paths(collected, 1, Duration::from_secs(15)).await;
        let elapsed = enqueued.elapsed();
        assert!(
            !got.is_empty(),
            "sweep callback never fired within budget (elapsed {elapsed:?})"
        );
        let paths: &Vec<String> = &got[0].1;
        assert!(
            paths.iter().any(|p| p == "huge.bin"),
            "expected huge.bin in callback paths, got {paths:?}"
        );
        // Generous ceiling: shadow.track() on ~30 MiB of fresh content
        // plus 100 tiny files should comfortably complete in seconds
        // on any dev machine. If this regresses we want to know.
        assert!(
            elapsed < Duration::from_secs(10),
            "sweep took {elapsed:?}; suspect starvation or perf regression"
        );
    }
}
