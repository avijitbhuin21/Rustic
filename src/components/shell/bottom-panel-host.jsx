import React from 'react';
import { Maximize2, Minimize2, X, Plus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useLayout } from '@/state/layout';
import { TerminalPanel } from '@/components/terminal/terminal-panel';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';

export function BottomPanelHost() {
  const bottomPanelFullscreen = useLayout((s) => s.bottomPanelFullscreen);
  const toggleBottomPanelFullscreen = useLayout((s) => s.toggleBottomPanelFullscreen);
  const setBottomPanelVisible = useLayout((s) => s.setBottomPanelVisible);
  const chatDockOpen = useLayout((s) => s.chatDockOpen);

  // The header's top-right buttons only collide with the OS window controls
  // (fixed top-right) when this panel actually reaches the window's right edge:
  // i.e. it's fullscreen AND nothing is docked to its right. When the chat dock
  // is open the terminal stops short of the edge, so reserving 138px there just
  // leaves a dead gap — only offset when we're truly the rightmost element.
  const needsWindowControlsOffset = bottomPanelFullscreen && !chatDockOpen;

  // One-click new terminal: open the project picker so the user chooses which
  // project's root the terminal opens in. Reuses the existing picker dialog
  // (TerminalProjectPicker listens for this window event).
  const openTerminalPicker = () =>
    window.dispatchEvent(new Event(TERMINAL_PICKER_EVENT));

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {/* Header with new-terminal, fullscreen and close buttons. We only
          reserve 138px on the right when this panel reaches the window's right
          edge (see needsWindowControlsOffset) so the buttons clear the fixed OS
          window controls; otherwise they sit flush at the panel's edge. */}
      <div
        className="flex h-7 shrink-0 items-center justify-between border-b border-border/60 px-2"
        style={{ paddingRight: needsWindowControlsOffset ? 138 : undefined }}
      >
        <span className="text-xs font-medium text-muted-foreground">Terminal</span>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={openTerminalPicker}
            title="New terminal"
          >
            <Plus className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={toggleBottomPanelFullscreen}
            title={bottomPanelFullscreen ? "Exit fullscreen" : "Fullscreen"}
          >
            {bottomPanelFullscreen ? (
              <Minimize2 className="size-3.5" />
            ) : (
              <Maximize2 className="size-3.5" />
            )}
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={() => setBottomPanelVisible(false)}
            title="Close panel"
          >
            <X className="size-3.5" />
          </Button>
        </div>
      </div>
      <div className="flex-1 overflow-hidden">
        <TerminalPanel />
      </div>
    </div>
  );
}
