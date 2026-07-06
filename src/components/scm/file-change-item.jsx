import React, { memo } from 'react';
import { Plus, Minus, Undo2, FileDiff as FileDiffIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

const STATUS_META = {
  M: { label: 'Modified', className: 'text-yellow-500' },
  A: { label: 'Added', className: 'text-emerald-500' },
  D: { label: 'Deleted', className: 'text-red-500' },
  R: { label: 'Renamed', className: 'text-blue-500' },
  U: { label: 'Conflict', className: 'text-orange-500' },
  '?': { label: 'Untracked', className: 'text-emerald-400' },
};

function basename(path) {
  if (!path) return '';
  const norm = path.replace(/\\/g, '/');
  const idx = norm.lastIndexOf('/');
  return idx === -1 ? norm : norm.slice(idx + 1);
}

function dirname(path) {
  if (!path) return '';
  const norm = path.replace(/\\/g, '/');
  const idx = norm.lastIndexOf('/');
  return idx === -1 ? '' : norm.slice(0, idx);
}

function FileChangeItem({
  file,
  staged = false,
  onStage,
  onUnstage,
  onDiscard,
  onOpenDiff,
}) {
  const path = file.path ?? file.file ?? '';
  const rawStatus = (file.status ?? (file.staged ? 'A' : 'M') ?? 'M').toString();
  const code = rawStatus.charAt(0).toUpperCase();
  const meta = STATUS_META[code] ?? STATUS_META.M;

  return (
    <div
      className="group flex min-w-0 items-center gap-1.5 overflow-hidden rounded px-2 py-0 text-xs hover:bg-muted/60"
      title={path}
    >
      <button
        type="button"
        onClick={() => onOpenDiff?.(file)}
        className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
      >
        <FileDiffIcon className="size-3 shrink-0 text-muted-foreground" />
        <span className="truncate text-foreground">{basename(path)}</span>
        <span className="truncate text-[10px] text-muted-foreground">
          {dirname(path)}
        </span>
      </button>
      <div className="flex shrink-0 items-center gap-0.5 opacity-0 group-hover:opacity-100 focus-visible:opacity-100 focus-within:opacity-100">
        {!staged && onDiscard && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={(e) => {
                  e.stopPropagation();
                  onDiscard(path);
                }}
              >
                <Undo2 />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Discard changes</TooltipContent>
          </Tooltip>
        )}
        {staged ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={(e) => {
                  e.stopPropagation();
                  onUnstage?.(path);
                }}
              >
                <Minus />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Unstage</TooltipContent>
          </Tooltip>
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={(e) => {
                  e.stopPropagation();
                  onStage?.(path);
                }}
              >
                <Plus />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Stage</TooltipContent>
          </Tooltip>
        )}
      </div>
      <span
        className={cn(
          'w-4 shrink-0 text-center font-mono text-[10px] font-semibold',
          meta.className
        )}
        title={meta.label}
      >
        {code}
      </span>
    </div>
  );
}

export default memo(FileChangeItem, (prev, next) =>
  prev.staged === next.staged &&
  (prev.file.path ?? prev.file.file) === (next.file.path ?? next.file.file) &&
  prev.file.status === next.file.status &&
  prev.onStage === next.onStage &&
  prev.onUnstage === next.onUnstage &&
  prev.onDiscard === next.onDiscard &&
  prev.onOpenDiff === next.onOpenDiff
);
