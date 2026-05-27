import React, { Suspense, useEffect, useState } from 'react';
import { TabBar } from '@/components/editor/tab-bar';
import { Breadcrumb } from '@/components/editor/breadcrumb';
import { useEditor } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { TerminalPane } from '@/components/terminal/terminal-pane';
import { Logo } from '@/components/logo';
import { cn } from '@/lib/utils';

const _monacoImport = import('@/components/editor/monaco-editor');
const MonacoEditor   = React.lazy(() => _monacoImport);

// Cross-pane tab drag-drop: module-level capture-phase listeners.
//
// WHY module-level (not useEffect): avoids React lifecycle timing issues with
// conditional renders — the listeners are always present from first module load.
//
// WHY capture phase + stopImmediatePropagation:
//   Monaco registers its own capture dragstart/dragover/drop listeners that call
//   e.preventDefault() on non-Monaco drags, silently cancelling them (spec: a
//   cancelled dragstart fires no dragend). Since our IIFE registers before Monaco
//   mounts, our capture listeners fire first, letting us stopImmediatePropagation
//   before Monaco can interfere.
//
// WHY window.__rusticTabDrag instead of dataTransfer.types:
//   WebView2 strips custom MIME types from dataTransfer.types during drag events,
//   so types.includes('application/x-rustic-tab') returns false. We use a plain
//   global variable instead, which is reliable across all events.
(function registerTabDragListeners() {
  if (window.__rusticDnDCleanup) window.__rusticDnDCleanup();

  const paneOf = (t) => t?.closest?.('[data-rustic-pane-id]');
  const tabOf  = (t) => t?.closest?.('[data-tab-id]');

  const onDragStart = (e) => {
    const tab  = tabOf(e.target);
    const pane = paneOf(e.target);
    if (!tab || !pane) return;
    const drag = { tabId: tab.dataset.tabId, fromGroupId: pane.dataset.rusticPaneId };
    window.__rusticTabDrag = drag;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('application/x-rustic-tab', JSON.stringify(drag));
    e.stopImmediatePropagation();
  };

  const onDragEnd = () => { window.__rusticTabDrag = null; };

  const onDragEnter = (e) => {
    const drag = window.__rusticTabDrag;
    if (!drag) return;
    const pane = paneOf(e.target);
    if (!pane || pane.dataset.rusticPaneId === drag.fromGroupId) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  };

  const onDragOver = (e) => {
    const drag = window.__rusticTabDrag;
    if (!drag) return;
    const pane = paneOf(e.target);
    if (!pane || pane.dataset.rusticPaneId === drag.fromGroupId) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  };

  const onDrop = (e) => {
    const drag = window.__rusticTabDrag;
    if (!drag) return;
    const pane = paneOf(e.target);
    if (!pane || pane.dataset.rusticPaneId === drag.fromGroupId) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    useEditor.getState().moveTabToGroup(drag.tabId, drag.fromGroupId, pane.dataset.rusticPaneId, null);
    useEditor.getState().setActiveGroup(pane.dataset.rusticPaneId);
  };

  document.addEventListener('dragstart', onDragStart, true);
  document.addEventListener('dragend',   onDragEnd,   true);
  document.addEventListener('dragenter', onDragEnter, true);
  document.addEventListener('dragover',  onDragOver,  true);
  document.addEventListener('drop',      onDrop,      true);

  window.__rusticDnDCleanup = () => {
    document.removeEventListener('dragstart', onDragStart, true);
    document.removeEventListener('dragend',   onDragEnd,   true);
    document.removeEventListener('dragenter', onDragEnter, true);
    document.removeEventListener('dragover',  onDragOver,  true);
    document.removeEventListener('drop',      onDrop,      true);
  };
}());
const MarkdownPreview = React.lazy(() => import('@/components/editor/previews/markdown-preview'));
const ImagePreview    = React.lazy(() => import('@/components/editor/previews/image-preview'));
const PdfPreview      = React.lazy(() => import('@/components/editor/previews/pdf-preview'));
const SvgPreview      = React.lazy(() => import('@/components/editor/previews/svg-preview'));
const HtmlPreview     = React.lazy(() => import('@/components/editor/previews/html-preview'));
const VideoPreview    = React.lazy(() => import('@/components/editor/previews/video-preview'));
const DocxPreview     = React.lazy(() => import('@/components/editor/previews/docx-preview'));
const XlsxPreview     = React.lazy(() => import('@/components/editor/previews/xlsx-preview'));
const HexPreview      = React.lazy(() => import('@/components/editor/previews/hex-preview'));
const DiffView        = React.lazy(() => import('@/components/scm/diff-view'));

function PaneFallback() {
  return (
    <div className="flex h-full w-full flex-col gap-2 p-6">
      <Skeleton className="h-5 w-1/3" />
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-4 w-1/2" />
    </div>
  );
}

function EmptyState() {
  // Watermark-only empty state — VS Code style. The logo is pure black or
  // pure white depending on theme, so we keep opacity low (~8%) to read as
  // ambient background rather than a stark mark.
  return (
    <div className="pointer-events-none flex h-full w-full flex-1 items-center justify-center">
      <Logo className="size-72 opacity-30" />
    </div>
  );
}

function ActiveView({ tab }) {
  switch (tab.kind) {
    case 'markdown': return <MarkdownPreview tab={tab} />;
    case 'image':    return <ImagePreview tab={tab} />;
    case 'pdf':      return <PdfPreview tab={tab} />;
    case 'svg':      return <SvgPreview tab={tab} />;
    case 'html':     return <HtmlPreview tab={tab} />;
    case 'video':    return <VideoPreview tab={tab} />;
    case 'docx':     return <DocxPreview tab={tab} />;
    case 'xlsx':     return <XlsxPreview tab={tab} />;
    case 'hex':      return <HexPreview tab={tab} />;
    case 'diff':     return <DiffView file={tab.diff} />;
    default:         return <MonacoEditor tab={tab} />;
  }
}

export default function EditorPane({ groupId }) {
  const group           = useEditor((s) => (s.groups ?? []).find(g => g.id === groupId));
  const activeGroupId   = useEditor((s) => s.activeGroupId);
  const setActiveGroup  = useEditor((s) => s.setActiveGroup);
  const openFileInGroup = useEditor((s) => s.openFileInGroup);

  const [isFileDragOver, setIsFileDragOver] = useState(false);

  // Ctrl+W: ref-counted so only one global listener exists across all panes.
  useEffect(() => {
    if (typeof window.__rusticCtrlWCount === 'undefined') window.__rusticCtrlWCount = 0;
    window.__rusticCtrlWCount += 1;
    if (window.__rusticCtrlWCount === 1) {
      window.__rusticCtrlWHandler = (e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'w') {
          const { groups, activeGroupId, closeTabInGroup } = useEditor.getState();
          const ag = (groups ?? []).find(g => g.id === activeGroupId);
          if (ag?.activeId) { e.preventDefault(); closeTabInGroup(ag.activeId, ag.id); }
        }
      };
      window.addEventListener('keydown', window.__rusticCtrlWHandler);
    }
    return () => {
      window.__rusticCtrlWCount -= 1;
      if (window.__rusticCtrlWCount === 0) {
        window.removeEventListener('keydown', window.__rusticCtrlWHandler);
        delete window.__rusticCtrlWHandler;
      }
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!group) return null;

  const { tabs, activeId } = group;
  const active = tabs.find(t => t.id === activeId) ?? null;
  const isThisPaneFocused = activeGroupId === groupId;

  // ── File drag (from explorer) ────────────────────────────────────────────
  const handleDragOver = (e) => {
    const types = [...(e.dataTransfer?.types ?? [])];
    if (types.includes('application/x-rustic-file') || types.includes('text/plain')) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
      setIsFileDragOver(true);
    }
  };
  const handleDrop = (e) => {
    e.preventDefault();
    setIsFileDragOver(false);
    const path = e.dataTransfer.getData('application/x-rustic-file') || e.dataTransfer.getData('text/plain');
    if (path?.trim()) {
      openFileInGroup(path.trim(), groupId);
      setActiveGroup(groupId);
    }
  };
  const handlePaneDragLeave = (e) => {
    if (!e.currentTarget.contains(e.relatedTarget)) setIsFileDragOver(false);
  };

  const terminalTabs     = tabs.filter(t => t.kind === 'terminal');
  const isTerminalActive = active?.kind === 'terminal';

  return (
    <div
      data-rustic-pane-id={groupId}
      className={cn(
        'flex h-full w-full flex-col bg-background',
        isThisPaneFocused && 'ring-1 ring-inset ring-primary/15'
      )}
      onMouseDown={() => setActiveGroup(groupId)}
      onDragOver={handleDragOver}
      onDragLeave={handlePaneDragLeave}
      onDrop={handleDrop}
    >
      <TabBar groupId={groupId} />
      {active?.path && active.kind !== 'diff' && active.kind !== 'terminal' && (
        <Breadcrumb tab={active} />
      )}

      <div className="relative flex-1 overflow-hidden">
        {/* File drag-over indicator */}
        {isFileDragOver && (
          <div className="pointer-events-none absolute inset-0 z-30 flex items-center justify-center rounded border-2 border-dashed border-primary/50 bg-primary/5">
            <span className="rounded-md bg-background/80 px-3 py-1.5 text-xs font-medium text-primary backdrop-blur-sm">
              Drop to open here
            </span>
          </div>
        )}

        {/* Terminal panes — always mounted to preserve PTY session state */}
        {terminalTabs.map(t => (
          <div key={t.id} className={cn('absolute inset-0', t.id === activeId ? 'z-10 block' : 'hidden')}>
            <TerminalPane sessionId={t.terminalSessionId} active={t.id === activeId} />
          </div>
        ))}

        {/* Non-terminal content */}
        {!isTerminalActive && (
          active ? (
            <div className="absolute inset-0">
              <Suspense fallback={<PaneFallback />}>
                <ActiveView key={active.id} tab={active} />
              </Suspense>
            </div>
          ) : (
            <EmptyState />
          )
        )}
      </div>
    </div>
  );
}
