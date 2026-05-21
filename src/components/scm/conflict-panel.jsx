import React from 'react';
import { AlertTriangle, FileWarning, FileText } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { toast } from 'sonner';
import { useGit, EMPTY_ARRAY } from '@/state/git';

export default function ConflictPanel({ projectId, onOpenEditor }) {
  const conflicts = useGit(
    (s) => s.projects[projectId]?.conflicts ?? EMPTY_ARRAY
  );
  const resolveConflict = useGit((s) => s.resolveConflict);

  if (!conflicts || conflicts.length === 0) return null;

  async function resolve(path, side) {
    try {
      await resolveConflict(path, side, projectId);
      toast.success(`Resolved ${path} (${side})`);
    } catch (err) {
      toast.error(`Resolve failed: ${err}`);
    }
  }

  return (
    <div className="flex flex-col gap-1 border-y border-destructive/30 bg-destructive/5 px-2 py-2">
      <div className="flex items-center gap-1.5 text-xs font-semibold text-destructive">
        <AlertTriangle className="size-3" />
        Merge Conflicts ({conflicts.length})
      </div>
      <div className="flex flex-col">
        {conflicts.map((c, i) => {
          const path = c.path ?? c.file ?? c;
          return (
            <div
              key={`${path}-${i}`}
              className="group flex min-w-0 items-center gap-1.5 overflow-hidden rounded px-1 py-1 text-xs hover:bg-muted/40"
              title={path}
            >
              <FileWarning className="size-3 shrink-0 text-destructive" />
              <span className="min-w-0 flex-1 truncate">{path}</span>
              <div className="flex shrink-0 items-center gap-0.5">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="xs"
                      className="h-5 px-1.5 text-[10px]"
                      onClick={() => resolve(path, 'ours')}
                    >
                      Ours
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Use our version</TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="xs"
                      className="h-5 px-1.5 text-[10px]"
                      onClick={() => resolve(path, 'theirs')}
                    >
                      Theirs
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Use their version</TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      onClick={() => onOpenEditor?.(path)}
                    >
                      <FileText />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Open in editor</TooltipContent>
                </Tooltip>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
