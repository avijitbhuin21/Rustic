//! Project-scoped filesystem watcher.
//!
//! Wraps `notify-debouncer-full` to feed `DirtyPathAccumulator` with the
//! paths that actually changed since the last snapshot. The sweep worker
//! then drains the accumulator and calls `shadow.track_paths` (targeted)
//! instead of `shadow.track` (full walk), per the design in
//! [docs/educated-guesses/005-notify-integration-design.md](../../../../docs/educated-guesses/005-notify-integration-design.md).
//!
//! One watcher per project. The watcher is created when the project's
//! `FileHistoryHandle` is built and dropped when the project closes;
//! `notify-debouncer-full` cleans up its OS-level handles on drop.
//!
//! ### Lost events
//! The kernel's notify buffer can overflow under heavy write load (e.g.
//! `cargo build` writing thousands of artifacts). When the debouncer
//! reports errors, we set `DirtyPathAccumulator::mark_lost` so the next
//! sweep falls back to a full walk — the correctness backstop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

use super::accumulator::DirtyPathAccumulator;
use super::walk::HARD_DENY_DIRS;

/// Default debounce window: long enough to coalesce editor save bursts
/// (3-5 events in ~50 ms), short enough that the changed-files panel
/// stays responsive.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(100);

/// Owning handle for one project's watcher. Drop to stop watching.
pub struct FileWatcher {
    /// Held to keep the OS-level watch alive. Field is unread directly but
    /// dropping it stops watching, which is the entire mechanism here.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    project_root: PathBuf,
}

impl FileWatcher {
    /// Spawn a watcher rooted at `project_root` and feed events into
    /// `accumulator`. Recursive — all subdirectories except the
    /// [`HARD_DENY_DIRS`] list are watched.
    ///
    /// Returns Err only if the initial `watch()` registration fails (e.g.
    /// Linux's `fs.inotify.max_user_watches` exceeded for a huge monorepo,
    /// or trying to watch a path that doesn't exist). On platforms where
    /// notify falls back to PollWatcher under the hood (network drives,
    /// WSL paths), construction still succeeds — at higher CPU cost.
    pub fn spawn(
        project_root: PathBuf,
        accumulator: Arc<DirtyPathAccumulator>,
    ) -> Result<Self, FileWatcherError> {
        let acc_for_cb = Arc::clone(&accumulator);
        let mut debouncer = new_debouncer(
            DEFAULT_DEBOUNCE,
            None,
            move |result: DebounceEventResult| {
                handle_events(&acc_for_cb, result);
            },
        )
        .map_err(FileWatcherError::Notify)?;

        debouncer
            .watch(&project_root, RecursiveMode::Recursive)
            .map_err(FileWatcherError::Notify)?;

        Ok(Self {
            _debouncer: debouncer,
            project_root,
        })
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileWatcherError {
    #[error("notify: {0}")]
    Notify(notify::Error),
}

/// Single entry point that routes both the success and failure paths into
/// the accumulator. Lives outside the closure so it can be tested with
/// synthetic event vectors.
fn handle_events(accumulator: &DirtyPathAccumulator, result: DebounceEventResult) {
    match result {
        Ok(events) => {
            for ev in events {
                process_event(accumulator, &ev.event);
            }
        }
        Err(errors) => {
            for e in &errors {
                tracing::warn!(?e, "fs watcher dropped events");
            }
            // Sticky until the next drain — see DirtyPathAccumulator::mark_lost.
            accumulator.mark_lost();
        }
    }
}

/// Translate one notify Event into accumulator calls. Filters out paths
/// that fall under [`HARD_DENY_DIRS`] (no point recording dirty paths the
/// sweep walker would skip anyway) and ignores access-only events.
fn process_event(accumulator: &DirtyPathAccumulator, event: &notify::Event) {
    use notify::event::{ModifyKind, RenameMode};

    if event.need_rescan() {
        // Backend explicitly told us a rescan is required (typically a
        // dropped-events / overflow signal). Mark lost — the next sweep
        // will do a full walk anyway.
        accumulator.mark_lost();
        return;
    }

    // Skip access events (Open/Read/etc.) — they don't change anything.
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }

    // Skip paths under hard-denied directories. Same list the walker uses
    // so file_history sweeps never see them either.
    let any_in_denied = event
        .paths
        .iter()
        .any(|p| path_is_in_hard_denied(p));
    if any_in_denied {
        return;
    }

    match &event.kind {
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Any) | EventKind::Modify(ModifyKind::Other)
        | EventKind::Modify(ModifyKind::Metadata(_)) => {
            for p in &event.paths {
                accumulator.record_modified(p);
            }
        }
        EventKind::Modify(ModifyKind::Name(rename_mode)) => match rename_mode {
            RenameMode::Both => {
                // notify pairs From/To into one event with two paths when
                // it can match them via inode tracking.
                if event.paths.len() == 2 {
                    accumulator.record_rename(&event.paths[0], &event.paths[1]);
                } else {
                    // Defensive: rare backend quirk. Treat every path as
                    // modified — track_paths will reconcile against disk.
                    for p in &event.paths {
                        accumulator.record_modified(p);
                    }
                }
            }
            RenameMode::From => {
                // The "from" half of an unmatched rename — file went away
                // from this path. The matching To, if it ever lands, will
                // arrive as a separate Create event.
                for p in &event.paths {
                    accumulator.record_removed(p);
                }
            }
            RenameMode::To | RenameMode::Any | RenameMode::Other => {
                for p in &event.paths {
                    accumulator.record_modified(p);
                }
            }
        },
        EventKind::Remove(_) => {
            for p in &event.paths {
                accumulator.record_removed(p);
            }
        }
        EventKind::Any | EventKind::Other => {
            // Backend couldn't classify the change. Conservative: treat as
            // modification so the next sweep re-reads the path. The
            // shadow.track_paths code handles "file doesn't exist anymore"
            // by removing it from the tree.
            for p in &event.paths {
                accumulator.record_modified(p);
            }
        }
        EventKind::Access(_) => unreachable!("filtered above"),
    }
}

/// True when any path component matches one of the hard-denied directory
/// names. Component-level match — `/foo/node_modules/bar/baz.js` is denied
/// but `/foo/node_modules_lookalike/x.js` is not.
fn path_is_in_hard_denied(p: &Path) -> bool {
    p.components().any(|c| {
        if let std::path::Component::Normal(name) = c {
            if let Some(s) = name.to_str() {
                return HARD_DENY_DIRS.iter().any(|d| *d == s);
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(unix)]
    const ROOT: &str = "/tmp/proj";
    #[cfg(windows)]
    const ROOT: &str = "C:\\tmp\\proj";

    fn rel(p: &str) -> PathBuf {
        let mut path = PathBuf::from(ROOT);
        for part in p.split('/') {
            path.push(part);
        }
        path
    }

    fn make_acc() -> Arc<DirtyPathAccumulator> {
        let a = Arc::new(DirtyPathAccumulator::new(PathBuf::from(ROOT)));
        a.set_active("t", "m");
        a
    }

    fn event(kind: EventKind, paths: Vec<PathBuf>) -> notify::Event {
        notify::Event {
            kind,
            paths,
            attrs: Default::default(),
        }
    }

    #[test]
    fn create_event_records_as_modified() {
        let acc = make_acc();
        process_event(
            &acc,
            &event(
                EventKind::Create(notify::event::CreateKind::File),
                vec![rel("src/foo.rs")],
            ),
        );
        let set = acc.peek_active().unwrap();
        assert!(set.modified.contains("src/foo.rs"));
    }

    #[test]
    fn modify_data_event_records_as_modified() {
        let acc = make_acc();
        process_event(
            &acc,
            &event(
                EventKind::Modify(notify::event::ModifyKind::Data(
                    notify::event::DataChange::Content,
                )),
                vec![rel("file.rs")],
            ),
        );
        let set = acc.peek_active().unwrap();
        assert!(set.modified.contains("file.rs"));
    }

    #[test]
    fn remove_event_records_as_removed() {
        let acc = make_acc();
        process_event(
            &acc,
            &event(
                EventKind::Remove(notify::event::RemoveKind::File),
                vec![rel("doomed.rs")],
            ),
        );
        let set = acc.peek_active().unwrap();
        assert!(set.removed.contains("doomed.rs"));
    }

    #[test]
    fn rename_both_with_two_paths_records_rename() {
        let acc = make_acc();
        process_event(
            &acc,
            &event(
                EventKind::Modify(notify::event::ModifyKind::Name(
                    notify::event::RenameMode::Both,
                )),
                vec![rel("old.rs"), rel("new.rs")],
            ),
        );
        let set = acc.peek_active().unwrap();
        assert!(set.removed.contains("old.rs"));
        assert!(set.modified.contains("new.rs"));
    }

    #[test]
    fn access_events_ignored() {
        let acc = make_acc();
        process_event(
            &acc,
            &event(
                EventKind::Access(notify::event::AccessKind::Read),
                vec![rel("read.rs")],
            ),
        );
        let set = acc.peek_active().unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn paths_in_hard_denied_dirs_filtered() {
        let acc = make_acc();
        for denied in &["node_modules", "target", ".git", "dist"] {
            let path = rel(&format!("{}/inner.rs", denied));
            process_event(
                &acc,
                &event(
                    EventKind::Create(notify::event::CreateKind::File),
                    vec![path],
                ),
            );
        }
        let set = acc.peek_active().unwrap();
        assert!(set.is_empty(), "denied dirs should never enter the dirty set");
    }

    #[test]
    fn need_rescan_marks_lost() {
        let acc = make_acc();
        // notify::Event::need_rescan() returns true when the flag is set.
        let mut ev = event(
            EventKind::Other,
            vec![rel("anywhere.rs")],
        );
        ev.attrs.set_flag(notify::event::Flag::Rescan);
        process_event(&acc, &ev);
        let set = acc.peek_active().unwrap();
        assert!(set.lost);
    }

    #[test]
    fn handle_events_marks_lost_on_err_result() {
        let acc = make_acc();
        // Construct a minimal DebounceEventResult Err. notify::Error has
        // helper constructors for common cases.
        let errs: Vec<notify::Error> = vec![notify::Error::generic("simulated buffer overflow")];
        handle_events(&acc, Err(errs));
        let set = acc.peek_active().unwrap();
        assert!(set.lost);
    }
}
