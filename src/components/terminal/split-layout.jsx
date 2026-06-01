import React, { useMemo } from 'react';
import { X, SplitSquareHorizontal, SplitSquareVertical, Plus } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTerminal, terminalTabLabel } from '@/state/terminal';
import { collectSessionIds } from '@/lib/split-tree';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';
import { TerminalPane } from './terminal-pane';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
} from '@/components/ui/dropdown-menu';

const openTerminalPicker = () =>
  window.dispatchEvent(new Event(TERMINAL_PICKER_EVENT));

/** Submenu listing terminals that can be placed into a new split. */
function SplitSubmenu({ label, icon: Icon, candidates, onPick }) {
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Icon className="size-3.5" />
        {label}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        {candidates.length === 0 ? (
          <DropdownMenuItem
            onClick={(e) => {
              e.stopPropagation();
              openTerminalPicker();
            }}
          >
            <Plus className="size-3.5" />
            New terminal…
          </DropdownMenuItem>
        ) : (
          candidates.map((s) => (
            <DropdownMenuItem
              key={s.id}
              onClick={(e) => {
                e.stopPropagation();
                onPick(s.id);
              }}
            >
              <span className="truncate">{terminalTabLabel(s)}</span>
            </DropdownMenuItem>
          ))
        )}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
}

/** A single leaf pane: header (label + split menu + close) over the terminal. */
function SplitPane({ node, active, candidates, onSplit, onClose, onSelect }) {
  const { sessionId } = node;
  const session = useTerminal((s) => s.sessions.find((x) => x.id === sessionId));
  const label = terminalTabLabel(session) || `pty ${sessionId}`;

  return (
    <div
      className={cn(
        'flex h-full w-full flex-col overflow-hidden rounded border bg-background',
        active ? 'border-primary/60' : 'border-border/60'
      )}
      onMouseDown={() => onSelect(sessionId)}
    >
      <div className="flex items-center justify-between border-b border-border/60 px-2 py-1 text-xs">
        <span className="truncate text-muted-foreground">{label}</span>
        <div className="flex items-center gap-0.5">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                onClick={(e) => e.stopPropagation()}
                className="rounded p-px text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                title="Split this pane"
                aria-label="Split this pane"
              >
                <SplitSquareHorizontal className="size-3" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-44">
              <DropdownMenuLabel>Split with…</DropdownMenuLabel>
              <SplitSubmenu
                label="Split right"
                icon={SplitSquareHorizontal}
                candidates={candidates}
                onPick={(id) => onSplit(sessionId, id, 'right')}
              />
              <SplitSubmenu
                label="Split down"
                icon={SplitSquareVertical}
                candidates={candidates}
                onPick={(id) => onSplit(sessionId, id, 'bottom')}
              />
              <DropdownMenuSeparator />
              <DropdownMenuItem
                variant="destructive"
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(sessionId);
                }}
              >
                Remove from split
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onClose(sessionId);
            }}
            className="rounded p-px text-muted-foreground hover:bg-destructive/20 hover:text-destructive"
            title="Remove pane from split"
            aria-label="Remove pane from split"
          >
            <X className="size-3" />
          </button>
        </div>
      </div>
      <div className="min-h-0 flex-1">
        <TerminalPane sessionId={sessionId} active />
      </div>
    </div>
  );
}

/** Recursively render a split-tree node. */
function SplitNode({ node, activeId, candidates, onSplit, onClose, onSelect, onResize }) {
  if (node.type === 'leaf') {
    return (
      <SplitPane
        node={node}
        active={activeId === node.sessionId}
        candidates={candidates}
        onSplit={onSplit}
        onClose={onClose}
        onSelect={onSelect}
      />
    );
  }

  // 'row' → side-by-side (vertical dividers); 'column' → stacked.
  const orientation = node.direction === 'row' ? 'horizontal' : 'vertical';
  // Remount the group when the set of children changes so RRP re-reads the
  // stored sizes as defaults instead of clinging to a stale internal layout.
  const groupKey = node.children.map((c) => c.id).join('|');

  return (
    <ResizablePanelGroup
      key={groupKey}
      orientation={orientation}
      className="h-full w-full"
      onLayout={(sizes) => {
        // Normalize to percentages summing to 100 so re-renders are unit-stable
        // regardless of what RRP reports back.
        const total = sizes.reduce((a, b) => a + b, 0) || 1;
        onResize(
          node.id,
          sizes.map((s) => Math.round((s / total) * 100 * 1000) / 1000)
        );
      }}
    >
      {node.children.map((child, i) => (
        <React.Fragment key={child.id}>
          {i > 0 && <ResizableHandle />}
          <ResizablePanel
            id={child.id}
            order={i}
            defaultSize={`${node.sizes?.[i] ?? 100 / node.children.length}%`}
            minSize="10%"
          >
            <SplitNode
              node={child}
              activeId={activeId}
              candidates={candidates}
              onSplit={onSplit}
              onClose={onClose}
              onSelect={onSelect}
              onResize={onResize}
            />
          </ResizablePanel>
        </React.Fragment>
      ))}
    </ResizablePanelGroup>
  );
}

/**
 * Top-level split layout. Reads the split tree from the store and renders it.
 * `candidates` are user terminals not already shown in the tree — the pool a
 * pane can be split with.
 */
export function SplitLayout({ sessions, activeId }) {
  const splitTree = useTerminal((s) => s.splitTree);
  const splitPane = useTerminal((s) => s.splitPane);
  const removeSplitPane = useTerminal((s) => s.removeSplitPane);
  const resizeSplit = useTerminal((s) => s.resizeSplit);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);

  const inTree = useMemo(
    () => new Set(splitTree ? collectSessionIds(splitTree) : []),
    [splitTree]
  );
  const candidates = useMemo(
    () => sessions.filter((s) => !s.is_agent && !inTree.has(s.id)),
    [sessions, inTree]
  );

  if (!splitTree) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
        No terminal to split. Open one first.
      </div>
    );
  }

  return (
    <div className="h-full w-full p-1">
      <SplitNode
        node={splitTree}
        activeId={activeId}
        candidates={candidates}
        onSplit={splitPane}
        onClose={removeSplitPane}
        onSelect={setActiveSessionId}
        onResize={resizeSplit}
      />
    </div>
  );
}
