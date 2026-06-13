import React, { useRef, useState } from 'react';
import { X, Circle, SplitSquareHorizontal, PanelLeftClose, PanelRightOpen } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useEditor } from '@/state/editor';
import { useExplorer, revealInFileManager } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { IS_WEB } from '@/lib/platform';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Button } from '@/components/ui/button';
import {
  ContextMenu, ContextMenuTrigger, ContextMenuContent,
  ContextMenuItem, ContextMenuSeparator,
} from '@/components/ui/context-menu';
import { toast } from 'sonner';

function trimPath(raw) {
  if (!raw || raw.length <= 55) return raw;
  const sep   = raw.includes('\\') ? '\\' : '/';
  const parts = raw.split(sep).filter(Boolean);
  if (parts.length <= 4) {
    const k = 26;
    return raw.slice(0, k) + '…' + raw.slice(raw.length - k);
  }
  const lead   = raw.match(/^[A-Za-z]:/) ? raw.slice(0, 2) : '';
  const prefix = parts.slice(0, 2).join(sep);
  const suffix = parts.slice(-2).join(sep);
  const full   = lead && !prefix.startsWith(lead) ? lead + sep + prefix : prefix;
  return `${full}${sep}…${sep}${suffix}`;
}

function Tab({
  tab, active, onActivate, onClose,
  onDragStart, onDragEnd, onDragOver, onDrop, dragOver,
  onCloseOthers, onCloseAll, onCopyPath, onCopyRelativePath, onReveal,
}) {
  // The outer div carries `draggable` and `data-tab-id` so the module-level
  // IIFE in editor-pane.jsx can identify it via closest('[data-tab-id]').
  // Radix's ContextMenuTrigger/TooltipTrigger use an inner display:contents div
  // so their prop-merging never touches the draggable element.
  return (
    <ContextMenu>
      <div
        draggable
        data-tab-id={tab.id}
        onDragStart={(e) => onDragStart(tab.id, e)}
        onDragEnd={onDragEnd}
        className={cn(
          'group/tab relative flex h-8 shrink-0 cursor-pointer items-center gap-1.5 border-r border-border px-3 text-xs select-none',
          active
            ? 'bg-background text-foreground'
            : 'bg-muted/40 text-muted-foreground hover:bg-muted/60 hover:text-foreground',
          dragOver && 'ring-1 ring-inset ring-primary'
        )}
      >
        {active && <span className="absolute left-0 right-0 top-0 h-px bg-primary" />}
        <ContextMenuTrigger asChild>
          <div
            className="contents"
            onClick={() => onActivate(tab.id)}
            onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); onClose(tab.id); } }}
            onDragOver={(e) => onDragOver(tab.id, e)}
            onDrop={(e) => onDrop(tab.id, e)}
          >
            <span className="max-w-[200px] truncate">{tab.title}</span>
            <span
              role="button"
              tabIndex={-1}
              onClick={(e) => { e.stopPropagation(); onClose(tab.id); }}
              className={cn(
                'flex size-4 items-center justify-center rounded-sm hover:bg-muted',
                !tab.dirty && 'opacity-0 group-hover/tab:opacity-100',
                active && 'opacity-100'
              )}
            >
              {tab.dirty ? (
                // Custom previews (xlsx, markdown, html, svg, docx) flip
                // tab.dirty via useEditor.setDirty when their internal
                // draft state diverges from disk. Monaco does the same on
                // model change. The yellow dot is the universal "unsaved"
                // signal so we don't duplicate it inside individual
                // preview chrome.
                <span className="size-2 rounded-full bg-yellow-400" />
              ) : (
                <X className="size-3" />
              )}
            </span>
          </div>
        </ContextMenuTrigger>
      </div>
      <ContextMenuContent className="w-52">
        <ContextMenuItem onSelect={() => onClose(tab.id)}>Close Tab</ContextMenuItem>
        <ContextMenuItem onSelect={() => onCloseOthers(tab.id)}>Close Others</ContextMenuItem>
        <ContextMenuItem onSelect={onCloseAll}>Close All</ContextMenuItem>
        {tab.path && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => onCopyPath(tab.path)}>Copy Path</ContextMenuItem>
            {onCopyRelativePath && (
              <ContextMenuItem onSelect={() => onCopyRelativePath(tab.path)}>Copy Relative Path</ContextMenuItem>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => onReveal(tab.path)}>Reveal in File Explorer</ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}

export function TabBar({ groupId }) {
  const group       = useEditor((s) => (s.groups ?? []).find(g => g.id === groupId));
  const groupCount  = useEditor((s) => (s.groups ?? []).length);
  const isRightmost = useEditor((s) => { const gs = s.groups ?? []; return gs[gs.length - 1]?.id === groupId; });
  // When the chat dock is open it sits to the right of the editor area and
  // takes over the top-right slot under the window controls. In that case the
  // editor's rightmost tab bar no longer needs the 138px offset — the chat
  // header is the one that has to clear the window-control strip.
  const chatDockOpen = useLayout((s) => s.chatDockOpen);
  const openChatDock = useLayout((s) => s.openChatDock);
  const needsWindowControlsOffset = isRightmost && !chatDockOpen && !IS_WEB;
  const splitGroup       = useEditor((s) => s.splitGroup);
  const closeGroup       = useEditor((s) => s.closeGroup);
  const setActiveInGroup   = useEditor((s) => s.setActiveInGroup);
  const closeTabInGroup    = useEditor((s) => s.closeTabInGroup);
  const closeOthersInGroup = useEditor((s) => s.closeOthersInGroup);
  const closeAllInGroup    = useEditor((s) => s.closeAllInGroup);
  const moveTabToGroup     = useEditor((s) => s.moveTabToGroup);
  const reorderTabsInGroup = useEditor((s) => s.reorderTabsInGroup);

  const projects       = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const projectRoot    = projects.find((p) => p.id === activeProjectId)?.root_path ?? null;

  const dragId    = useRef(null);
  const scrollRef = useRef(null);
  const [overId, setOverId]         = useState(null);
  const [barDragOver, setBarDragOver] = useState(false); // cross-pane drag hovering the strip

  if (!group) return null;
  const { tabs, activeId } = group;

  // onDragStart rarely runs in practice — the module-level IIFE in editor-pane.jsx
  // intercepts dragstart first (capture phase) and calls stopImmediatePropagation
  // to block Monaco. Same-group reordering still uses these React drop handlers.
  const onDragStart = (id, e) => {
    dragId.current = id;
  };

  const onDragEnd = () => {
    dragId.current = null;
  };

  const onDragOver = (id, e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    if (overId !== id) setOverId(id);
  };

  const readTabDrag = (e) =>
    e.dataTransfer.getData('application/x-rustic-tab') || JSON.stringify(window.__rusticTabDrag ?? null);

  // Drop onto a specific tab (reorder within group OR move from another group)
  const onDrop = (targetTabId, e) => {
    e.preventDefault();
    e.stopPropagation(); // prevent bubbling to onBarDrop and double-processing
    setOverId(null);
    setBarDragOver(false);
    const raw = readTabDrag(e);
    if (!raw || raw === 'null') return;
    const { tabId: fromTabId, fromGroupId } = JSON.parse(raw);
    if (fromGroupId === groupId) {
      if (fromTabId !== targetTabId) reorderTabsInGroup(fromTabId, targetTabId, groupId);
    } else {
      moveTabToGroup(fromTabId, fromGroupId, groupId, targetTabId);
    }
  };

  // Drop on the empty strip area (appends to this group)
  const onBarDragOver = (e) => {
    if (!e.dataTransfer.types.includes('application/x-rustic-tab')) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    setBarDragOver(true);
  };
  const onBarDragLeave = (e) => {
    if (!scrollRef.current?.contains(e.relatedTarget)) {
      setOverId(null);
      setBarDragOver(false);
    }
  };
  const onBarDrop = (e) => {
    e.preventDefault();
    e.stopPropagation(); // prevent bubbling to outer EditorPane drop handler
    setBarDragOver(false);
    setOverId(null);
    const raw = readTabDrag(e);
    if (!raw || raw === 'null') return;
    const { tabId: fromTabId, fromGroupId } = JSON.parse(raw);
    if (fromGroupId !== groupId) moveTabToGroup(fromTabId, fromGroupId, groupId, null);
  };

  const onWheel = (e) => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollLeft += Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
  };

  const handleCopyPath = (path) => {
    navigator.clipboard.writeText(path).catch(() => {});
    toast.success('Path copied');
  };

  const handleCopyRelativePath = projectRoot
    ? (path) => {
        const sep  = path.includes('\\') ? '\\' : '/';
        const root = projectRoot.endsWith(sep) ? projectRoot : projectRoot + sep;
        navigator.clipboard.writeText(path.startsWith(root) ? path.slice(root.length) : path).catch(() => {});
        toast.success('Relative path copied');
      }
    : null;

  const handleReveal = async (path) => {
    try { await revealInFileManager(path); } catch (err) { toast.error(`Could not reveal: ${err}`); }
  };

  return (
    <div className="flex h-8 shrink-0 items-stretch border-b border-border bg-muted/20">
      {/* Scrollable tab strip */}
      <div
        ref={scrollRef}
        data-tauri-drag-region={!IS_WEB || undefined}
        onWheel={onWheel}
        onDragOver={onBarDragOver}
        onDragLeave={onBarDragLeave}
        onDrop={onBarDrop}
        className={cn(
          'explorer-scroll flex flex-1 items-center overflow-x-auto overflow-y-hidden',
          barDragOver && 'bg-primary/10 ring-1 ring-inset ring-primary/40'
        )}
      >
        {tabs.length === 0 ? (
          <span data-tauri-drag-region={!IS_WEB || undefined} className="px-3 text-xs text-muted-foreground opacity-50">No files open — drop a tab here</span>
        ) : (
          tabs.map((t) => (
            <Tab
              key={t.id}
              tab={t}
              active={t.id === activeId}
              onActivate={(id) => setActiveInGroup(id, groupId)}
              onClose={(id) => closeTabInGroup(id, groupId)}
              onDragStart={onDragStart}
              onDragEnd={onDragEnd}
              onDragOver={onDragOver}
              onDrop={onDrop}
              dragOver={overId === t.id}
              onCloseOthers={(id) => closeOthersInGroup(id, groupId)}
              onCloseAll={() => closeAllInGroup(groupId)}
              onCopyPath={handleCopyPath}
              onCopyRelativePath={handleCopyRelativePath}
              onReveal={handleReveal}
            />
          ))
        )}
      </div>

      {/* Right-side pane actions. The rightmost editor group needs a 138 px
          offset to clear the fixed window-control strip — but only when it's
          actually the rightmost thing on screen. When the chat dock is open
          the chat header takes over that responsibility, so we drop the
          offset here. */}
      <div className="flex shrink-0 items-center gap-px px-1" style={{ paddingRight: needsWindowControlsOffset ? 138 : 4 }}>
        {/* Split this group */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              onClick={() => splitGroup(groupId)}
              className="size-6 text-muted-foreground hover:text-foreground"
            >
              <SplitSquareHorizontal className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Split Editor</TooltipContent>
        </Tooltip>

        {/* Open chat dock — only visible on the rightmost group when the dock is closed */}
        {isRightmost && !chatDockOpen && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={openChatDock}
                className="size-6 text-muted-foreground hover:text-foreground"
              >
                <PanelRightOpen className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Open Chat Dock</TooltipContent>
          </Tooltip>
        )}

        {/* Close this group (only when more than one exists) */}
        {groupCount > 1 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={() => closeGroup(groupId)}
                className="size-6 text-muted-foreground hover:text-foreground"
              >
                <PanelLeftClose className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Close Panel</TooltipContent>
          </Tooltip>
        )}
      </div>
    </div>
  );
}
