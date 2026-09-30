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

  return (
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
  );
}

export default LanPairPrompt;
