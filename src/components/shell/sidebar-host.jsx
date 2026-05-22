import React, { useState, useEffect } from 'react';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useEditor } from '@/state/editor';
import { Explorer } from '@/components/explorer/explorer';
import { SearchPanel } from '@/components/search/search-panel';
import ScmPanel from '@/components/scm/scm-panel';
import AgentPanel from '@/components/agent/agent-panel';
import { cn } from '@/lib/utils';

function openFileInEditor(path, opts) {
  try { useEditor.getState().openFile(path, opts); } catch {}
}

// Each panel is defined once; components are created lazily on first visit
// and kept alive (hidden) on subsequent visits so they never remount.
const PANEL_IDS = [
  SIDEBAR_PANELS.EXPLORER,
  SIDEBAR_PANELS.SEARCH,
  SIDEBAR_PANELS.SCM,
  SIDEBAR_PANELS.AGENT,
];

function panelComponent(id) {
  switch (id) {
    case SIDEBAR_PANELS.EXPLORER:  return <Explorer onOpenFile={openFileInEditor} />;
    case SIDEBAR_PANELS.SEARCH:    return <SearchPanel onOpenFile={openFileInEditor} />;
    case SIDEBAR_PANELS.SCM:       return <ScmPanel />;
    case SIDEBAR_PANELS.AGENT:     return <AgentPanel />;
    default: return null;
  }
}

export function SidebarHost() {
  const activePanel = useLayout((s) => s.activeSidebarPanel);
  // Track which panels have been visited — mount once, never unmount
  const [everMounted, setEverMounted] = useState(() => new Set([activePanel]));

  useEffect(() => {
    setEverMounted(prev => {
      if (prev.has(activePanel)) return prev;
      const next = new Set(prev);
      next.add(activePanel);
      return next;
    });
  }, [activePanel]);

  return (
    <div className="h-full w-full overflow-hidden bg-sidebar">
      {PANEL_IDS.map(id => {
        if (!everMounted.has(id)) return null;
        return (
          <div key={id} className={cn('h-full w-full', activePanel !== id && 'hidden')}>
            {panelComponent(id)}
          </div>
        );
      })}
    </div>
  );
}
