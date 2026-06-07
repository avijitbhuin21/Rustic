import React, { useEffect, useMemo, useRef, useState } from 'react';
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
  const fetchRemote = useGit((s) => s.fetch);
  const remoteUrl = useGit((s) => s.projects[projectId]?.remoteUrl ?? null);

  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [creating, setCreating] = useState(false);
  const [newBranch, setNewBranch] = useState('');

  // Unified branch list: all locals, plus remote-tracking branches that have no
  // local counterpart yet. Previously this filtered remotes out entirely, so a
  // branch that only existed on the remote was invisible until you checked it
  // out. Remote entries arrive as `origin/<branch>`; we display the short name,
  // drop the `origin/HEAD` symbolic pointer, and de-dupe against locals.
  const items = useMemo(() => {
    const locals = allBranches.filter((b) => !b.is_remote);
    // Seed with local names so a remote that already has a local copy is hidden;
    // it also de-dupes the same branch across multiple remotes (origin/upstream).
    const seen = new Set(locals.map((b) => b.name));
    const out = locals.map((b) => ({
      key: b.name,
      label: b.name,
      checkoutName: b.name,
      isRemote: false,
    }));
    for (const b of allBranches) {
      if (!b.is_remote) continue;
      const slash = (b.name ?? '').indexOf('/');
      const shortName = slash >= 0 ? b.name.slice(slash + 1) : b.name;
      if (!shortName || shortName === 'HEAD') continue; // origin/HEAD isn't a branch
      if (seen.has(shortName)) continue; // already shown (local, or another remote)
      seen.add(shortName);
      // Checkout by the SHORT name so git creates a local tracking branch
      // (checking out `origin/x` directly would land in detached HEAD).
      out.push({ key: b.name, label: shortName, checkoutName: shortName, isRemote: true });
    }
    return out;
  }, [allBranches]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((b) => (b.label ?? '').toLowerCase().includes(q));
  }, [items, query]);

  // When the switcher opens, refresh remote-tracking refs once so remote-only
  // branches actually show up (without a fetch, refs/remotes holds only what the
  // last clone/fetch left — which is why branches were "missing" until checkout).
  // Best-effort and silent: a private repo with no token just keeps the locals.
  const fetchedRef = useRef(false);
  useEffect(() => {
    if (!open || !remoteUrl || fetchedRef.current) return;
    fetchedRef.current = true;
    fetchRemote(projectId).catch(() => {});
  }, [open, remoteUrl, projectId, fetchRemote]);

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
              const isCurrent = b.checkoutName === currentBranch;
              return (
                <button
                  key={b.key}
                  type="button"
                  onClick={() => handleCheckout(b.checkoutName)}
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
                  <span className="truncate">{b.label}</span>
                  {b.isRemote && (
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
