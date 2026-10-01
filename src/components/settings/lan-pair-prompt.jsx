// Global Accept/Decline prompt for local-network pairing requests, plus
// notices when a paired device syncs into this machine (issue #15).
import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';

/** Mounted once in App; shows incoming pairing requests one at a time. */
export function LanPairPrompt() {
  const [queue, setQueue] = useState([]);
  const current = queue[0] || null;
  const [transfers, setTransfers] = useState([]);
  const transfer = transfers[0] || null;

  useEffect(() => {
    const unlistens = [];
    let disposed = false;
    import('@tauri-apps/api/event')
      .then(async ({ listen }) => {
        const add = (fn) => (disposed ? fn() : unlistens.push(fn));
        add(await listen('lan-pair-request', (e) => {
          const p = e.payload || {};
          if (p.request_id) setQueue((q) => [...q, p]);
        }));
        add(await listen('lan-transfer-request', (e) => {
          const p = e.payload || {};
          if (p.request_id) setTransfers((q) => [...q, p]);
        }));
        add(await listen('lan-sync-received', (e) => {
          const p = e.payload || {};
          if (p.metadata) {
            toast.success(`${p.from} synced metadata (providers, rules, skills, MCP) into this machine`);
            window.dispatchEvent(new CustomEvent('rustic:extensions-changed'));
          } else if (p.scoped) {
            toast.success(`${p.from} pushed a project to this machine`);
          } else {
            toast.success(`${p.from} replaced this machine's environment (${p.projects} project(s))`, {
              duration: Infinity,
              action: { label: 'Reload', onClick: () => window.location.reload() },
            });
          }
        }));
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlistens.forEach((fn) => fn());
    };
  }, []);

  const respond = async (accept) => {
    if (!current) return;
    setQueue((q) => q.slice(1));
    try {
      await invoke('lan_respond_pair', { requestId: current.request_id, accept });
      if (accept) toast.success(`Paired with ${current.name}`);
    } catch (e) {
      toast.error(String(e?.message || e));
    }
  };

  const respondTransfer = async (accept) => {
    if (!transfer) return;
    setTransfers((q) => q.slice(1));
    try {
      await invoke('lan_respond_transfer', { requestId: transfer.request_id, accept });
    } catch (e) {
      toast.error(String(e?.message || e));
    }
  };

  const isPush = transfer?.kind === 'push';
  const tProjects = transfer?.projects || [];
  const tMeta = transfer?.meta || [];
  const replacing = tProjects.filter((p) => p.exists);

  return (
    <>
    <Dialog open={!!transfer} onOpenChange={(open) => !open && respondTransfer(false)}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {isPush ? `${transfer?.from} wants to push to this machine` : `${transfer?.from} wants to pull from this machine`}
          </DialogTitle>
          <DialogDescription>
            {isPush
              ? 'These items will be written into your workspace. Projects you already have are replaced with the incoming copy; selected metadata overwrites same-name items.'
              : 'These items will be sent to the other machine. Nothing leaves this machine unless you approve.'}
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-72 space-y-3 overflow-y-auto rounded-md border border-border/50 p-2 text-[12px]">
          {tProjects.length > 0 && (
            <div className="space-y-1">
              <div className="text-[10.5px] uppercase tracking-wider text-muted-foreground/70">Projects</div>
              {tProjects.map((p) => (
                <div key={p.id} className="flex items-center gap-2">
                  <span className="truncate">{p.name || p.id}</span>
                  {isPush && (
                    <span className={p.exists ? 'ml-auto text-[10.5px] text-amber-500' : 'ml-auto text-[10.5px] text-emerald-500'}>
                      {p.exists ? 'replaces yours' : 'new'}
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}
          {tMeta.length > 0 && (
            <div className="space-y-1">
              <div className="text-[10.5px] uppercase tracking-wider text-muted-foreground/70">Metadata</div>
              {tMeta.map((m) => (
                <div key={m.key} className="flex items-center gap-2">
                  <span className="w-24 shrink-0 text-[10.5px] text-muted-foreground">{m.category}</span>
                  <span className="truncate font-mono">{m.name}</span>
                </div>
              ))}
            </div>
          )}
        </div>
        {isPush && replacing.length > 0 && (
          <p className="text-[11.5px] text-amber-500">
            {replacing.length} project{replacing.length === 1 ? '' : 's'} on this machine will be wiped and replaced.
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => respondTransfer(false)}>Deny</Button>
          <Button size="sm" variant={isPush && replacing.length ? 'destructive' : 'default'} className="h-7 text-xs" onClick={() => respondTransfer(true)}>
            {isPush ? 'Accept push' : 'Approve pull'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    <Dialog open={!!current} onOpenChange={(open) => !open && respond(false)}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Pair with {current?.name}?</DialogTitle>
          <DialogDescription>
            A Rustic desktop on your network wants to sync with this machine. Paired devices can push and pull
            projects, API keys and settings. Only accept if the code matches the one on {current?.name}.
          </DialogDescription>
        </DialogHeader>
        <div className="py-2 text-center font-mono text-3xl tracking-[0.35em]">{current?.code}</div>
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => respond(false)}>Decline</Button>
          <Button size="sm" className="h-7 text-xs" onClick={() => respond(true)}>Accept</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    </>
  );
}

export default LanPairPrompt;
