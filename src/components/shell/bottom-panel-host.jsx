import React from 'react';
import { useLayout } from '@/state/layout';
import { cn } from '@/lib/utils';

const TABS = [
  { id: 'problems', label: 'Problems' },
  { id: 'output', label: 'Output' },
];

export function BottomPanelHost() {
  const activeTab = useLayout((s) => s.bottomPanelTab);
  const setTab = useLayout((s) => s.setBottomPanelTab);

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-border px-2 text-xs">
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              'px-2 py-1 uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground',
              activeTab === t.id && 'text-foreground border-b border-primary -mb-px'
            )}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="relative flex-1 overflow-hidden">
        {activeTab === 'problems' && (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            Problems — populated by LSP/lint output.
          </div>
        )}
        {activeTab === 'output' && (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            Output — agent / build / task streams.
          </div>
        )}
      </div>
    </div>
  );
}
