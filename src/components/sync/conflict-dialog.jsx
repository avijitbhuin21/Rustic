// Name-conflict prompt for file transfers: auto-rename (-1, -2…), replace,
// skip, or type a new name per item. Resolves to `{ policy, renames }` or null.
import React, { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { InfoTip } from '@/components/ui/info-tip';

const POLICIES = [
  { id: 'auto_rename', label: 'Keep both', hint: 'adds -1, -2…' },
  { id: 'replace', label: 'Replace', hint: 'overwrites theirs' },
  { id: 'skip', label: 'Skip', hint: 'leave existing' },
];

/** Suggested `name-1.ext`. */
function suggest(name) {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? `${name.slice(0, dot)}-1${name.slice(dot)}` : `${name}-1`;
}

/** Controlled dialog; `request = { names, where }` opens it. */
export function ConflictDialog({ request, onResolve }) {
  const [policy, setPolicy] = useState('auto_rename');
  const [renames, setRenames] = useState({});
  const [custom, setCustom] = useState(false);
  useEffect(() => {
    if (request) { setPolicy('auto_rename'); setRenames({}); setCustom(false); }
  }, [request]);
  const names = request?.names || [];
  const done = () => {
    const r = custom
      ? Object.fromEntries(Object.entries(renames).filter(([k, v]) => v && v.trim() && v.trim() !== k).map(([k, v]) => [k, v.trim()]))
      : {};
    onResolve({ policy, renames: r });
  };
  return (
    <Dialog open={!!request} onOpenChange={(o) => !o && onResolve(null)}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{names.length === 1 ? `"${names[0]}" already exists` : `${names.length} items already exist`}</DialogTitle>
          <DialogDescription>In {request?.where || 'the destination'}. Nothing is overwritten unless you choose Replace.</DialogDescription>
        </DialogHeader>
        <div className="grid grid-cols-3 gap-1.5">
          {POLICIES.map((p) => (
            <button
              key={p.id}
              type="button"
              onClick={() => setPolicy(p.id)}
              className={cn(
                'rounded-lg border px-2 py-2 text-left transition-colors',
                policy === p.id ? 'border-primary/60 bg-primary/10' : 'border-border/50 hover:bg-muted/40',
              )}
            >
              <div className="text-[12px] font-medium">{p.label}</div>
              <div className="text-[10.5px] text-muted-foreground">{p.hint}</div>
            </button>
          ))}
        </div>
        <label className="flex items-center gap-2 text-[11.5px] text-muted-foreground">
          <input type="checkbox" checked={custom} onChange={(e) => setCustom(e.target.checked)} />
          Rename specific items myself
          <InfoTip>Items left blank follow the choice above.</InfoTip>
        </label>
        {custom && (
          <div className="max-h-56 space-y-1.5 overflow-y-auto">
            {names.map((n) => (
              <div key={n} className="flex items-center gap-2">
                <span className="w-36 shrink-0 truncate font-mono text-[11.5px]" title={n}>{n}</span>
                <Input
                  className="h-7 text-xs"
                  placeholder={suggest(n)}
                  value={renames[n] ?? ''}
                  onChange={(e) => setRenames((r) => ({ ...r, [n]: e.target.value }))}
                />
              </div>
            ))}
          </div>
        )}
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => onResolve(null)}>Cancel</Button>
          <Button size="sm" variant={policy === 'replace' ? 'destructive' : 'default'} className="h-7 text-xs" onClick={done}>Continue</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Hook: `ask(names, where)` → Promise<{policy, renames} | null>; render `dialog`. */
export function useConflictPrompt() {
  const [state, setState] = useState(null);
  const ask = (names, where) => new Promise((resolve) => setState({ names, where, resolve }));
  const dialog = (
    <ConflictDialog
      request={state}
      onResolve={(v) => { state?.resolve(v); setState(null); }}
    />
  );
  return { ask, dialog };
}
