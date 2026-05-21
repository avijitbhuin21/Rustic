import React, { useState } from 'react';
import { ChevronDown, ChevronRight, Wrench, AlertCircle, CheckCircle2 } from 'lucide-react';
import { cn } from '@/lib/utils';

function formatValue(v) {
  if (v === undefined || v === null) return '';
  if (typeof v === 'string') return v;
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}

export function ToolCallCard({ name, input, output, isError, defaultOpen = false }) {
  const [open, setOpen] = useState(defaultOpen);
  const hasResult = output !== undefined && output !== null;
  return (
    <div
      className={cn(
        'rounded-md border border-border bg-muted/40 text-xs',
        isError && 'border-destructive/50'
      )}
    >
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left hover:bg-muted/60"
      >
        {open ? (
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="size-3 shrink-0 text-muted-foreground" />
        )}
        <Wrench className="size-3 shrink-0 text-muted-foreground" />
        <span className="font-mono text-foreground">{name}</span>
        <span className="ml-auto flex items-center gap-1">
          {hasResult &&
            (isError ? (
              <AlertCircle className="size-3 text-destructive" />
            ) : (
              <CheckCircle2 className="size-3 text-emerald-500" />
            ))}
        </span>
      </button>
      {open && (
        <div className="space-y-2 border-t border-border px-2 py-2">
          {input !== undefined && (
            <div>
              <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                Input
              </div>
              <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-background/60 p-1.5 font-mono text-[11px] text-foreground/90">
                {formatValue(input)}
              </pre>
            </div>
          )}
          {hasResult && (
            <div>
              <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                {isError ? 'Error' : 'Output'}
              </div>
              <pre
                className={cn(
                  'overflow-x-auto whitespace-pre-wrap break-words rounded bg-background/60 p-1.5 font-mono text-[11px]',
                  isError ? 'text-destructive' : 'text-foreground/90'
                )}
              >
                {formatValue(output)}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default ToolCallCard;
