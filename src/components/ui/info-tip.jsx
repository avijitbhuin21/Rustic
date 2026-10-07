// Small "i" icon that reveals a description on hover (or tap / click on touch
// devices). Used everywhere a row or section used to print its help text inline.
import React, { useState } from 'react';
import { Info } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

/** Info icon whose tooltip shows `children` (renders nothing when empty). */
export function InfoTip({ children, side = 'top', align = 'center', className, iconClassName, label = 'More info' }) {
  const [open, setOpen] = useState(false);
  if (children == null || children === false || children === '') return null;
  return (
    <Tooltip open={open} onOpenChange={setOpen} delayDuration={150}>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={label}
          onClick={(e) => { e.preventDefault(); e.stopPropagation(); setOpen((v) => !v); }}
          className={cn(
            'inline-flex size-4 shrink-0 items-center justify-center rounded-full align-middle text-muted-foreground/70 transition-colors',
            'hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            className,
          )}
        >
          <Info className={cn('size-3.5', iconClassName)} />
        </button>
      </TooltipTrigger>
      <TooltipContent side={side} align={align} className="max-w-xs text-[11.5px] font-normal normal-case leading-snug tracking-normal">
        {children}
      </TooltipContent>
    </Tooltip>
  );
}

export default InfoTip;
