import React from 'react';
import { cn } from '@/lib/utils';

// Shared empty-state block for the agent surface (dock tabs, sub-agent wait
// state) so the icon + title + hint language stays consistent everywhere.
export function EmptyState({ icon: Icon, title, hint, iconClassName, className }) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-2 px-4 py-6 text-center text-xs text-muted-foreground',
        className,
      )}
    >
      {Icon && <Icon className={cn('size-5 text-muted-foreground/50', iconClassName)} />}
      {title && <div className="font-medium text-foreground/80">{title}</div>}
      {hint && <div className="text-[11px] italic leading-snug">{hint}</div>}
    </div>
  );
}

export default EmptyState;
