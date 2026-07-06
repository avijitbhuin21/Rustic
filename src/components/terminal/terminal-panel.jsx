import React, { useEffect, useMemo, useState } from 'react';
import { Plus, X, GripVertical } from 'lucide-react';
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
import { TerminalPane } from './terminal-pane';
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
        className="mr-0.5 cursor-grab touch-none text-muted-foreground/50 opacity-0 hover:text-foreground focus-visible:opacity-100 active:cursor-grabbing group-hover:opacity-100"
        title="Drag to reorder"
        aria-label="Drag to reorder terminal"
      >
        <GripVertical className="size-3" />
      </button>
      {/* Running/exited status dot (H1c): live shells get a green dot, retired
          ones a muted dot — the backend now keeps exited sessions listed. */}
      <span
        aria-hidden
        title={session.exited ? 'Shell exited' : 'Running'}
        className={cn(
          'mr-1 size-1.5 shrink-0 rounded-full',
          session.exited ? 'bg-muted-foreground/40' : 'bg-emerald-500',
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
        className="ml-1 rounded p-px text-muted-foreground opacity-0 hover:bg-destructive/20 hover:text-destructive focus-visible:opacity-100 group-hover:opacity-100"
        title="Terminate terminal"
        aria-label="Terminate terminal"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}

export function TerminalPanel() {
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

  // Content area: only the active pane is visible; the rest stay mounted but
  // hidden so their PTYs keep streaming and their scrollback survives.
  const renderContent = () => {
    if (empty) {
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
        {listed.map((s) => (
          <div
            key={s.id}
            className={cn('absolute inset-0', activeId === s.id ? 'block' : 'hidden')}
          >
            <TerminalPane sessionId={s.id} active={activeId === s.id} />
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
          </div>
        </div>
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
