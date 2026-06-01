import React, { useEffect, useMemo, useRef, useState } from 'react';
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

// Stacked-column layout. Each terminal fills the full width of the content area
// and stacks vertically; the column scrolls with a native scrollbar (no pan or
// grab-drag). Tiles default to DEFAULT_TILE_HEIGHT and are resizable by dragging
// the handle at their bottom edge. A lone terminal ignores the fixed height and
// fills the available space instead.
const DEFAULT_TILE_HEIGHT = 520;
const MIN_TILE_HEIGHT = 160;

/** A draggable, selectable terminal tab in the sidebar list. */
function SortableTab({ session, active, onSelect, onClose }) {
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
        className="mr-0.5 cursor-grab touch-none text-muted-foreground/50 opacity-0 hover:text-foreground active:cursor-grabbing group-hover:opacity-100"
        title="Drag to reorder"
        aria-label="Drag to reorder terminal"
      >
        <GripVertical className="size-3" />
      </button>
      <span className="truncate flex-1">{terminalTabLabel(session)}</span>
      <button
        onClick={(e) => {
          e.stopPropagation();
          onClose(session.id);
        }}
        className="ml-1 rounded p-px text-muted-foreground opacity-0 hover:bg-destructive/20 hover:text-destructive group-hover:opacity-100"
        title="Terminate terminal"
        aria-label="Terminate terminal"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}

/** A single terminal rendered as a tile (grid / row layouts). */
function TerminalTile({ session, active, onSelect, onClose, className }) {
  return (
    <div
      className={cn(
        'flex w-full flex-col overflow-hidden rounded border bg-background',
        active ? 'border-primary/60' : 'border-border/60',
        className
      )}
      onMouseDown={() => onSelect(session.id)}
    >
      <div className="flex items-center justify-between border-b border-border/60 px-2 py-1 text-xs">
        <span className="truncate text-muted-foreground">
          {terminalTabLabel(session)}
        </span>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onClose(session.id);
          }}
          className="rounded p-px text-muted-foreground hover:bg-destructive/20 hover:text-destructive"
          title="Terminate terminal"
          aria-label="Terminate terminal"
        >
          <X className="size-3" />
        </button>
      </div>
      <div className="min-h-0 flex-1">
        {/* All tiles are visible, so each is `active` (drives the fit/resize). */}
        <TerminalPane sessionId={session.id} active />
      </div>
    </div>
  );
}

/**
 * Stacked column: every listed terminal rendered full-width, one above the
 * next, in a single vertically-scrolling column. The column scrolls with the
 * native scrollbar (no pan / grab-drag). Each tile has a resizable height —
 * drag the handle at its bottom edge — while a lone terminal ignores the fixed
 * default height and fills the whole area instead.
 */
function TerminalColumn({ tiles, activeId, onSelect, onClose }) {
  // Per-session tile heights (px). Ephemeral like terminal order/splits: session
  // ids are reassigned every launch, so there's nothing stable to persist.
  const [heights, setHeights] = useState({});
  const [resizing, setResizing] = useState(false);
  const drag = useRef(null);
  // A lone terminal fills the available space; only stack siblings get a fixed,
  // resizable height.
  const single = tiles.length === 1;

  const startResize = (sessionId, e) => {
    if (e.button !== 0) return; // primary button only
    e.preventDefault();
    e.stopPropagation();
    drag.current = {
      sessionId,
      startY: e.clientY,
      startH: heights[sessionId] ?? DEFAULT_TILE_HEIGHT,
    };
    setResizing(true);
    const onMove = (ev) => {
      const d = drag.current;
      if (!d) return;
      const next = Math.max(MIN_TILE_HEIGHT, d.startH + (ev.clientY - d.startY));
      setHeights((h) => ({ ...h, [d.sessionId]: next }));
    };
    const onUp = () => {
      drag.current = null;
      setResizing(false);
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  };

  return (
    <div
      className={cn(
        'h-full w-full overflow-y-auto overflow-x-hidden',
        // While dragging a handle, suppress text selection across the column.
        resizing && 'select-none'
      )}
    >
      <div className={cn('flex flex-col gap-2 p-2', single && 'h-full')}>
        {tiles.map((s) => (
          <div
            key={s.id}
            className={cn('relative shrink-0', single && 'min-h-0 flex-1')}
            style={single ? undefined : { height: heights[s.id] ?? DEFAULT_TILE_HEIGHT }}
          >
            <TerminalTile
              session={s}
              active={activeId === s.id}
              onSelect={onSelect}
              onClose={onClose}
              className="h-full"
            />
            {/* Resize handle — drag the bottom edge to set this tile's height.
                Sits in the gap below the tile so it never covers terminal text;
                highlights on hover / while dragging. Hidden for a lone terminal
                (it fills the area, so there's nothing to resize against). */}
            {!single && (
              <div
                onPointerDown={(e) => startResize(s.id, e)}
                className="absolute inset-x-0 -bottom-1 z-10 h-2 cursor-ns-resize bg-transparent transition-colors hover:bg-primary/50"
                title="Drag to resize"
                aria-label="Resize terminal height"
              />
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export function TerminalPanel() {
  const allSessions = useTerminal((s) => s.sessions);
  const hiddenSessionIds = useTerminal((s) => s.hiddenSessionIds);
  const activeSessionId = useTerminal((s) => s.activeSessionId);
  const order = useTerminal((s) => s.order);
  const layoutMode = useTerminal((s) => s.layoutMode);
  const reorderTerminals = useTerminal((s) => s.reorderTerminals);
  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);
  const closeTerminal = useTerminal((s) => s.closeTerminal);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);

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

  // Content area renders per layout mode. In 'tabs' only the active pane is
  // visible (the rest stay mounted but hidden so their PTYs keep streaming); in
  // 'grid' every listed terminal is stacked full-width in a scrollable column.
  const renderContent = () => {
    if (empty) {
      return (
        <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
          <Button variant="outline" size="sm" onClick={openTerminalPicker}>
            <Plus className="size-3 mr-1" />
            New Terminal
          </Button>
        </div>
      );
    }

    if (layoutMode === 'grid') {
      return (
        <TerminalColumn
          tiles={listed}
          activeId={activeId}
          onSelect={setActiveSessionId}
          onClose={closeTerminal}
        />
      );
    }

    // 'tabs' — single visible pane, the rest stay mounted but hidden.
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
                    onClose={closeTerminal}
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
