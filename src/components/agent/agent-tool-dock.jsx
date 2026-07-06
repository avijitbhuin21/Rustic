import React, { useEffect, useMemo, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  CheckCircle2,
  Circle,
  CircleDotDashed,
  FileEdit,
  Folder,
  ListChecks,
  RotateCcw,
  TerminalSquare,
  X,
} from 'lucide-react';
import { useAgent } from '@/state/agent';
import { useTerminal } from '@/state/terminal';
import { useEditor } from '@/state/editor';
import { useExplorer } from '@/state/explorer';
import { confirm } from '@/components/confirm-dialog';
import { EmptyState } from './empty-state';
import { cn } from '@/lib/utils';

// Three-tab dock pinned to the top of the prompt box. Visually fused with the
// prompt below it via shared border colour + matching corner radii (top
// rounded here, bottom rounded on the prompt; the seam between them has no
// horizontal border, so they read as one container).
//
// Tabs (left → right): Plan (todos), Files (placeholder), Terminals
// (placeholder). Only one can be expanded at a time. Clicking a tab toggles
// its panel open/closed below the tab row, filling the full dock width.
// Empty-state copy lives inside each panel so tabs remain visible even when
// their underlying feature has no data yet — matches what the user asked for.

const EMPTY = [];

function PlanStatusIcon({ status }) {
  if (status === 'completed') {
    return <CheckCircle2 className="size-3.5 shrink-0 text-green-500" />;
  }
  if (status === 'in_progress') {
    return (
      <CircleDotDashed className="size-3.5 shrink-0 animate-spin text-blue-500 [animation-duration:3s]" />
    );
  }
  return <Circle className="size-3.5 shrink-0 text-muted-foreground/60" />;
}

function PlanContent({ todos }) {
  if (!todos || todos.length === 0) {
    return (
      <EmptyState
        icon={ListChecks}
        title="No plan yet"
        hint="The agent hasn't published a plan for this task."
      />
    );
  }
  return (
    <ul className="flex flex-col">
      {todos.map((t, idx) => (
        <li
          key={idx}
          className={cn(
            'flex items-start gap-2 px-3 py-1.5 text-xs',
            t.status === 'completed' &&
              'text-muted-foreground line-through decoration-muted-foreground/50',
            t.status === 'in_progress' && 'text-foreground',
            t.status === 'pending' && 'text-foreground/85',
          )}
        >
          <span className="pt-0.5">
            <PlanStatusIcon status={t.status} />
          </span>
          <span className="min-w-0 flex-1 whitespace-pre-wrap break-words leading-relaxed">
            {t.content}
          </span>
        </li>
      ))}
    </ul>
  );
}

function TerminalsContent({ terminals, onOpenTerminal, onCloseTerminal }) {
  if (!terminals || terminals.length === 0) {
    return (
      <EmptyState
        icon={TerminalSquare}
        title="No active agent terminals"
        hint="Long-running shells the agent spawns will appear here."
      />
    );
  }
  return (
    <ul className="flex flex-col">
      {terminals.map((t) => (
        // group row: the open action is the main button, the close (×) is a
        // sibling button (nesting a button inside a button is invalid HTML),
        // revealed on hover/focus like the Files-list revert action.
        <li
          key={t.id}
          className="group flex items-center gap-1 border-b border-border/20 last:border-b-0"
        >
          <button
            type="button"
            onClick={() => onOpenTerminal?.(t.id, t.label || 'agent')}
            className={cn(
              'flex min-w-0 flex-1 flex-col gap-0.5 px-3 py-1.5 text-left text-xs',
              'transition-colors hover:bg-foreground/[0.04] focus:bg-foreground/[0.04]',
              'focus:outline-none',
            )}
            title={`Open terminal #${t.id}`}
          >
            <div className="flex items-center gap-2">
              <TerminalSquare className="size-3.5 shrink-0 text-muted-foreground/60" />
              <span className="font-mono text-[11px] text-muted-foreground">
                #{t.id}
              </span>
              <span className="min-w-0 flex-1 truncate font-medium text-foreground/90">
                {t.label || 'agent'}
              </span>
            </div>
            {t.last_command && (
              <div className="ml-[22px] truncate font-mono text-[10px] text-muted-foreground">
                $ {t.last_command}
              </div>
            )}
            {t.cwd && (
              <div className="ml-[22px] truncate text-[10px] text-muted-foreground/70">
                {t.cwd}
              </div>
            )}
          </button>
          <button
            type="button"
            onClick={(ev) => {
              ev.stopPropagation();
              onCloseTerminal?.(t.id);
            }}
            className={cn(
              'mr-2 flex shrink-0 items-center rounded p-1 text-muted-foreground/60',
              'transition-colors hover:bg-destructive/10 hover:text-destructive',
              'focus:outline-none focus:bg-destructive/10 focus:text-destructive',
              'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100',
            )}
            title={`Terminate terminal #${t.id}`}
          >
            <X className="size-3.5" />
          </button>
        </li>
      ))}
    </ul>
  );
}

function FilesContent({ entries, onOpenDiff, onRevertPath, onRevertAll, busyPath }) {
  if (!entries || entries.length === 0) {
    return (
      <EmptyState
        icon={FileEdit}
        title="No file changes tracked yet"
        hint="The agent's writes and bash-driven changes will appear here."
      />
    );
  }
  // Hide ghost entries — paths the task created/modified but that no
  // longer exist on disk (typically because the user deleted them
  // manually after the task finished). The shadow store's final tree
  // still records them, so they'd otherwise show up as "created" with
  // "No changes to display" on click. We surface the count so the user
  // knows something was filtered, but don't show the rows themselves.
  const isGhost = (e) =>
    !e.is_dir &&
    e.exists_on_disk === false &&
    (e.kind === 'created' || e.kind === 'modified');
  const visibleEntries = entries.filter((e) => !isGhost(e));
  const ghostCount = entries.length - visibleEntries.length;

  // Compute totals over file entries only — folder rows don't carry
  // meaningful line counts (they show up because the tracked path
  // happens to be a directory on disk right now).
  const fileEntries = visibleEntries.filter((e) => !e.is_dir);
  const totalAdditions = fileEntries.reduce((acc, e) => acc + (e.additions || 0), 0);
  const totalDeletions = fileEntries.reduce((acc, e) => acc + (e.deletions || 0), 0);

  return (
    <div className="flex flex-col">
      {/* Header row with aggregate +/- counts and revert-all. Sits inside
          the scroll container so it scrolls with the list — keeps the
          dock from getting a sticky-second-header look at small heights. */}
      <div className="flex items-center justify-between border-b border-border/30 bg-foreground/[0.02] px-3 py-1.5">
        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          <span>{fileEntries.length} file{fileEntries.length === 1 ? '' : 's'}</span>
          {totalAdditions > 0 && (
            <span className="font-mono text-emerald-500">+{totalAdditions}</span>
          )}
          {totalDeletions > 0 && (
            <span className="font-mono text-rose-500">-{totalDeletions}</span>
          )}
        </div>
        <button
          type="button"
          onClick={onRevertAll}
          // Enable when there's anything the task touched — files, ghosts
          // (paths already gone from disk), or leftover folder entries.
          // Ghosts matter because revert also prunes empty parent dirs the
          // task created; a state with "0 files + folder leftover from a
          // prior revert" still has something to clean up.
          disabled={entries.length === 0 || !!busyPath}
          className={cn(
            'flex items-center gap-1 rounded border border-border/40 px-2 py-0.5 text-[10px] font-medium',
            'text-foreground/80 transition-colors',
            'hover:bg-destructive/10 hover:text-destructive hover:border-destructive/40',
            'disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent disabled:hover:text-foreground/80',
          )}
          title="Revert every file this task changed"
        >
          <RotateCcw className="size-3" />
          Revert all
        </button>
      </div>
      <ul className="flex flex-col">
        {visibleEntries.map((e) => (
          <FileEntryRow
            key={e.path}
            entry={e}
            onOpenDiff={onOpenDiff}
            onRevertPath={onRevertPath}
            busy={busyPath === e.path}
            anyBusy={!!busyPath}
          />
        ))}
      </ul>
      {ghostCount > 0 && (
        <div className="border-t border-border/30 px-3 py-1 text-[10px] italic text-muted-foreground/70">
          {ghostCount} hidden — file{ghostCount === 1 ? '' : 's'} no longer on disk
        </div>
      )}
    </div>
  );
}

/// Map an entry's `kind` to the on-disk action a revert will perform:
///   - 'created'  → file will be DELETED  (the task brought it into being)
///   - 'modified' → file will be RESTORED to its pre-task content
///   - 'deleted'  → file will be RESTORED (recreated as it was pre-task)
/// Returns 'restore' for everything that isn't a fresh create.
function revertActionForEntry(entry) {
  if (!entry) return 'restore';
  return entry.kind === 'created' ? 'delete' : 'restore';
}

/// Single-line preview row for the revert dialog — file name + the
/// action that's about to happen + line counts. `compact` mode is used
/// inside the revert-all list (smaller padding, no extra spacing); the
/// non-compact mode is used for the per-file revert which shows just
/// one entry.
function RevertEntryPreview({ entry, compact = false }) {
  const action = revertActionForEntry(entry);
  const actionColour =
    action === 'delete' ? 'text-rose-500' : 'text-emerald-500';
  const actionLabel = action === 'delete' ? 'will delete' : 'will restore';
  const { additions, deletions, binary } = entry;

  return (
    <div
      className={cn(
        'flex w-full items-center gap-2',
        !compact && 'gap-3 py-0.5',
      )}
    >
      <FileEdit className="size-3 shrink-0 text-muted-foreground/60" />
      <span className="min-w-0 flex-1 truncate font-mono text-[11px]">
        {entry.path}
      </span>
      <span className={cn('shrink-0 text-[10px] font-medium', actionColour)}>
        {actionLabel}
      </span>
      {!binary && (additions > 0 || deletions > 0) && (
        <span className="flex shrink-0 items-center gap-1 font-mono text-[10px]">
          {additions > 0 && (
            <span className="text-emerald-500">+{additions}</span>
          )}
          {deletions > 0 && (
            <span className="text-rose-500">-{deletions}</span>
          )}
        </span>
      )}
      {binary && (
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground/60">
          bin
        </span>
      )}
    </div>
  );
}

function FileEntryRow({ entry, onOpenDiff, onRevertPath, busy, anyBusy }) {
  const { path, kind, additions, deletions, binary, is_dir: isDir } = entry;

  // Folders are non-clickable, no revert action. We surface them with a
  // folder icon and a hint so the user understands why the row is inert.
  if (isDir) {
    return (
      <li
        className={cn(
          'flex items-center gap-2 border-b border-border/20 px-3 py-1.5 text-xs text-muted-foreground/80 last:border-b-0',
        )}
        title={`${path} (folder)`}
      >
        <Folder className="size-3.5 shrink-0 text-muted-foreground/50" />
        <span className="min-w-0 flex-1 truncate font-mono text-[11px]">{path}</span>
        <span className="text-[10px] italic text-muted-foreground/60">folder</span>
      </li>
    );
  }

  const kindColour =
    kind === 'created'
      ? 'text-emerald-500'
      : kind === 'deleted'
      ? 'text-rose-500'
      : 'text-amber-500';
  const kindGlyph =
    kind === 'created' ? 'A' : kind === 'deleted' ? 'D' : 'M';

  return (
    <li className="group flex items-center gap-1 border-b border-border/20 last:border-b-0">
      <button
        type="button"
        onClick={() => onOpenDiff?.(path)}
        className={cn(
          'flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-left text-xs text-foreground/85',
          'transition-colors hover:bg-foreground/[0.04] focus:bg-foreground/[0.04]',
          'focus:outline-none',
        )}
        title={`Open diff for ${path}`}
      >
        <FileEdit className="size-3.5 shrink-0 text-muted-foreground/60" />
        <span
          className={cn(
            'shrink-0 font-mono text-[10px] font-semibold w-3 text-center',
            kindColour,
          )}
          title={kind}
        >
          {kindGlyph}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px]">
          {path}
        </span>
        {/* Per-file +/- stats. Hidden for binary diffs where the counts
            are always zero — the kind glyph already conveys the change. */}
        {!binary && (additions > 0 || deletions > 0) && (
          <span className="ml-1 flex shrink-0 items-center gap-1 font-mono text-[10px]">
            {additions > 0 && <span className="text-emerald-500">+{additions}</span>}
            {deletions > 0 && <span className="text-rose-500">-{deletions}</span>}
          </span>
        )}
        {binary && (
          <span className="ml-1 shrink-0 font-mono text-[10px] text-muted-foreground/60">
            bin
          </span>
        )}
      </button>
      <button
        type="button"
        onClick={(ev) => {
          ev.stopPropagation();
          onRevertPath?.(path);
        }}
        disabled={busy || anyBusy}
        className={cn(
          'mr-2 flex shrink-0 items-center rounded p-1 text-muted-foreground/60',
          'transition-colors hover:bg-destructive/10 hover:text-destructive',
          'focus:outline-none focus:bg-destructive/10 focus:text-destructive',
          'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100',
          'disabled:cursor-not-allowed disabled:opacity-30',
        )}
        title={`Revert ${path} to its pre-task state`}
      >
        <RotateCcw className={cn('size-3.5', busy && 'animate-spin')} />
      </button>
    </li>
  );
}

const panelVariants = {
  hidden: { opacity: 0, height: 0 },
  visible: {
    opacity: 1,
    height: 'auto',
    transition: { duration: 0.22, ease: [0.2, 0.65, 0.3, 0.9] },
  },
  exit: {
    opacity: 0,
    height: 0,
    transition: { duration: 0.18, ease: [0.2, 0.65, 0.3, 0.9] },
  },
};

export function AgentToolDock() {
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const todos = useAgent((s) =>
    activeTaskId ? s.todosByTask[activeTaskId] || EMPTY : EMPTY,
  );
  const fileEntries = useAgent((s) =>
    activeTaskId ? s.filesByTask[activeTaskId]?.entries || EMPTY : EMPTY,
  );
  // Drop ghost entries (kind='created'/'modified' but the file is no
  // longer on disk) before they reach the badge count, the revert-all
  // preview, or the revert-all action targets. Keeping them in the raw
  // state lets us still display "N hidden" inside the Files panel; the
  // panel decides where to show them.
  const visibleFileEntries = useMemo(
    () =>
      fileEntries.filter(
        (e) =>
          !(
            !e.is_dir &&
            e.exists_on_disk === false &&
            (e.kind === 'created' || e.kind === 'modified')
          ),
      ),
    [fileEntries],
  );

  // Open a path in the editor area as a diff tab when the user clicks it
  // in the Files list. Uses the same `openDiff` action the SCM panel
  // uses, so the resulting view (Monaco / parse-diff renderer) is exactly
  // what the user already sees in Source Control. `oid: null` means
  // "working tree vs HEAD"; the diff-view falls through to the
  // untracked-file path for newly-created files (rustic-git/diff.rs).
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const activeProjectRoot = useAgent((s) => s.activeProject?.root || null);
  const activeWorktree = useAgent((s) =>
    s.activeTaskId ? s.worktreeByTask[s.activeTaskId] : null,
  );
  const activeWorktreeTaskId =
    activeWorktree && !['merged', 'discarded'].includes(activeWorktree.state)
      ? activeWorktree.task_id
      : null;
  const openEditorDiff = useEditor((s) => s.openDiff);
  const openEditorTerminal = useEditor((s) => s.openTerminal);
  const handleOpenDiff = useMemo(
    () => (path) => {
      if (!activeProjectId) return;
      // Prefer the file-history cumulative diff (pre-task vs current) — the
      // same source as the +/- stats on the row. Git-based fallbacks show
      // nothing for isolated tasks once turns are checkpoint-committed.
      const entry = fileEntries.find((e) => e.path === path);
      const anchor = entry?.anchor_message_id || null;
      openEditorDiff({
        projectId: activeProjectId,
        filePath: path,
        worktreeTaskId: activeWorktreeTaskId,
        fhAnchor:
          anchor && activeProjectRoot
            ? { projectRoot: activeProjectRoot, messageId: anchor }
            : null,
      });
    },
    [activeProjectId, openEditorDiff, activeWorktreeTaskId, fileEntries, activeProjectRoot],
  );
  // Opens the terminal in the bottom panel. The terminal session is already
  // live (the agent spawned it); we're just attaching the UI.
  const handleOpenTerminal = useMemo(
    () => (sessionId, label) => {
      openEditorTerminal(sessionId, label);
    },
    [openEditorTerminal],
  );

  // Per-file revert: invokes fh_revert_path, then removes the entry from
  // filesByTask on success. `busyPath` blocks duplicate clicks and other
  // revert actions while one is in flight (revert ops touch disk and
  // race with subsequent operations badly).
  const [busyRevertPath, setBusyRevertPath] = useState(null);
  const handleRevertPath = useMemo(
    () => async (path) => {
      if (!activeTaskId || !activeProjectRoot) return;
      // Find this entry in the current dock state so we can preview the
      // action (delete vs restore) and the line counts in the dialog.
      const entry = fileEntries.find((e) => e.path === path);
      const action = revertActionForEntry(entry);
      const ok = await confirm({
        title: `Revert ${path}?`,
        description:
          action === 'delete'
            ? 'This file will be deleted — the task created it from scratch.'
            : 'This file will be restored to its pre-task state.',
        details: entry ? (
          <div className="rounded border border-border/40 bg-foreground/[0.03] p-2">
            <RevertEntryPreview entry={entry} />
          </div>
        ) : null,
        confirmLabel: action === 'delete' ? 'Delete file' : 'Restore file',
        cancelLabel: 'Cancel',
        destructive: true,
      });
      if (!ok) return;
      setBusyRevertPath(path);
      try {
        const outcomes = await invoke('fh_revert_path', {
          projectRoot: activeProjectRoot,
          taskId: activeTaskId,
          path,
        });
        const failed = (Array.isArray(outcomes) ? outcomes : []).filter(
          (o) => o.action === 'failed',
        );
        if (failed.length > 0) {
          // The shadow store couldn't apply the revert (locked file,
          // file-vs-dir mismatch, permission denied). Don't drop the row
          // — it's still a real change as far as the task is concerned.
          toast.error(failed[0].error || `Couldn't revert ${path}`);
        } else {
          useAgent.setState((s) => {
            const prev = s.filesByTask[activeTaskId];
            if (!prev) return s;
            const nextEntries = prev.entries.filter((e) => e.path !== path);
            return {
              filesByTask: {
                ...s.filesByTask,
                [activeTaskId]: { ...prev, entries: nextEntries },
              },
            };
          });
          toast.success(`Reverted ${path}`);
        }
      } catch (err) {
        toast.error(`Revert failed: ${err}`);
      } finally {
        setBusyRevertPath(null);
      }
    },
    // fileEntries belongs in deps — the closure looks the entry up by path
    // for the preview, and without this the callback captures whatever
    // `fileEntries` was at first render (often the empty initial array, so
    // the dialog renders blank line counts and the wrong action label).
    [activeTaskId, activeProjectRoot, fileEntries],
  );

  const handleRevertAll = useMemo(
    () => async () => {
      if (!activeTaskId || !activeProjectRoot) return;
      // Group entries by what's about to happen so the user sees the
      // damage count up front. Folder rows are excluded — they don't
      // correspond to a revert action — and ghost rows have already
      // been filtered out of visibleFileEntries.
      const previewEntries = visibleFileEntries.filter((e) => !e.is_dir);
      const restoreCount = previewEntries.filter(
        (e) => revertActionForEntry(e) === 'restore',
      ).length;
      const deleteCount = previewEntries.filter(
        (e) => revertActionForEntry(e) === 'delete',
      ).length;
      const totalAdditions = previewEntries.reduce(
        (acc, e) => acc + (e.additions || 0),
        0,
      );
      const totalDeletions = previewEntries.reduce(
        (acc, e) => acc + (e.deletions || 0),
        0,
      );
      const ok = await confirm({
        title: 'Revert every file this task changed?',
        description: (() => {
          const parts = [];
          if (restoreCount > 0) {
            parts.push(`${restoreCount} restored`);
          }
          if (deleteCount > 0) {
            parts.push(`${deleteCount} deleted`);
          }
          const summary = parts.join(', ') || 'no files';
          return `${summary}.`;
        })(),
        details: (
          <div className="flex flex-col gap-2">
            <div className="flex items-center gap-3 text-[11px] text-muted-foreground">
              <span>
                {previewEntries.length} file
                {previewEntries.length === 1 ? '' : 's'}
              </span>
              {totalAdditions > 0 && (
                <span className="font-mono text-emerald-500">
                  +{totalAdditions}
                </span>
              )}
              {totalDeletions > 0 && (
                <span className="font-mono text-rose-500">
                  -{totalDeletions}
                </span>
              )}
            </div>
            <ul
              className={cn(
                'flex flex-col rounded border border-border/40',
                'max-h-[260px] overflow-y-auto bg-foreground/[0.03]',
              )}
            >
              {previewEntries.map((e) => (
                <li
                  key={e.path}
                  className="flex items-center gap-2 border-b border-border/20 px-2 py-1 text-[11px] last:border-b-0"
                >
                  <RevertEntryPreview entry={e} compact />
                </li>
              ))}
            </ul>
          </div>
        ),
        confirmLabel: 'Revert all',
        cancelLabel: 'Cancel',
        destructive: true,
      });
      if (!ok) return;
      setBusyRevertPath('__all__');
      try {
        const outcomes = await invoke('fh_revert_task', {
          projectRoot: activeProjectRoot,
          taskId: activeTaskId,
        });
        const list = Array.isArray(outcomes) ? outcomes : [];
        const failed = list.filter((o) => o.action === 'failed');
        // "skipped" = the cross-session guard refused the path because it
        // was modified outside this task's timeline. Not reverted — keep
        // the row so the user can force it with the per-file revert.
        const skipped = list.filter((o) => o.action === 'skipped');
        const succeededPaths = new Set(
          list
            .filter((o) => o.action !== 'failed' && o.action !== 'skipped')
            .map((o) => o.path),
        );

        // Drop only the rows that successfully reverted; failed rows
        // stay so the user can see what didn't get cleaned up.
        useAgent.setState((s) => {
          const prev = s.filesByTask[activeTaskId];
          if (!prev) return s;
          const nextEntries = prev.entries.filter(
            (e) => !succeededPaths.has(e.path),
          );
          return {
            filesByTask: {
              ...s.filesByTask,
              [activeTaskId]: { ...prev, entries: nextEntries },
            },
          };
        });

        const skippedNote =
          skipped.length > 0
            ? ` — ${skipped.length} skipped (changed outside this task)`
            : '';
        if (failed.length === 0 && skipped.length === 0) {
          toast.success(
            `Reverted ${succeededPaths.size} file${succeededPaths.size === 1 ? '' : 's'}`,
          );
        } else if (failed.length === 0 && succeededPaths.size === 0) {
          toast.warning(
            `Nothing reverted${skippedNote}. Use the per-file revert to force.`,
          );
        } else if (failed.length === 0) {
          toast.warning(
            `Reverted ${succeededPaths.size} file${succeededPaths.size === 1 ? '' : 's'}${skippedNote}`,
          );
        } else if (succeededPaths.size === 0) {
          toast.error(
            `Revert failed for all ${failed.length} file${failed.length === 1 ? '' : 's'}: ${failed[0].error || 'unknown error'}${skippedNote}`,
          );
        } else {
          toast.warning(
            `Reverted ${succeededPaths.size}, failed ${failed.length}: ${failed[0].error || 'unknown error'}${skippedNote}`,
          );
        }
      } catch (err) {
        toast.error(`Revert failed: ${err}`);
      } finally {
        setBusyRevertPath(null);
      }
    },
    // visibleFileEntries belongs in deps — the callback reads it for the
    // preview AND the file counts. Without this, opening the dock then
    // clicking "Revert all" later showed "no files / 0 files" even though
    // the panel above it was clearly displaying entries (the closure had
    // captured the empty initial array).
    [activeTaskId, activeProjectRoot, visibleFileEntries],
  );

  // Pull the live agent-spawned terminals for the active task. The terminal
  // store is kept in sync with the backend via `terminal-list-changed`
  // events (wired by the terminal-panel / activity-bar on mount), so this
  // selector just filters the global list. We also kick wireListeners +
  // refreshSessions on mount in case the dock renders before either of
  // those components has — the calls are idempotent.
  const wireTerminalListeners = useTerminal((s) => s.wireListeners);
  const refreshTerminalSessions = useTerminal((s) => s.refreshSessions);
  const closeTerminalSession = useTerminal((s) => s.closeTerminal);
  useEffect(() => {
    wireTerminalListeners();
    refreshTerminalSessions();
  }, [wireTerminalListeners, refreshTerminalSessions]);

  // Terminate an agent terminal straight from the dock. closeTerminal kills the
  // backend pty session and updates the store; the `terminal-list-changed`
  // event then prunes the row. No confirm — terminals are cheap and the agent
  // can always spawn another.
  const handleCloseTerminal = useMemo(
    () => (sessionId) => {
      closeTerminalSession(sessionId);
    },
    [closeTerminalSession],
  );

  // Pull the raw sessions list with a stable-reference selector, then derive
  // the per-task filtered view inside useMemo. Doing `s.sessions.filter(...)`
  // *inside* the selector breaks Zustand's getSnapshot caching: `filter`
  // returns a brand-new array each call, React sees a new reference every
  // render, forces a re-render, the selector runs again, returns another
  // new array, and we hit "Maximum update depth exceeded".
  const allSessions = useTerminal((s) => s.sessions);
  const agentTerminals = useMemo(() => {
    if (!activeTaskId) return EMPTY;
    return allSessions.filter(
      (t) => t.is_agent && t.task_id === activeTaskId,
    );
  }, [allSessions, activeTaskId]);

  // Active tab + "have we auto-opened Plan yet?" both live in the agent
  // store so they survive component remounts. Previously these were
  // `useState` hooks, but the dock unmounts whenever the editor area
  // shifts (opening a diff / terminal from one of its tabs), and the
  // remount reset both fields — the auto-open effect would then fire
  // and bounce the active tab back to 'plan' every time.
  const activeByTask = useAgent((s) => s.dockActiveByTask);
  const autoOpened = useAgent((s) => s.dockAutoOpenedByTask);
  const setDockActiveTab = useAgent((s) => s.setDockActiveTab);
  const markDockAutoOpened = useAgent((s) => s.markDockAutoOpened);
  const activeTab = activeTaskId ? activeByTask[activeTaskId] || null : null;
  const setActiveTab = (val) => setDockActiveTab(activeTaskId, val);

  // Auto-open the Plan tab the first time todos appear for a task — same
  // "the agent just published something, the user should notice" trigger
  // we had on the standalone TodoPanel. The per-task auto-open flag
  // persists across remounts so this only fires once per task, not every
  // time the dock comes back from an editor-area shift.
  useEffect(() => {
    if (!activeTaskId) return;
    if (todos.length === 0) return;
    if (autoOpened[activeTaskId]) return;
    markDockAutoOpened(activeTaskId);
    // Only flip the tab if the user hasn't picked one yet. `undefined`
    // means "never touched"; `null` means "user explicitly collapsed".
    if (activeByTask[activeTaskId] === undefined) {
      setDockActiveTab(activeTaskId, 'plan');
    }
  }, [
    activeTaskId,
    todos.length,
    autoOpened,
    activeByTask,
    markDockAutoOpened,
    setDockActiveTab,
  ]);

  const planCounts = useMemo(() => {
    let done = 0;
    for (const t of todos) if (t.status === 'completed') done += 1;
    return { done, total: todos.length };
  }, [todos]);

  if (!activeTaskId) return null;

  const tabs = [
    {
      id: 'plan',
      icon: ListChecks,
      label: 'Plan',
      badge:
        planCounts.total > 0
          ? `${planCounts.done}/${planCounts.total}`
          : null,
    },
    {
      id: 'files',
      icon: FileEdit,
      // Badge counts files only, excluding folder rows AND ghost rows
      // (paths the task created/modified but that the user has since
      // deleted from disk). Keeps the badge in lock-step with the
      // "N files" header inside the panel.
      label: 'Files',
      badge: (() => {
        const fileCount = visibleFileEntries.filter((e) => !e.is_dir).length;
        return fileCount > 0 ? String(fileCount) : null;
      })(),
    },
    {
      id: 'terminals',
      icon: TerminalSquare,
      label: 'Terminals',
      badge: agentTerminals.length > 0 ? String(agentTerminals.length) : null,
    },
  ];

  return (
    <div className="mx-auto w-full max-w-3xl px-3">
      <div
        className={cn(
          'overflow-hidden rounded-3xl rounded-b-none border border-border/70 border-b-0 bg-popover',
          'transition-shadow duration-200',
          activeTab ? 'shadow-[0_-4px_18px_rgba(0,0,0,0.12)]' : '',
        )}
      >
        {/* Tab row. The seam between tabs is a 1px border-r on each except
            the last; the active tab gets a tinted bg so it reads as
            "selected" without disturbing the row's horizontal rhythm. */}
        <div className="flex">
          {tabs.map((t) => {
            const Icon = t.icon;
            const isActive = activeTab === t.id;
            return (
              <button
                key={t.id}
                type="button"
                onClick={() => setActiveTab(isActive ? null : t.id)}
                className={cn(
                  'group flex flex-1 items-center justify-center gap-1.5 px-3 py-2 text-xs',
                  'border-r border-border/40 last:border-r-0',
                  'transition-colors',
                  isActive
                    ? 'bg-foreground/[0.06] text-foreground'
                    : 'text-muted-foreground hover:bg-foreground/[0.03] hover:text-foreground',
                )}
              >
                <Icon
                  className={cn(
                    'size-3.5 shrink-0',
                    isActive ? 'text-primary' : '',
                  )}
                />
                <span className="font-medium">{t.label}</span>
                {t.badge && (
                  <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                    {t.badge}
                  </span>
                )}
              </button>
            );
          })}
        </div>

        <AnimatePresence initial={false}>
          {activeTab && (
            <motion.div
              key={activeTab}
              variants={panelVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="overflow-hidden"
            >
              <div className="max-h-[40vh] overflow-y-auto border-t border-border/40">
                {activeTab === 'plan' && <PlanContent todos={todos} />}
                {activeTab === 'files' && (
                  <FilesContent
                    entries={fileEntries}
                    onOpenDiff={handleOpenDiff}
                    onRevertPath={handleRevertPath}
                    onRevertAll={handleRevertAll}
                    busyPath={busyRevertPath}
                  />
                )}
                {activeTab === 'terminals' && (
                  <TerminalsContent
                    terminals={agentTerminals}
                    onOpenTerminal={handleOpenTerminal}
                    onCloseTerminal={handleCloseTerminal}
                  />
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

export default AgentToolDock;
