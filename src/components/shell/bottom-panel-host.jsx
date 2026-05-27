import React from 'react';
import { Maximize2, Minimize2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useLayout } from '@/state/layout';
import { TerminalPanel } from '@/components/terminal/terminal-panel';

export function BottomPanelHost() {
  const bottomPanelFullscreen = useLayout((s) => s.bottomPanelFullscreen);
  const toggleBottomPanelFullscreen = useLayout((s) => s.toggleBottomPanelFullscreen);
  const setBottomPanelVisible = useLayout((s) => s.setBottomPanelVisible);

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {/* Header with fullscreen and close buttons */}
      <div className="flex h-7 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="text-xs font-medium text-muted-foreground">Terminal</span>
        <div className="flex items-center gap-1">
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
