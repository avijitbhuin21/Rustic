import React, { useEffect, useMemo, useState } from 'react';
import { Files, Search, GitBranch, Terminal as TerminalIcon, Code2, Bot, MoreHorizontal, Settings } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useLayout, MOBILE_TABS } from '@/state/layout';
import { useEditor } from '@/state/editor';
import { Explorer } from '@/components/explorer/explorer';
import { SearchPanel } from '@/components/search/search-panel';
import ScmPanel from '@/components/scm/scm-panel';
import AgentPanel from '@/components/agent/agent-panel';
import { EditorAreaHost } from '@/components/shell/editor-area-host';
import { BottomPanelHost } from '@/components/shell/bottom-panel-host';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

const PRIMARY_TABS = [
  { id: MOBILE_TABS.AGENT, label: 'Agent', icon: Bot },
  { id: MOBILE_TABS.EXPLORER, label: 'Files', icon: Files },
  { id: MOBILE_TABS.EDITOR, label: 'Editor', icon: Code2 },
  { id: MOBILE_TABS.TERMINAL, label: 'Terminal', icon: TerminalIcon },
];

function renderView(id, onOpenFile) {
  switch (id) {
    case MOBILE_TABS.AGENT:    return <AgentPanel />;
    case MOBILE_TABS.EXPLORER: return <Explorer onOpenFile={onOpenFile} />;
    case MOBILE_TABS.EDITOR:   return <EditorAreaHost />;
    case MOBILE_TABS.TERMINAL: return <BottomPanelHost />;
    case MOBILE_TABS.SEARCH:   return <SearchPanel onOpenFile={onOpenFile} />;
    case MOBILE_TABS.SCM:      return <ScmPanel />;
    default: return null;
  }
}

function TabButton({ active, label, icon: Icon, onClick }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'flex flex-1 flex-col items-center justify-center gap-0.5 py-1.5 text-[10px] font-medium transition-colors',
        active ? 'text-primary' : 'text-muted-foreground active:text-foreground',
      )}
    >
      <Icon className="size-5" />
      <span className="leading-none">{label}</span>
    </button>
  );
}

/** Phone layout for the web build: one full-screen view at a time + a bottom tab bar. */
export function MobileShell() {
  const mobileTab = useLayout((s) => s.mobileTab);
  const setMobileTab = useLayout((s) => s.setMobileTab);
  const openSettings = useLayout((s) => s.openSettings);

  // Keep-alive: mount a view on first visit and keep it mounted (hidden) so chat
  // scroll position, editor state and terminal sessions survive tab switches.
  const [mounted, setMounted] = useState(() => new Set([mobileTab]));
  useEffect(() => {
    setMounted((prev) => (prev.has(mobileTab) ? prev : new Set(prev).add(mobileTab)));
  }, [mobileTab]);

  const openFileOnEditorTab = useMemo(
    () => (path, opts) => {
      try { useEditor.getState().openFile(path, opts); } catch {}
      setMobileTab(MOBILE_TABS.EDITOR);
    },
    [setMobileTab],
  );

  const allViews = [
    MOBILE_TABS.AGENT,
    MOBILE_TABS.EXPLORER,
    MOBILE_TABS.EDITOR,
    MOBILE_TABS.TERMINAL,
    MOBILE_TABS.SEARCH,
    MOBILE_TABS.SCM,
  ];

  const moreActive = mobileTab === MOBILE_TABS.SEARCH || mobileTab === MOBILE_TABS.SCM;

  return (
    <div className="flex h-full w-full flex-col bg-background text-foreground">
      <div className="relative min-h-0 flex-1 overflow-hidden">
        {allViews.map((id) =>
          mounted.has(id) ? (
            <div key={id} className={cn('absolute inset-0', mobileTab !== id && 'hidden')}>
              {renderView(id, openFileOnEditorTab)}
            </div>
          ) : null,
        )}
      </div>

      <nav
        className="flex shrink-0 items-stretch border-t border-border bg-sidebar"
        style={{ paddingBottom: 'env(safe-area-inset-bottom)' }}
      >
        {PRIMARY_TABS.map(({ id, label, icon }) => (
          <TabButton
            key={id}
            active={mobileTab === id}
            label={label}
            icon={icon}
            onClick={() => setMobileTab(id)}
          />
        ))}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="More"
              className={cn(
                'flex flex-1 flex-col items-center justify-center gap-0.5 py-1.5 text-[10px] font-medium transition-colors',
                moreActive ? 'text-primary' : 'text-muted-foreground active:text-foreground',
              )}
            >
              <MoreHorizontal className="size-5" />
              <span className="leading-none">More</span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="top" align="end" className="mb-1 min-w-[180px]">
            <DropdownMenuItem onClick={() => setMobileTab(MOBILE_TABS.SEARCH)}>
              <Search className="size-4" /> Search
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => setMobileTab(MOBILE_TABS.SCM)}>
              <GitBranch className="size-4" /> Source Control
            </DropdownMenuItem>
            <DropdownMenuItem onClick={openSettings}>
              <Settings className="size-4" /> Settings
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </nav>
    </div>
  );
}
