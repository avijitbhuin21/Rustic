import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Server, Scroll, BookOpen, Workflow } from 'lucide-react';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetBody,
} from '@/components/ui/sheet';
import { Slider } from '@/components/settings/cloud-machines';
import { McpPanel } from './mcp-panel';
import { RulesPanel } from './rules-panel';
import { SkillsPanel } from './skills-panel';
import { WorkflowsPanel } from './workflows-panel';

const WIDTH_KEY = 'rustic.agentToolsWidth';
const DEFAULT_WIDTH = 720;
const MIN_WIDTH = 380;

/** Clamp a sheet width to the usable window range. */
function clampWidth(w) {
  const max = Math.max(MIN_WIDTH, Math.floor(window.innerWidth * 0.95));
  return Math.min(max, Math.max(MIN_WIDTH, Math.round(w)));
}

const TABS = [
  { id: 'mcp', label: 'MCP', icon: Server },
  { id: 'rules', label: 'Rules', icon: Scroll },
  { id: 'skills', label: 'Skills', icon: BookOpen },
  { id: 'workflows', label: 'Workflows', icon: Workflow },
];

// Hosts the previously-separate tabs (MCP / Rules / Skills / Workflows) inside
// a slide-over so the chat stays visible underneath. Driven from the kebab in
// ChatView's header. The `initialTab` prop lets the kebab menu jump straight
// to a specific section.
export function AgentToolsSheet({ open, onOpenChange, initialTab = 'mcp' }) {
  const [tab, setTab] = useState(initialTab);
  const [width, setWidth] = useState(() => {
    const saved = Number(localStorage.getItem(WIDTH_KEY));
    return clampWidth(saved > 0 ? saved : DEFAULT_WIDTH);
  });
  const dragging = useRef(false);

  // Jump to the requested section each time the sheet opens.
  useEffect(() => { if (open) setTab(initialTab); }, [open, initialTab]);

  useEffect(() => {
    const onResize = () => setWidth((w) => clampWidth(w));
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  const startResize = useCallback((e) => {
    e.preventDefault();
    dragging.current = true;
    const move = (ev) => { if (dragging.current) setWidth(clampWidth(window.innerWidth - ev.clientX)); };
    const up = () => {
      dragging.current = false;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      setWidth((w) => { localStorage.setItem(WIDTH_KEY, String(w)); return w; });
    };
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }, []);

  const resetWidth = () => { setWidth(clampWidth(DEFAULT_WIDTH)); localStorage.setItem(WIDTH_KEY, String(DEFAULT_WIDTH)); };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="max-w-[95vw]" style={{ width }}>
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize agent tools"
          title="Drag to resize · double-click to reset"
          onPointerDown={startResize}
          onDoubleClick={resetWidth}
          className="absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize after:absolute after:inset-y-0 after:left-1 after:w-px after:bg-transparent after:transition-colors hover:after:bg-primary/60"
        />
        <SheetHeader>
          <SheetTitle>Agent tools</SheetTitle>
        </SheetHeader>
        <div className="shrink-0 px-3 pb-1 pt-2">
          <Slider value={tab} onChange={setTab} options={TABS} />
        </div>
        <SheetBody className="flex flex-col overflow-hidden">
          {tab === 'mcp' && <McpPanel />}
          {tab === 'rules' && <RulesPanel />}
          {tab === 'skills' && <SkillsPanel />}
          {tab === 'workflows' && <WorkflowsPanel />}
        </SheetBody>
      </SheetContent>
    </Sheet>
  );
}

export default AgentToolsSheet;
