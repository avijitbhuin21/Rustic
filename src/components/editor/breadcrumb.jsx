import React, { useMemo } from 'react';
import { useExplorer } from '@/state/explorer';
import { cn } from '@/lib/utils';

export function Breadcrumb({ tab }) {
  const projects = useExplorer((s) => s.projects);

  const segments = useMemo(() => {
    if (!tab?.path) return [];
    const norm = tab.path.replace(/\\/g, '/');
    // Try to make the path relative to a known project root so the breadcrumb
    // shows "src › lib.rs" instead of the full absolute path.
    for (const p of projects) {
      const root = p.root_path.replace(/\\/g, '/').replace(/\/$/, '');
      if (norm.startsWith(root + '/')) {
        return norm.slice(root.length + 1).split('/').filter(Boolean);
      }
    }
    // Fallback: all path segments of the absolute path.
    return norm.split('/').filter(Boolean);
  }, [tab?.path, projects]);

  if (segments.length === 0) return null;

  return (
    <div
      className="flex h-[22px] shrink-0 items-center overflow-hidden border-b border-border/40 bg-background px-3"
      title={tab.path}
    >
      {segments.map((seg, i) => {
        const isLast = i === segments.length - 1;
        return (
          <React.Fragment key={i}>
            {i > 0 && (
              <span className="mx-1 select-none text-[10px] text-muted-foreground/60">›</span>
            )}
            <span
              className={cn(
                'truncate text-[11px]',
                isLast
                  ? 'text-foreground/85'
                  : 'text-muted-foreground'
              )}
            >
              {seg}
            </span>
          </React.Fragment>
        );
      })}
    </div>
  );
}
