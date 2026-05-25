import React, { useEffect, useMemo, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  CheckCircle2,
  Circle,
  CircleDotDashed,
  FileEdit,
  ListChecks,
  TerminalSquare,
} from 'lucide-react';
import { useAgent } from '@/state/agent';
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
      <div className="px-4 py-6 text-center text-xs text-muted-foreground">
        The agent hasn't published a plan for this task yet.
      </div>
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

function PlaceholderContent({ icon: Icon, text, hint }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-4 py-6 text-center text-xs text-muted-foreground">
      <Icon className="size-5 text-muted-foreground/50" />
      <div className="font-medium text-foreground/80">{text}</div>
      {hint && <div className="text-[11px] leading-snug">{hint}</div>}
    </div>
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

  // Active tab is local: it's a viewing preference, not part of agent state
  // that needs to persist across reloads. Keyed by task so switching tasks
  // doesn't carry over an unrelated open panel.
  const [activeByTask, setActiveByTask] = useState({});
  const activeTab = activeTaskId ? activeByTask[activeTaskId] || null : null;
  const setActiveTab = (val) => {
    if (!activeTaskId) return;
    setActiveByTask((prev) => ({ ...prev, [activeTaskId]: val }));
  };

  // Auto-open the Plan tab the first time todos appear for a task — same
  // "the agent just published something, the user should notice" trigger we
  // had on the standalone TodoPanel. Tracked per task so re-renders don't
  // keep slamming the panel open after the user collapses it.
  const [autoOpened, setAutoOpened] = useState({});
  useEffect(() => {
    if (!activeTaskId) return;
    if (todos.length === 0) return;
    if (autoOpened[activeTaskId]) return;
    setAutoOpened((prev) => ({ ...prev, [activeTaskId]: true }));
    setActiveByTask((prev) =>
      prev[activeTaskId] === undefined
        ? { ...prev, [activeTaskId]: 'plan' }
        : prev,
    );
  }, [activeTaskId, todos.length, autoOpened]);

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
      label: 'Files',
      badge: null,
    },
    {
      id: 'terminals',
      icon: TerminalSquare,
      label: 'Terminals',
      badge: null,
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
                  <PlaceholderContent
                    icon={FileEdit}
                    text="No file changes tracked yet"
                    hint="The agent's writes will be listed here once tracking is wired up."
                  />
                )}
                {activeTab === 'terminals' && (
                  <PlaceholderContent
                    icon={TerminalSquare}
                    text="No active agent terminals"
                    hint="Long-running shells the agent spawns will appear here."
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
