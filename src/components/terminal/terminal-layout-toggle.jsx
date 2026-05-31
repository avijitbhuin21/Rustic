import React from 'react';
import { Square, Rows3 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTerminal } from '@/state/terminal';

/**
 * Terminal layout control, shown in the panel top bar next to "+".
 *
 * Two modes: Tabs (one pane at a time) and Stack (all terminals full-width in a
 * single scrollable column, each with a resizable height).
 */
export function TerminalLayoutToggle() {
  const layoutMode = useTerminal((s) => s.layoutMode);
  const setLayoutMode = useTerminal((s) => s.setLayoutMode);

  const segCls = (on) =>
    cn(
      'rounded p-1 transition-colors',
      on
        ? 'bg-muted text-foreground'
        : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
    );

  return (
    <div className="flex items-center gap-0.5 rounded-md border border-border/60 p-0.5">
      <button
        title="Tabs — one terminal at a time"
        aria-label="Tabs layout"
        aria-pressed={layoutMode === 'tabs'}
        onClick={() => setLayoutMode('tabs')}
        className={segCls(layoutMode === 'tabs')}
      >
        <Square className="size-3.5" />
      </button>

      <button
        title="Stack — all terminals in a scrollable column"
        aria-label="Stacked column layout"
        aria-pressed={layoutMode === 'grid'}
        onClick={() => setLayoutMode('grid')}
        className={segCls(layoutMode === 'grid')}
      >
        <Rows3 className="size-3.5" />
      </button>
    </div>
  );
}
