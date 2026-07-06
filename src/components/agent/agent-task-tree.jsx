import React, { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import {
  ChevronRight,
  FolderGit2,
  Plus,
  Loader2,
  MessageSquare,
  Pencil,
  Trash2,
  Sparkles,
  ListCollapse,
  ListChecks,
  CheckCheck,
  Star,
  Terminal,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Skeleton } from '@/components/ui/skeleton';
import { useExplorer } from '@/state/explorer';
import { useAgent } from '@/state/agent';
import { useTerminal } from '@/state/terminal';
import { useEditor } from '@/state/editor';
import { AddProjectButton } from '@/components/shell/add-project-button';
import { confirm } from '@/components/confirm-dialog';
import { useRelativeTime } from '@/lib/relative-time';
import { cn } from '@/lib/utils';


// A task counts as "running" when the model is actively streaming or making
// tool calls — paused / awaiting-permission / completed do not. We trust the
// local `streamingByTask` flag first (it's wired to the streaming events) and
// fall back to the task's persisted status from the backend, so the tree can
// still surface running state for projects the user hasn't visited yet.
function isRunning(task, streamingByTask, statusByTask) {
  if (!task) return false;
  if (streamingByTask[task.id]) return true;
  const status = statusByTask[task.id] ?? task.status;
  return status === 'streaming' || status === 'running' || status === 'working' || status === 'preparing';
}

// Tasks come back from the backend in some order; we normalize on the client
// so pinned tasks stick to the very top (sticky notes), running tasks float
// next, and the rest are sorted newest-first by whatever timestamp the
// backend includes. Falls back to the original order when no timestamp is
// present.
function sortTasks(tasks, streamingByTask, statusByTask) {
  const withIndex = tasks.map((t, i) => ({ t, i }));
  withIndex.sort((a, b) => {
    const ap = !!a.t.pinned;
    const bp = !!b.t.pinned;
    if (ap !== bp) return ap ? -1 : 1;
    const ar = isRunning(a.t, streamingByTask, statusByTask);
    const br = isRunning(b.t, streamingByTask, statusByTask);
    if (ar !== br) return ar ? -1 : 1;
    const at = new Date(a.t.updated_at ?? a.t.created_at ?? 0).getTime();
    const bt = new Date(b.t.updated_at ?? b.t.created_at ?? 0).getTime();
    if (at !== bt) return bt - at;
    return a.i - b.i;
  });
  return withIndex.map((x) => x.t);
}

// Inline rename field swapped into a task row. Mounted fresh per rename so
// the done-guard ref resets; the guard stops Enter's commit and the ensuing
// blur from double-firing.
function InlineRenameInput({ initial, onCommit, onCancel }) {
  const doneRef = useRef(false);
  const finish = (commit, value) => {
    if (doneRef.current) return;
    doneRef.current = true;
    if (commit) onCommit(value);
    else onCancel();
  };
  return (
    <input
      autoFocus
      defaultValue={initial}
      onFocus={(e) => e.currentTarget.select()}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(true, e.currentTarget.value);
        else if (e.key === 'Escape') finish(false);
      }}
      onBlur={(e) => finish(true, e.currentTarget.value)}
      aria-label="Rename task"
      className="min-w-0 flex-1 rounded border border-border bg-background px-1 py-0.5 text-xs text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    />
  );
}

const WT_CHIP = {
  queued: ['queued', 'border-violet-500/40 text-violet-500'],
  merging: ['merging', 'border-violet-500/40 text-violet-500 animate-pulse'],
  'needs-reconciliation': ['conflict', 'border-rose-500/40 text-rose-500'],
};

function TaskRow({
  project,
  task,
  active,
  running,
  multiSelect,
  selected,
  renaming,
  onSelect,
  onToggleSelect,
  onTogglePin,
  onRename,
  onRenameCommit,
  onRenameCancel,
  onDelete,
}) {
  const timestampMs = useMemo(() => {
    const t = new Date(task.updated_at ?? task.created_at ?? 0).getTime();
    return Number.isFinite(t) && t > 0 ? t : null;
  }, [task.updated_at, task.created_at]);
  const relative = useRelativeTime(timestampMs);
  const worktree = useAgent((s) => s.worktreeByTask[task.id]);
  const handleClick = () => {
    if (renaming) return;
    if (multiSelect) onToggleSelect(project, task);
    else onSelect(project, task);
  };
  return (
    <div
      role="button"
      onClick={handleClick}
      className={cn(
        'group/task flex h-7 cursor-pointer items-center gap-1.5 pl-6 pr-2 text-xs',
        'hover:bg-foreground/[0.06]',
        active && !multiSelect && 'bg-primary/15 text-foreground',
        multiSelect && selected && 'bg-primary/10',
      )}
    >
      {multiSelect ? (
        <Checkbox
          checked={selected}
          onCheckedChange={() => onToggleSelect(project, task)}
          onClick={(e) => e.stopPropagation()}
          className="size-3.5 shrink-0"
          aria-label={selected ? 'Deselect chat' : 'Select chat'}
        />
      ) : (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onTogglePin(project, task);
          }}
          title={task.pinned ? 'Unpin' : 'Pin to top'}
          aria-label={task.pinned ? 'Unpin task' : 'Pin task to top'}
          className="flex size-3 shrink-0 items-center justify-center"
        >
          {task.pinned ? (
            <Star className={cn('size-3 fill-amber-400 text-amber-400', running && 'animate-pulse')} />
          ) : running ? (
            <Loader2 className="size-3 animate-spin text-primary" />
          ) : (
            <>
              <MessageSquare className="size-3 text-muted-foreground group-hover/task:hidden" />
              <Star className="hidden size-3 text-muted-foreground group-hover/task:block" />
            </>
          )}
        </button>
      )}
      {renaming ? (
        <InlineRenameInput
          initial={task.title || ''}
          onCommit={(value) => onRenameCommit(project, task, value)}
          onCancel={onRenameCancel}
        />
      ) : (
        <span className="min-w-0 flex-1 truncate">
          {task.title || `Task ${String(task.id ?? '').slice(0, 6)}`}
        </span>
      )}
      {!multiSelect && !renaming && worktree && WT_CHIP[worktree.state] && (
        <span
          title={`Worktree: ${worktree.state}`}
          className={cn(
            'shrink-0 rounded border px-1 text-[9px] font-medium leading-4',
            WT_CHIP[worktree.state][1],
          )}
        >
          {WT_CHIP[worktree.state][0]}
        </span>
      )}
      {!multiSelect && !renaming && relative && (
        <span className="ml-auto shrink-0 select-none text-[10px] tabular-nums text-muted-foreground/70 group-hover/task:hidden">
          {relative}
        </span>
      )}
      {!multiSelect && !renaming && (
        <div className="ml-auto flex items-center gap-0.5 opacity-0 transition-opacity group-hover/task:opacity-100">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onRename(project, task);
            }}
            title="Rename"
            className="flex size-4 items-center justify-center rounded hover:bg-foreground/10"
          >
            <Pencil className="size-2.5" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onDelete(project, task);
            }}
            title="Delete"
            className="flex size-4 items-center justify-center rounded hover:bg-destructive/20 hover:text-destructive"
          >
            <Trash2 className="size-2.5" />
          </button>
        </div>
      )}
    </div>
  );
}

function ProjectNode({ project, onSelectTask, multiSelect, selectedMap, onToggleSelect }) {
  const expanded = useAgent((s) => !!s.expandedProjects[project.id]);
  const toggle = useAgent((s) => s.toggleProjectExpanded);
  const loadTasks = useAgent((s) => s.loadTasksForProject);
  const tasks = useAgent((s) => s.tasksByProject[project.id]) ?? EMPTY_TASKS;
  const loaded = useAgent((s) => !!s.tasksLoadedByProject[project.id]);
  const historyLimit = useAgent((s) => s.historyLimitByProject[project.id] || 5);
  const bumpHistoryLimit = useAgent((s) => s.bumpHistoryLimit);
  // Don't subscribe to the whole streamingByTask map — its identity churns
  // with unrelated tasks' stream activity. Subscribe to a string signature of
  // THIS project's streaming task ids instead: Object.is on equal strings
  // means the component only re-renders when one of its own tasks actually
  // starts/stops streaming.
  const streamingSig = useAgent((s) => {
    const list = s.tasksByProject[project.id];
    if (!list || list.length === 0) return '';
    let sig = '';
    for (const t of list) if (s.streamingByTask[t.id]) sig += `${t.id}|`;
    return sig;
  });
  const statusByTask = useAgent((s) => s.statusByTask);
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const activeProjectId = useAgent((s) => s.activeProject.id);
  const removeTaskFromCache = useAgent((s) => s.removeTaskFromCache);
  const removeProject = useExplorer((s) => s.removeProject);

  // Lazy fetch on first expand. Projects are collapsed by default, so tasks
  // load the first time the user expands a project.
  useEffect(() => {
    if (expanded && !loaded) {
      loadTasks(project.id).catch(() => {});
    }
  }, [expanded, loaded, project.id, loadTasks]);

  const { pinned, running, history, hiddenCount, runningCount, runningIds } = useMemo(() => {
    // Read the map non-reactively — streamingSig in the dep list already
    // captures every change that matters to this project.
    const streamingByTask = useAgent.getState().streamingByTask;
    const sorted = sortTasks(tasks, streamingByTask, statusByTask);
    const pinnedList = [];
    const runningList = [];
    const restList = [];
    const runningIdSet = new Set();
    for (const t of sorted) {
      if (isRunning(t, streamingByTask, statusByTask)) runningIdSet.add(t.id);
      if (t.pinned) pinnedList.push(t);
      else if (runningIdSet.has(t.id)) runningList.push(t);
      else restList.push(t);
    }
    const shown = restList.slice(0, historyLimit);
    return {
      pinned: pinnedList,
      running: runningList,
      history: shown,
      hiddenCount: Math.max(0, restList.length - shown.length),
      runningCount: runningIdSet.size,
      runningIds: runningIdSet,
    };
  }, [tasks, streamingSig, statusByTask, historyLimit]);

  const handleCreate = (e) => {
    e.stopPropagation();
    // Don't materialize a backend task yet — just switch the active project to
    // this one and clear activeTaskId. sendMessage → ensureTask creates the
    // task lazily on first send, so spamming "+" no longer leaves a trail of
    // empty "New Task" rows in the sidebar / DB.
    if (useExplorer.getState().activeProjectId !== project.id) {
      useExplorer.getState().setActiveProject(project.id);
    }
    useAgent.setState({ activeTaskId: null });
  };

  const handleRemove = async (e) => {
    e.stopPropagation();
    try {
      await removeProject(project.id);
    } catch (err) {
      toast.error(String(err));
    }
  };

  // Open a terminal rooted at this project — same path the file explorer uses:
  // spawn the PTY, then surface it in the bottom panel via the editor store.
  const handleOpenTerminal = async (e) => {
    e.stopPropagation();
    const cwd = project.root_path || project.root;
    try {
      const info = await useTerminal
        .getState()
        .createTerminal({ cwd, label: project.name });
      useEditor.getState().openTerminal(info.id, project.name);
      toast.success(`Terminal opened in ${project.name}`);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const [renamingTaskId, setRenamingTaskId] = useState(null);

  const handleRename = (proj, task) => {
    setRenamingTaskId(task.id);
  };

  const handleRenameCommit = async (proj, task, value) => {
    setRenamingTaskId(null);
    const next = (value || '').trim();
    if (!next || next === task.title || !isTauri()) return;
    try {
      await invoke('rename_task', { taskId: task.id, title: next });
      await loadTasks(proj.id, { force: true });
    } catch (err) {
      toast.error(String(err));
    }
  };

  const handleRenameCancel = () => setRenamingTaskId(null);

  const handleTogglePin = (proj, task) => {
    useAgent
      .getState()
      .setTaskPinned(proj.id, task.id, !task.pinned)
      .catch((err) => toast.error(String(err)));
  };

  const handleDelete = async (proj, task) => {
    const label = task.title || task.id;
    const shortLabel = label.length > 60 ? `${label.slice(0, 60)}…` : label;
    const ok = await confirm({
      title: 'Delete task?',
      description: `"${shortLabel}"\nAll messages will be removed. This can't be undone.`,
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    try {
      if (isTauri()) await invoke('delete_task', { taskId: task.id });
      removeTaskFromCache(proj.id, task.id);
      if (useAgent.getState().activeTaskId === task.id) {
        useAgent.setState({ activeTaskId: null });
      }
    } catch (err) {
      toast.error(String(err));
    }
  };

  const isActiveProject = activeProjectId === project.id;
  const allRows = [...pinned, ...running, ...history];

  return (
    <div className="flex flex-col border-b border-border/60 last:border-b-0">
      <div
        onClick={() => toggle(project.id)}
        className="group/project sticky top-0 z-10 flex h-7 cursor-pointer items-center gap-1 border-b border-border/60 bg-muted/60 px-2 text-[11px] font-semibold uppercase tracking-wide text-foreground/90 backdrop-blur hover:bg-muted/80"
      >
        <ChevronRight
          className="size-3 shrink-0 transition-transform duration-200 ease-in-out"
          style={{ transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
        />
        <FolderGit2 className="size-3 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{project.name}</span>
        {runningCount > 0 && (
          <span
            className="ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-1 text-[9px] font-medium text-primary"
            title={`${runningCount} running`}
          >
            {runningCount}
          </span>
        )}
        <button
          onClick={handleCreate}
          title="New task in this project"
          className="flex size-5 items-center justify-center rounded opacity-0 transition-opacity hover:bg-foreground/10 group-hover/project:opacity-100"
        >
          <Plus className="size-3" />
        </button>
        <button
          onClick={handleOpenTerminal}
          title="Open terminal in project root"
          className="flex size-5 items-center justify-center rounded opacity-0 transition-opacity hover:bg-foreground/10 group-hover/project:opacity-100"
        >
          <Terminal className="size-3" />
        </button>
        <button
          onClick={handleRemove}
          title="Remove project from workspace"
          className="flex size-5 items-center justify-center rounded opacity-0 transition-opacity hover:bg-destructive/20 hover:text-destructive group-hover/project:opacity-100"
        >
          <X className="size-3" />
        </button>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 220ms ease',
        }}
      >
        <div style={{ overflow: 'hidden' }}>
          {expanded && !loaded && tasks.length === 0 && (
            <div className="flex flex-col gap-1 px-2 py-1.5">
              <Skeleton className="ml-3 h-4 w-3/4" />
              <Skeleton className="ml-3 h-4 w-2/3" />
            </div>
          )}
          {expanded && loaded && allRows.length === 0 && (
            <div
              className="px-6 py-2 text-xs text-muted-foreground italic cursor-pointer hover:text-foreground"
              onClick={handleCreate}
            >
              No tasks — click + to start one
            </div>
          )}
          {allRows.map((task) => (
            <TaskRow
              key={task.id}
              project={project}
              task={task}
              active={isActiveProject && activeTaskId === task.id}
              running={runningIds.has(task.id)}
              multiSelect={multiSelect}
              selected={!!selectedMap?.[task.id]}
              renaming={renamingTaskId === task.id}
              onSelect={onSelectTask}
              onToggleSelect={onToggleSelect}
              onTogglePin={handleTogglePin}
              onRename={handleRename}
              onRenameCommit={handleRenameCommit}
              onRenameCancel={handleRenameCancel}
              onDelete={handleDelete}
            />
          ))}
          {hiddenCount > 0 && (
            <button
              onClick={() => bumpHistoryLimit(project.id, 5)}
              className="flex h-6 w-full items-center gap-1.5 pl-6 pr-2 text-[11px] text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground"
            >
              <span className="size-3" />
              <span>Load more ({hiddenCount})</span>
            </button>
          )}

          {/* Bottom gap — matching the Explorer's spacing */}
          {expanded && <div className="h-16 w-full" />}
        </div>
      </div>
    </div>
  );
}

const EMPTY_TASKS = [];

export function AgentTaskTree() {
  const projects = useExplorer((s) => s.projects);
  const loading = useExplorer((s) => s.loading);
  const error = useExplorer((s) => s.error);
  const loadProjects = useExplorer((s) => s.loadProjects);
  const setActiveProjectInExplorer = useExplorer((s) => s.setActiveProject);
  const loadInitial = useAgent((s) => s.loadInitial);
  const bindListeners = useAgent((s) => s.bindListeners);
  const setActiveTask = useAgent((s) => s.setActiveTask);
  const collapseAllProjects = useAgent((s) => s.collapseAllProjects);

  // Multi-select mode for bulk-deleting chats. `selected` maps taskId →
  // projectId so a bulk delete knows which project's cache to evict each task
  // from. Toggling the mode off clears the selection.
  const [multiSelect, setMultiSelect] = useState(false);
  const [selected, setSelected] = useState({});
  const selectedCount = Object.keys(selected).length;

  const toggleMultiSelect = () => {
    setMultiSelect((on) => {
      if (on) setSelected({});
      return !on;
    });
  };

  const toggleTaskSelected = (project, task) => {
    setSelected((prev) => {
      const next = { ...prev };
      if (next[task.id]) delete next[task.id];
      else next[task.id] = project.id;
      return next;
    });
  };

  const selectAllLoaded = () => {
    const tbp = useAgent.getState().tasksByProject || {};
    const next = {};
    for (const proj of projects) {
      for (const t of tbp[proj.id] || []) next[t.id] = proj.id;
    }
    setSelected(next);
  };

  const handleBulkDelete = async () => {
    const entries = Object.entries(selected);
    if (entries.length === 0) return;
    const n = entries.length;
    const ok = await confirm({
      title: `Delete ${n} chat${n > 1 ? 's' : ''}?`,
      description: `All messages in the selected chat${n > 1 ? 's' : ''} will be removed. This can't be undone.`,
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    const agent = useAgent.getState();
    let failed = 0;
    for (const [taskId, projectId] of entries) {
      try {
        if (isTauri()) await invoke('delete_task', { taskId });
        agent.removeTaskFromCache(projectId, taskId);
        if (useAgent.getState().activeTaskId === taskId) {
          useAgent.setState({ activeTaskId: null });
        }
      } catch (err) {
        failed += 1;
      }
    }
    if (failed) toast.error(`Failed to delete ${failed} chat${failed > 1 ? 's' : ''}`);
    else toast.success(`Deleted ${n} chat${n > 1 ? 's' : ''}`);
    setSelected({});
    setMultiSelect(false);
  };

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Wire agent event listeners as soon as the tree mounts so running indicators
  // on tasks across projects can update without the chat dock having to mount
  // first. The hook is idempotent — calling it twice is a no-op.
  useEffect(() => {
    loadInitial();
    let cleanup;
    bindListeners().then((fn) => {
      cleanup = fn;
    });
    return () => {
      if (typeof cleanup === 'function') cleanup();
    };
  }, [loadInitial, bindListeners]);

  const handleSelectTask = (project, task) => {
    // Switch active project first (this also seeds the per-task transient
    // state and rehydrates the flat `tasks` mirror) — then point the chat at
    // the picked task.
    if (useAgent.getState().activeProject.id !== project.id) {
      // Update the agent store synchronously alongside the explorer so the
      // setActiveTask + loadTaskHistory below run with the correct project
      // root. Without this synchronous sync, App.jsx's useActiveProjectSync
      // effect would later call setActiveProject — which clears activeTaskId
      // / messagesByTask / historyLoadedByTask — and wipe out the task we
      // just selected, leaving the chat blank.
      useAgent.getState().setActiveProject({
        id: project.id,
        name: project.name,
        root: project.root_path,
      });
      setActiveProjectInExplorer(project.id);
    }
    setActiveTask(task.id);
  };

  const handleCollapseAll = () => {
    collapseAllProjects(projects.map((p) => p.id));
  };

  return (
    <div className="flex h-full flex-col bg-sidebar">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          <Sparkles className="size-3" />
          Agent
        </span>
        <div className="flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={toggleMultiSelect}
                aria-pressed={multiSelect}
                className={cn(multiSelect && 'bg-primary/15 text-primary')}
              >
                <ListChecks className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              {multiSelect ? 'Exit multi-select' : 'Select multiple chats'}
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={handleCollapseAll}>
                <ListCollapse className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              Collapse All
            </TooltipContent>
          </Tooltip>
          <AddProjectButton />
        </div>
      </div>

      {multiSelect && (
        <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-border/60 bg-muted/40 px-2 text-xs">
          <span className="min-w-0 flex-1 truncate text-muted-foreground">
            {selectedCount > 0 ? `${selectedCount} selected` : 'Select chats'}
          </span>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={selectAllLoaded}>
                <CheckCheck className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              Select all
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={() => setSelected({})}
                disabled={selectedCount === 0}
              >
                <X className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              Clear selection
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="destructive"
                size="icon-xs"
                onClick={handleBulkDelete}
                disabled={selectedCount === 0}
              >
                <Trash2 className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              Delete selected
            </TooltipContent>
          </Tooltip>
        </div>
      )}
      <div className="explorer-scroll min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        {loading && projects.length === 0 && (
          <div className="flex flex-col gap-1 px-2 py-2">
            <Skeleton className="h-5 w-3/4" />
            <Skeleton className="ml-3 h-4 w-2/3" />
            <Skeleton className="h-5 w-2/3" />
            <Skeleton className="ml-3 h-4 w-1/2" />
          </div>
        )}
        {error && (
          <div className="px-3 py-4 text-xs text-destructive">Error: {error}</div>
        )}
        {!loading && projects.length === 0 && !error && (
          <div className="flex flex-col items-start gap-2 px-3 py-4 text-xs text-muted-foreground">
            <span>No projects added yet.</span>
            <AddProjectButton />
          </div>
        )}
        {projects.map((p) => (
          <ProjectNode
            key={p.id}
            project={p}
            onSelectTask={handleSelectTask}
            multiSelect={multiSelect}
            selectedMap={selected}
            onToggleSelect={toggleTaskSelected}
          />
        ))}
      </div>
    </div>
  );
}

export default AgentTaskTree;
