import React, { useMemo, useState } from 'react';
import { GitBranch, Plus, Check, Search } from 'lucide-react';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { toast } from 'sonner';
import { useGit, EMPTY_ARRAY } from '@/state/git';
import { cn } from '@/lib/utils';

export default function BranchSwitcher({ projectId, className }) {
  const allBranches = useGit(
    (s) => s.projects[projectId]?.branches ?? EMPTY_ARRAY
  );
  const currentBranch = useGit(
    (s) => s.projects[projectId]?.currentBranch ?? null
  );
  const checkoutBranch = useGit((s) => s.checkoutBranch);
  const createBranch = useGit((s) => s.createBranch);

  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [creating, setCreating] = useState(false);
  const [newBranch, setNewBranch] = useState('');

  const localBranches = useMemo(
    () => allBranches.filter((b) => !b.is_remote),
    [allBranches]
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return localBranches;
    return localBranches.filter((b) =>
      (b.name ?? '').toLowerCase().includes(q)
    );
  }, [localBranches, query]);

  async function handleCheckout(name) {
    if (!name || name === currentBranch) {
      setOpen(false);
      return;
    }
    try {
      await checkoutBranch(name, projectId);
      toast.success(`Switched to ${name}`);
      setOpen(false);
    } catch (err) {
      toast.error(`Checkout failed: ${err}`);
    }
  }

  async function handleCreate() {
    const name = newBranch.trim();
    if (!name) return;
    try {
      await createBranch(name, true, projectId);
      toast.success(`Created branch ${name}`);
      setNewBranch('');
      setCreating(false);
      setOpen(false);
    } catch (err) {
      toast.error(`Create failed: ${err}`);
    }
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={!projectId}
          className={cn("h-6 max-w-[180px] gap-1 px-1.5 text-xs", className)}
        >
          <GitBranch className="size-3" />
          <span className="truncate">{currentBranch ?? 'No branch'}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 p-0">
        <div className="flex items-center gap-2 border-b border-border px-3 py-1.5">
          <Search className="size-3 shrink-0 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Find or create branch"
            className="h-6 border-0 bg-transparent px-0 text-xs shadow-none focus-visible:ring-0 dark:bg-transparent"
          />
        </div>
        <ScrollArea className="max-h-60">
          <div className="py-1">
            {filtered.length === 0 && (
              <div className="px-3 py-2 text-xs text-muted-foreground">
                No branches.
              </div>
            )}
            {filtered.map((b) => {
              const isCurrent = b.name === currentBranch;
              return (
                <button
                  key={b.name}
                  type="button"
                  onClick={() => handleCheckout(b.name)}
                  className={cn(
                    'flex w-full items-center gap-1.5 px-2 py-1 text-left text-xs hover:bg-muted',
                    isCurrent && 'text-foreground'
                  )}
                >
                  <Check
                    className={cn(
                      'size-3 shrink-0',
                      isCurrent ? 'opacity-100' : 'opacity-0'
                    )}
                  />
                  <span className="truncate">{b.name}</span>
                  {b.is_remote && (
                    <span className="ml-auto text-[10px] text-muted-foreground">
                      remote
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </ScrollArea>
        <Separator />
        {creating ? (
          <div className="flex flex-col gap-1.5 p-2">
            <Input
              autoFocus
              value={newBranch}
              onChange={(e) => setNewBranch(e.target.value)}
              placeholder="New branch name"
              className="h-7 text-xs"
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleCreate();
                if (e.key === 'Escape') {
                  setCreating(false);
                  setNewBranch('');
                }
              }}
            />
            <div className="flex gap-1.5">
              <Button
                size="xs"
                onClick={handleCreate}
                disabled={!newBranch.trim()}
                className="flex-1"
              >
                Create
              </Button>
              <Button
                size="xs"
                variant="ghost"
                onClick={() => {
                  setCreating(false);
                  setNewBranch('');
                }}
              >
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <button
            type="button"
            onClick={() => setCreating(true)}
            className="flex w-full items-center gap-1.5 px-2 py-1.5 text-xs hover:bg-muted"
          >
            <Plus className="size-3" />
            Create new branch
          </button>
        )}
      </PopoverContent>
    </Popover>
  );
}
