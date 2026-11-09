import React, { useEffect, useMemo, useState } from 'react';
import { Plus, X, GripVertical, ArrowUpRight, Folder } from 'lucide-react';
import { useExplorer } from '@/state/explorer';
import { useAgent } from '@/state/agent';
import { useLayout } from '@/state/layout';
import {
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  closestCenter,
} from '@dnd-kit/core';
import {
  restrictToVerticalAxis,
  restrictToParentElement,
} from '@dnd-kit/modifiers';
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
  arrayMove,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { confirm } from '@/components/confirm-dialog';
import { useTerminal, orderedSessions, terminalTabLabel } from '@/state/terminal';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';
import { TerminalPane } from './lazy-terminal-pane';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';

// One-click new terminal → project picker (same dialog the title-bar "+"
// opens) so the user chooses which project's root to open in.
const openTerminalPicker = () =>
  window.dispatchEvent(new Event(TERMINAL_PICKER_EVENT));

/** A draggable, selectable terminal tab in the sidebar list. */
function SortableTab({ session, active, onSelect, onClose }) {
  const override = useTerminal((s) => s.labelOverrides[session.id]);
  const renameTerminal = useTerminal((s) => s.renameTerminal);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const label = terminalTabLabel(session, override);

  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: session.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    // Lift the row being dragged above its siblings and dim it slightly so the
    // drop target underneath stays readable.
    zIndex: isDragging ? 10 : undefined,
    opacity: isDragging ? 0.6 : 1,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        'group flex items-center border-b border-border/60 px-1 py-1.5 cursor-pointer text-xs',
        active
          ? 'bg-muted text-foreground'
          : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
      )}
      onClick={() => onSelect(session.id)}
    >
      {/* Drag handle — dragging only starts from here so clicking the row still
          selects the terminal and clicking X still closes it. */}
      <button
        {...attributes}
        {...listeners}
        onClick={(e) => e.stopPropagation()}
          className="mr-0.5 cursor-grab touch-none text-muted-foreground/50 opacity-0 hover:text-foreground focus-visible:opacity-100 active:cursor-grabbing group-hover:opacity-100 touch-reveal"
        title="Drag to reorder"
        aria-label="Drag to reorder terminal"
      >
        <GripVertical className="size-3" />
      </button>
      {/* Status dot: an in-flight command pulses amber, a live-but-idle shell
          shows a steady green, and a retired one a muted dot. */}
      <span
        aria-hidden
        title={terminalStatusTitle(session)}
        className={cn(
          'mr-1 size-1.5 shrink-0 rounded-full',
          session.exited
            ? 'bg-muted-foreground/40'
            : session.running
                  ? 'animate-pulse bg-warning'
                  : session.background_job
                    ? 'bg-success ring-2 ring-info/50'
                    : 'bg-success',
        )}
      />
      {editing ? (
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter') {
              renameTerminal(session.id, draft);
              setEditing(false);
            } else if (e.key === 'Escape') {
              setEditing(false);
            }
          }}
          onBlur={() => setEditing(false)}
          className="min-w-0 flex-1 rounded border border-border bg-background px-1 text-xs text-foreground outline-none"
          aria-label="Rename terminal"
        />
      ) : (
        <span
          className={cn('truncate flex-1', session.exited && 'line-through opacity-60')}
          title={session.exited ? `${label} (exited)` : label}
          onDoubleClick={(e) => {
            e.stopPropagation();
            setDraft(override ?? session.label ?? '');
            setEditing(true);
          }}
        >
          {label}
        </span>
      )}
      <button
        onClick={(e) => {
          e.stopPropagation();
          onClose(session.id);
        }}
            className="ml-1 rounded p-px text-muted-foreground opacity-0 hover:bg-destructive/20 hover:text-destructive focus-visible:opacity-100 group-hover:opacity-100 touch-reveal touch-target"
        title="Terminate terminal"
        aria-label="Terminate terminal"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}

/** Hover text for a terminal's status dot. */
function terminalStatusTitle(session) {
  if (session.exited) return 'Shell exited';
  if (session.running) return 'Command running';
  if (session.background_job) return 'Idle · background job running';
  return 'Idle at prompt';
}

/** Normalise a path for prefix comparison (case-insensitive, forward slashes, no trailing slash). */
const normPath = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();

/** Find the explorer project whose root is the longest prefix of `cwd`. */
function projectForCwd(projects, cwd) {
  const c = normPath(cwd);
  let best = null;
  for (const p of projects) {
    const r = normPath(p.root_path);
    if (!r || (c !== r && !c.startsWith(r + '/'))) continue;
    if (!best || r.length > normPath(best.root_path).length) best = p;
  }
  return best;
}

/** One live agent terminal in the Agent view list. */
function AgentTerminalRow({ session, chatTitle, active, onSelect, onOpenChat, onClose }) {
  const override = useTerminal((s) => s.labelOverrides[session.id]);
  const label = terminalTabLabel(session, override);
  return (
    <div
      className={cn(
        'group flex items-start gap-1.5 border-b border-border/60 px-2 py-1.5 cursor-pointer text-xs',
        active ? 'bg-muted text-foreground' : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
      )}
      onClick={() => onSelect(session.id)}
    >
      <span
        aria-hidden
        title={terminalStatusTitle(session)}
        className={cn(
          'mt-1 size-1.5 shrink-0 rounded-full',
          session.running ? 'animate-pulse bg-warning' : session.background_job ? 'bg-success ring-2 ring-info/50' : 'bg-success',
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="truncate" title={label}>{label}</div>
        {chatTitle && <div className="truncate text-[10.5px] text-muted-foreground/80" title={chatTitle}>{chatTitle}</div>}
        {!session.running && session.background_job && (
          <div className="truncate text-[10px] text-muted-foreground/80">idle · background job</div>
        )}
        {session.last_command && (
          <div className="truncate font-mono text-[10px] text-muted-foreground/70" title={session.last_command}>
            $ {session.last_command}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {session.task_id && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onOpenChat(session);
            }}
            className="rounded p-px text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 touch-reveal touch-target"
            title="Go to chat"
            aria-label="Go to the chat that owns this terminal"
          >
            <ArrowUpRight className="size-3" />
          </button>
        )}
        <button
          onClick={(e) => {
            e.stopPropagation();
            onClose(session.id);
          }}
          className="rounded p-px text-muted-foreground opacity-0 hover:bg-destructive/20 hover:text-destructive focus-visible:opacity-100 group-hover:opacity-100 touch-reveal touch-target"
          title="Terminate terminal"
          aria-label="Terminate terminal"
        >
          <X className="size-3" />
        </button>
      </div>
    </div>
  );
}

export function TerminalPanel() {
  const terminalView = useTerminal((s) => s.terminalView);
  const projects = useExplorer((s) => s.projects);
  const tasksByProject = useAgent((s) => s.tasksByProject);
  const flatTasks = useAgent((s) => s.tasks);
  const [agentPick, setAgentPick] = useState(null);
  const allSessions = useTerminal((s) => s.sessions);
  const hiddenSessionIds = useTerminal((s) => s.hiddenSessionIds);
  const activeSessionId = useTerminal((s) => s.activeSessionId);
  const order = useTerminal((s) => s.order);
  const reorderTerminals = useTerminal((s) => s.reorderTerminals);
  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);
  const closeTerminal = useTerminal((s) => s.closeTerminal);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);
  const labelOverrides = useTerminal((s) => s.labelOverrides);

  const onCloseTab = async (id) => {
    const session = allSessions.find((x) => x.id === id);
    // Exited shells have nothing running — close without the kill confirm.
    if (session?.exited) {
      closeTerminal(id);
      return;
    }
    const ok = await confirm({
      title: 'Terminate terminal',
      description: `Terminate "${terminalTabLabel(session, labelOverrides[id])}"? Any process running in it will be killed.`,
      confirmLabel: 'Terminate',
      destructive: true,
      rememberKey: 'terminal-close',
    });
    if (ok) closeTerminal(id);
  };

  useEffect(() => {
    wireListeners();
    refreshSessions();
  }, [wireListeners, refreshSessions]);

  // Filter out hidden terminals.
  const sessions = allSessions.filter((s) => !hiddenSessionIds.has(s.id));
  // The active terminal: keep the current one if still alive, otherwise fall
  // back to the first *user* terminal (never auto-select an agent terminal —
  // those only surface here when explicitly opened from the chat dock).
  const activeId =
    sessions.find((s) => s.id === activeSessionId)?.id ??
    sessions.find((s) => !s.is_agent)?.id ??
    null;
  // The sidebar lists user terminals only. Agent terminals are tracked
  // separately in the chat dock; they appear here solely as the active pane
  // when the user explicitly opens one — so include the active session even
  // if it is an agent terminal. Ordered by the user's drag-drop ordering.
  const listed = useMemo(
    () =>
      orderedSessions(
        sessions.filter((s) => !s.is_agent || s.id === activeId),
        order
      ),
    [sessions, activeId, order]
  );

  const chatTitleById = useMemo(() => {
    const m = new Map();
    for (const list of [flatTasks || [], ...Object.values(tasksByProject || {})]) {
      for (const t of list || []) if (t?.id && !m.has(t.id)) m.set(t.id, t.title);
    }
    return m;
  }, [flatTasks, tasksByProject]);

  // Live agent terminals only, grouped by owning project, newest first.
  const agentGroups = useMemo(() => {
    const live = sessions
      .filter((s) => s.is_agent && !s.exited)
      .sort((a, b) => (b.created_at_ms || 0) - (a.created_at_ms || 0));
    const groups = new Map();
    for (const s of live) {
      const proj = projectForCwd(projects, s.cwd);
      const key = proj?.id ?? `cwd:${normPath(s.cwd)}`;
      const name = proj?.name ?? (String(s.cwd || '').split(/[\\/]/).filter(Boolean).pop() || 'Other');
      if (!groups.has(key)) groups.set(key, { key, name, project: proj, items: [] });
      groups.get(key).items.push(s);
    }
    return [...groups.values()];
  }, [sessions, projects]);
  const agentListed = useMemo(() => agentGroups.flatMap((g) => g.items), [agentGroups]);
  const agentActiveId =
    agentListed.find((s) => s.id === agentPick)?.id ??
    agentListed.find((s) => s.id === activeSessionId)?.id ??
    agentListed[0]?.id ??
    null;

  const openChat = (session) => {
    const taskId = session.task_id;
    if (!taskId) return;
    const agent = useAgent.getState();
    const ownerProjectId = Object.entries(agent.tasksByProject || {}).find(([, list]) =>
      (list || []).some((t) => t.id === taskId),
    )?.[0];
    const proj =
      projects.find((p) => p.id === ownerProjectId) ?? projectForCwd(projects, session.cwd);
    if (proj && proj.id !== agent.activeProject?.id) {
      agent.setActiveProject({ id: proj.id, name: proj.name, root: proj.root_path });
    }
    useAgent.getState().setActiveTask(taskId);
    useLayout.getState().openChatDock();
  };

  // Require a small drag distance before a pointer-down on the grip becomes a
  // drag, so a plain click on the handle doesn't get swallowed.
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } })
  );

  const onDragEnd = (event) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const ids = listed.map((s) => s.id);
    const oldIndex = ids.indexOf(active.id);
    const newIndex = ids.indexOf(over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    reorderTerminals(arrayMove(ids, oldIndex, newIndex));
  };

  const empty = listed.length === 0;
  const isAgentView = terminalView === 'agent';
  const shownId = isAgentView ? agentActiveId : activeId;
  const mounted = useMemo(() => {
    const seen = new Set();
    return [...listed, ...agentListed].filter((s) => (seen.has(s.id) ? false : seen.add(s.id)));
  }, [listed, agentListed]);

  // Content area: only the active pane is visible; the rest stay mounted but
  // hidden so their PTYs keep streaming and their scrollback survives.
  const renderContent = () => {
    if (isAgentView && agentListed.length === 0) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-1 text-xs text-muted-foreground">
          <p>No live agent terminals.</p>
          <p className="text-[11px] text-muted-foreground/70">
            Terminals an agent opens with run_command show up here while they're alive.
          </p>
        </div>
      );
    }
    if (!isAgentView && empty) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-2 text-xs text-muted-foreground">
          <Button variant="outline" size="sm" onClick={openTerminalPicker}>
            <Plus className="size-3 mr-1" />
            New Terminal
          </Button>
          <p className="text-[11px] text-muted-foreground/70">
            Opens a shell at a project root — you pick the project next.
          </p>
        </div>
      );
    }

    return (
      <div className="relative h-full w-full overflow-hidden">
        {mounted.map((s) => (
          <div
            key={s.id}
            className={cn('absolute inset-0', shownId === s.id ? 'block' : 'hidden')}
          >
            <TerminalPane sessionId={s.id} active={shownId === s.id} />
          </div>
        ))}
      </div>
    );
  };

  return (
    <ResizablePanelGroup
      direction="horizontal"
      className="h-full w-full bg-background"
    >
      {/* Terminal content area (left) */}
      <ResizablePanel id="terminal-content" defaultSize="88%" minSize="40%">
        <div className="h-full w-full overflow-hidden">{renderContent()}</div>
      </ResizablePanel>

      <ResizableHandle />

      {/* Right-side vertical terminal tabs */}
      <ResizablePanel id="terminal-sidebar" defaultSize="12%" minSize="6%" maxSize="30%">
        <div className="flex h-full flex-col border-l border-border/60">
          {/* Draggable tab list. New-terminal "+" lives in the panel top bar
              (BottomPanelHost), so there's no redundant header button here. */}
          <div className="flex-1 overflow-y-auto">
            {isAgentView ? (
              agentGroups.map((g) => (
                <div key={g.key}>
                  <div
                    className="sticky top-0 z-[1] flex items-center gap-1 border-b border-border/60 bg-background/95 px-2 py-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground"
                    title={g.project?.root_path || g.name}
                  >
                    <Folder className="size-3 shrink-0" />
                    <span className="truncate">{g.name}</span>
                    <span className="ml-auto tabular-nums">{g.items.length}</span>
                  </div>
                  {g.items.map((s) => (
                    <AgentTerminalRow
                      key={s.id}
                      session={s}
                      chatTitle={s.task_id ? chatTitleById.get(s.task_id) || 'Chat' : null}
                      active={agentActiveId === s.id}
                      onSelect={setAgentPick}
                      onOpenChat={openChat}
                      onClose={onCloseTab}
                    />
                  ))}
                </div>
              ))
            ) : (
            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              modifiers={[restrictToVerticalAxis, restrictToParentElement]}
              onDragEnd={onDragEnd}
            >
              <SortableContext
                items={listed.map((s) => s.id)}
                strategy={verticalListSortingStrategy}
              >
                {listed.map((s) => (
                  <SortableTab
                    key={s.id}
                    session={s}
                    active={activeId === s.id}
                    onSelect={setActiveSessionId}
                    onClose={onCloseTab}
                  />
                ))}
              </SortableContext>
            </DndContext>
            )}
          </div>
        </div>
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
