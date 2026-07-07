import React, { useState, useRef, useCallback, useEffect, useMemo } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { FolderGit2, Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useLayout } from '@/state/layout';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';

const islandVariants = {
  hidden: { x: '110%', opacity: 0 },
  visible: {
    x: 0,
    opacity: 1,
    transition: { type: 'spring', stiffness: 380, damping: 28, mass: 0.8 },
  },
  exit: {
    x: '110%',
    opacity: 0,
    transition: { duration: 0.18, ease: [0.36, 0, 0.66, 0] },
  },
};

// Mirrors the task tree's notion of "running": actively streaming or in a
// working status — paused / awaiting-permission / completed don't count.
function isRunning(task, streamingByTask, statusByTask) {
  if (!task) return false;
  if (streamingByTask[task.id]) return true;
  const status = statusByTask[task.id] ?? task.status;
  return status === 'streaming' || status === 'running' || status === 'working' || status === 'preparing';
}

function RunningTaskRow({ project, task, active, onSelect }) {
  return (
    <button
      onClick={() => onSelect(project, task)}
      className={cn(
        'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors',
        'hover:bg-white/10',
        active ? 'bg-primary/15 text-foreground' : 'text-muted-foreground hover:text-foreground',
      )}
    >
      <Loader2 className="size-3 shrink-0 animate-spin text-primary" />
      <span className="min-w-0 flex-1 truncate">
        {task.title || `Task ${String(task.id ?? '').slice(0, 6)}`}
      </span>
    </button>
  );
}

function RunningTaskList() {
  const projects = useExplorer((s) => s.projects);
  const tasksByProject = useAgent((s) => s.tasksByProject);
  const streamingByTask = useAgent((s) => s.streamingByTask);
  const statusByTask = useAgent((s) => s.statusByTask);
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const activeProjectId = useAgent((s) => s.activeProject.id);
  const setActiveTask = useAgent((s) => s.setActiveTask);
  const setActiveProjectInExplorer = useExplorer((s) => s.setActiveProject);

  const groups = useMemo(() => {
    const out = [];
    for (const project of projects) {
      const tasks = tasksByProject[project.id] || [];
      const running = tasks.filter((t) => isRunning(t, streamingByTask, statusByTask));
      if (running.length > 0) out.push({ project, running });
    }
    return out;
  }, [projects, tasksByProject, streamingByTask, statusByTask]);

  // Same switch path as the sidebar task tree: sync the agent's project
  // synchronously before pointing the chat at the task, so the project-sync
  // effect in App.jsx doesn't wipe the freshly selected task.
  const handleSelect = useCallback((project, task) => {
    if (useAgent.getState().activeProject.id !== project.id) {
      useAgent.getState().setActiveProject({
        id: project.id,
        name: project.name,
        root: project.root_path,
      });
      setActiveProjectInExplorer(project.id);
    }
    setActiveTask(task.id);
  }, [setActiveTask, setActiveProjectInExplorer]);

  if (groups.length === 0) {
    return (
      <p className="px-2 py-3 text-center text-xs italic text-muted-foreground">
        No running tasks
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      {groups.map(({ project, running }) => (
        <div key={project.id} className="flex flex-col gap-0.5">
          <div className="flex items-center gap-1.5 px-1 text-[11px] font-semibold uppercase tracking-wide text-foreground/80">
            <FolderGit2 className="size-3 shrink-0" />
            <span className="min-w-0 flex-1 truncate">{project.name}</span>
            <span className="inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-primary/20 px-1 text-[9px] font-medium text-primary">
              {running.length}
            </span>
          </div>
          {running.map((task) => (
            <RunningTaskRow
              key={task.id}
              project={project}
              task={task}
              active={activeProjectId === project.id && activeTaskId === task.id}
              onSelect={handleSelect}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

// Right-edge floating "dynamic island" listing every running task grouped by
// project. Reveals on hover of the screen's right edge (or pinned open via
// the status-bar toggle for touch devices); clicking a task switches to it.
export function TaskIsland() {
  const rightIslandOpen = useLayout((s) => s.rightIslandOpen);
  const [visible, setVisible] = useState(false);
  const hideTimerRef = useRef(null);

  const projects = useExplorer((s) => s.projects);
  const loadTasksForProject = useAgent((s) => s.loadTasksForProject);

  // Running state must be visible across ALL projects, not just the ones the
  // user has expanded in the sidebar — fetch any project list we don't have
  // yet (loadTasksForProject is cached via tasksLoadedByProject).
  useEffect(() => {
    for (const p of projects) {
      loadTasksForProject(p.id).catch(() => {});
    }
  }, [projects, loadTasksForProject]);

  // Cheap boolean subscription — the island (and its edge hint) only exists
  // while at least one task anywhere is running.
  const hasRunning = useAgent((s) => {
    for (const p of projects) {
      for (const t of s.tasksByProject[p.id] || []) {
        if (isRunning(t, s.streamingByTask, s.statusByTask)) return true;
      }
    }
    return false;
  });

  const open = hasRunning && (visible || rightIslandOpen);

  const show = useCallback(() => {
    clearTimeout(hideTimerRef.current);
    setVisible(true);
  }, []);

  const scheduleHide = useCallback(() => {
    hideTimerRef.current = setTimeout(() => setVisible(false), 500);
  }, []);

  useEffect(() => () => clearTimeout(hideTimerRef.current), []);

  if (!hasRunning) return null;

  return (
    <>
      {/* Right-edge trigger strip — the sliver is the always-visible hint. */}
      <div
        className="group fixed right-0 top-0 bottom-6 z-[60] flex w-2 items-center justify-end"
        onMouseEnter={show}
        onMouseLeave={scheduleHide}
        onClick={show}
      >
        <div
          aria-hidden
          className={cn(
            'h-12 w-[3px] rounded-l-full bg-primary/40 transition-all duration-200',
            'group-hover:h-16 group-hover:bg-primary/80',
            open && 'opacity-0',
          )}
        />
      </div>

      {/* Vertical centering wrapper */}
      <div className="pointer-events-none fixed right-0 top-0 bottom-6 z-50 flex items-center">
        <AnimatePresence>
          {open && (
            <motion.div
              key="task-island"
              variants={islandVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className={cn(
                'pointer-events-auto mr-1.5',
                'flex max-h-[70vh] w-64 flex-col overflow-y-auto px-2 py-2.5',
                'rounded-[14px]',
                'border border-white/[0.09]',
                'bg-background/65 backdrop-blur-2xl',
                'shadow-[0_8px_32px_rgba(0,0,0,0.55),inset_0_1px_0_rgba(255,255,255,0.05)]',
              )}
              onMouseEnter={show}
              onMouseLeave={scheduleHide}
            >
              <p className="mb-1.5 px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                Running tasks
              </p>
              <RunningTaskList />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </>
  );
}
