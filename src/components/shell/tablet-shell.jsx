import React from 'react';
import { Files, Search, GitBranch, Bot, Terminal as TerminalIcon, Globe, Settings } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useBrowser } from '@/state/browser';
import { SidebarHost } from '@/components/shell/sidebar-host';
import { EditorAreaHost } from '@/components/shell/editor-area-host';
import { BottomPanelHost } from '@/components/shell/bottom-panel-host';
import { StatusBar } from '@/components/shell/status-bar';
import AgentPanel from '@/components/agent/agent-panel';
import { Sheet, SheetContent } from '@/components/ui/sheet';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';

function RailButton({ active, label, icon: Icon, onClick }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          aria-label={label}
          aria-current={active ? 'true' : undefined}
          className={cn(
            'flex size-11 items-center justify-center rounded-xl text-muted-foreground transition-colors',
            'hover:bg-muted hover:text-foreground active:text-foreground',
            active && 'bg-muted text-foreground',
          )}
        >
          <Icon className="size-5" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="right">{label}</TooltipContent>
    </Tooltip>
  );
}

const SIDEBAR_ITEMS = [
  { id: SIDEBAR_PANELS.EXPLORER, label: 'Explorer', icon: Files },
  { id: SIDEBAR_PANELS.SEARCH, label: 'Search', icon: Search },
  { id: SIDEBAR_PANELS.SCM, label: 'Source Control', icon: GitBranch },
];

/** Tablet layout for the web build: a persistent touch rail + push sidebar +
 *  editor, with the agent chat as a right drawer and the terminal as a bottom panel. */
export function TabletShell() {
  const activePanel = useLayout((s) => s.activeSidebarPanel);
  const sidebarVisible = useLayout((s) => s.sidebarVisible);
  const setActivePanel = useLayout((s) => s.setActiveSidebarPanel);
  const openSettings = useLayout((s) => s.openSettings);
  const bottomPanelVisible = useLayout((s) => s.bottomPanelVisible);
  const toggleBottomPanel = useLayout((s) => s.toggleBottomPanel);
  const mobileDrawer = useLayout((s) => s.mobileDrawer);
  const toggleMobileDrawer = useLayout((s) => s.toggleMobileDrawer);
  const closeMobileDrawer = useLayout((s) => s.closeMobileDrawer);
  const browserOpen = useBrowser((s) => s.windowState !== 'closed');

  return (
    <div className="flex h-full w-full bg-background text-foreground">
      <nav className="flex w-14 shrink-0 flex-col items-center gap-1 border-r border-border bg-sidebar py-2">
        {SIDEBAR_ITEMS.map(({ id, label, icon }) => (
          <RailButton
            key={id}
            active={sidebarVisible && activePanel === id}
            label={label}
            icon={icon}
            onClick={() => setActivePanel(id)}
          />
        ))}
        <div className="my-1 h-px w-7 rounded-full bg-border" />
        <RailButton
          label="Agent"
          icon={Bot}
          active={mobileDrawer === 'chat'}
          onClick={() => toggleMobileDrawer('chat')}
        />
        <RailButton
          label="Terminal"
          icon={TerminalIcon}
          active={bottomPanelVisible}
          onClick={toggleBottomPanel}
        />
        <RailButton
          label="Browser"
          icon={Globe}
          active={browserOpen}
          onClick={() => useBrowser.getState().openMaximized()}
        />
        <div className="flex-1" />
        <RailButton label="Settings" icon={Settings} onClick={openSettings} />
      </nav>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex min-h-0 flex-1">
          {sidebarVisible && (
            <aside className="w-72 shrink-0 overflow-hidden border-r border-border bg-sidebar">
              <SidebarHost />
            </aside>
          )}
          <main className="min-w-0 flex-1 overflow-hidden">
            <EditorAreaHost />
          </main>
        </div>
        {bottomPanelVisible && (
          <div className="h-[45%] shrink-0 overflow-hidden border-t border-border">
            <BottomPanelHost />
          </div>
        )}
        <StatusBar />
      </div>

      <Sheet open={mobileDrawer === 'chat'} onOpenChange={(open) => { if (!open) closeMobileDrawer(); }}>
        <SheetContent side="right" className="w-[440px] max-w-[92vw] p-0">
          <AgentPanel />
        </SheetContent>
      </Sheet>
    </div>
  );
}
