import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight,
  FolderGit2,
  Plus,
  Loader2,
  MessageSquare,
  Pencil,
  Trash2,
  Sparkles,
} from 'lucide-react';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { useExplorer } from '@/state/explorer';
import { useAgent } from '@/state/agent';
import { AddProjectButton } from '@/components/shell/add-project-button';
import { confirm } from '@/components/confirm-dialog';
import { cn } from '@/lib/utils';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

// A task counts as "running" when the model is actively streaming or making
// tool calls — paused / awaiting-permission / completed do not. We trust the
// local `streamingByTask` flag first (it's wired to the streaming events) and
// fall back to the task's persisted status from the backend, so the tree can
// still surface running state for projects the user hasn't visited yet.
function isRunning(task, streamingByTask, statusByTask) {
  if (!task) return false;
  if (streamingByTask[task.id]) return true;
  const status = statusByTask[task.id] ?? task.status;
  return status === 'streaming' || status === 'running' || status === 'working';
}

// Tasks come back from the backend in some order; we normalize on the client
// so running tasks float to the top and the rest are sorted newest-first by
// whatever timestamp the backend includes. Falls back to the original order
// when no timestamp is present.
function sortTasks(tasks, streamingByTask, statusByTask) {
  const withIndex = tasks.map((t, i) => ({ t, i }));
  withIndex.sort((a, b) => {
    const ar = isRunning(a.t, streamingByTask, statusByTask);
    const br = isRunning(b.t, streamingByTask, statusByTask);
    if (ar !== br) return ar ? -1 : 1;
    const at = Number(a.t.updated_at ?? a.t.created_at ?? 0);
    const bt = Number(b.t.updated_at ?? b.t.created_at ?? 0);
    if (at !== bt) return bt - at;
    return a.i - b.i;
  });
  return withIndex.map((x) => x.t);
}

function TaskRow({ project, task, active, running, onSelect, onRename, onDelete }) {
  return (
    <div
      role="button"
      onClick={() => onSelect(project, task)}
      className={cn(
        'group/task flex h-7 cursor-pointer items-center gap-1.5 pl-6 pr-2 text-xs',
        'hover:bg-foreground/[0.06]',
        active && 'bg-primary/15 text-foreground',
      )}
    >
      <span className="flex size-3 shrink-0 items-center justify-center">
        {running ? (
          <Loader2 className="size-3 animate-spin text-primary" />
        ) : (
          <MessageSquare className="size-3 text-muted-foreground" />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate">
        {task.title || `Task ${String(task.id ?? '').slice(0, 6)}`}
      </span>
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
    </div>
  );
}

function ProjectNode({ project, onSelectTask }) {
  const expanded = useAgent((s) => s.expandedProjects[project.id] !== false);
  const toggle = useAgent((s) => s.toggleProjectExpanded);
  const loadTasks = useAgent((s) => s.loadTasksForProject);
  const tasks = useAgent((s) => s.tasksByProject[project.id]) ?? EMPTY_TASKS;
  const loaded = useAgent((s) => !!s.tasksLoadedByProject[project.id]);
  const historyLimit = useAgent((s) => s.historyLimitByProject[project.id] || 5);
  const bumpHistoryLimit = useAgent((s) => s.bumpHistoryLimit);
  const streamingByTask = useAgent((s) => s.streamingByTask);
  const statusByTask = useAgent((s) => s.statusByTask);
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const activeProjectId = useAgent((s) => s.activeProject.id);
  const removeTaskFromCache = useAgent((s) => s.removeTaskFromCache);

  // Lazy fetch on first expand. Default-expanded projects (the active one,
  // newly added ones) will trigger this on mount.
  useEffect(() => {
    if (expanded && !loaded) {
      loadTasks(project.id).catch(() => {});
    }
  }, [expanded, loaded, project.id, loadTasks]);

  const { running, history, hiddenCount } = useMemo(() => {
    const sorted = sortTasks(tasks, streamingByTask, statusByTask);
    const runningList = [];
    const restList = [];
    for (const t of sorted) {
      if (isRunning(t, streamingByTask, statusByTask)) runningList.push(t);
      else restList.push(t);
    }
    const shown = restList.slice(0, historyLimit);
    return {
      running: runningList,
      history: shown,
      hiddenCount: Math.max(0, restList.length - shown.length),
    };
  }, [tasks, streamingByTask, statusByTask, historyLimit]);

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

  const handleRename = async (proj, task) => {
    const next = window.prompt('Rename task:', task.title ?? '');
    if (!next || next === task.title || !isTauri()) return;
    try {
      await invoke('rename_task', { taskId: task.id, title: next });
      await loadTasks(proj.id, { force: true });
    } catch (err) {
      toast.error(String(err));
    }
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
  const allRows = [...running, ...history];

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
        {running.length > 0 && (
          <span
            className="ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-1 text-[9px] font-medium text-primary"
            title={`${running.length} running`}
          >
            {running.length}
          </span>
        )}
        <button
          onClick={handleCreate}
          title="New task in this project"
          className="flex size-5 items-center justify-center rounded opacity-0 transition-opacity hover:bg-foreground/10 group-hover/project:opacity-100"
        >
          <Plus className="size-3" />
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
              running={isRunning(task, streamingByTask, statusByTask)}
              onSelect={onSelectTask}
              onRename={handleRename}
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
      setActiveProjectInExplorer(project.id);
    }
    setActiveTask(task.id);
  };

  return (
    <div className="flex h-full flex-col bg-sidebar">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          <Sparkles className="size-3" />
          Agent
        </span>
        <div className="flex items-center gap-1">
          <AddProjectButton />
        </div>
      </div>
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
          <ProjectNode key={p.id} project={p} onSelectTask={handleSelectTask} />
        ))}
      </div>
    </div>
  );
}

export default AgentTaskTree;
