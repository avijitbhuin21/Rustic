import React from 'react';
import { Maximize2, Minimize2, X, Plus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { WINDOW_CONTROLS_OFFSET } from '@/components/shell/window-controls';
import { useLayout } from '@/state/layout';
import { IS_WEB, MOD_LABEL } from '@/lib/platform';
import { TerminalPanel } from '@/components/terminal/terminal-panel';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';
import { useTerminal } from '@/state/terminal';
import { cn } from '@/lib/utils';

/** Header switch between your terminals and every live agent terminal. */
function TerminalViewSwitch({ view, onChange, agentCount }) {
  const opts = [
    { id: 'user', label: 'User terminals' },
    { id: 'agent', label: 'Agent terminals', count: agentCount },
  ];
  return (
    <div className="inline-flex items-center gap-0.5 rounded-md bg-muted/40 p-0.5" role="tablist" aria-label="Terminal view">
      {opts.map((o) => (
        <button
          key={o.id}
          type="button"
          role="tab"
          aria-selected={view === o.id}
          onClick={() => onChange(o.id)}
          className={cn(
            'flex h-5 items-center gap-1 rounded px-2 text-[11px] font-medium transition-colors',
            view === o.id ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {o.label}
          {o.count > 0 && (
            <span className="rounded-full bg-muted px-1.5 text-[9.5px] tabular-nums text-muted-foreground">{o.count}</span>
          )}
        </button>
      ))}
    </div>
  );
}

export function BottomPanelHost() {
  const terminalView = useTerminal((s) => s.terminalView);
  const setTerminalView = useTerminal((s) => s.setTerminalView);
  const agentCount = useTerminal((s) => s.sessions.filter((x) => x.is_agent && !x.exited).length);
  const bottomPanelFullscreen = useLayout((s) => s.bottomPanelFullscreen);
  const toggleBottomPanelFullscreen = useLayout((s) => s.toggleBottomPanelFullscreen);
  const setBottomPanelVisible = useLayout((s) => s.setBottomPanelVisible);
  const chatDockOpen = useLayout((s) => s.chatDockOpen);

  // The header's top-right buttons only collide with the OS window controls
  // (fixed top-right) when this panel actually reaches the window's right edge:
  // i.e. it's fullscreen AND nothing is docked to its right. When the chat dock
  // is open the terminal stops short of the edge, so reserving the offset just
  // leaves a dead gap — only offset when we're truly the rightmost element.
  const needsWindowControlsOffset = bottomPanelFullscreen && !chatDockOpen && !IS_WEB;

  // One-click new terminal: open the project picker so the user chooses which
  // project's root the terminal opens in. Reuses the existing picker dialog
  // (TerminalProjectPicker listens for this window event).
  const openTerminalPicker = () =>
    window.dispatchEvent(new Event(TERMINAL_PICKER_EVENT));

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {/* Header with new-terminal, fullscreen and close buttons. We only
          reserve the window-controls offset on the right when this panel
          reaches the window's right edge (see needsWindowControlsOffset) so
          the buttons clear the fixed OS window controls; otherwise they sit
          flush at the panel's edge. */}
      <div
        className="flex h-7 shrink-0 items-center justify-between border-b border-border/60 px-2"
        style={{ paddingRight: needsWindowControlsOffset ? WINDOW_CONTROLS_OFFSET : undefined }}
      >
        <TerminalViewSwitch view={terminalView} onChange={setTerminalView} agentCount={agentCount} />
        <div className="flex items-center gap-1">
          {terminalView === 'user' && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon-xs" onClick={openTerminalPicker}>
                  <Plus className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">New terminal (Ctrl+`)</TooltipContent>
            </Tooltip>
          )}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={toggleBottomPanelFullscreen}>
                {bottomPanelFullscreen ? (
                  <Minimize2 className="size-3.5" />
                ) : (
                  <Maximize2 className="size-3.5" />
                )}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">
              {bottomPanelFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={() => setBottomPanelVisible(false)}>
                <X className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">Close panel ({MOD_LABEL}+J)</TooltipContent>
          </Tooltip>
        </div>
      </div>
      <div className="flex-1 overflow-hidden">
        <TerminalPanel />
      </div>
    </div>
  );
}
