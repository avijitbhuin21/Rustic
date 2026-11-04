// "My machine" dialog for Cloud & Sync: name + how this machine is reachable
// (local network, internet, or both). Edits are a draft applied on Save.
import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Copy, Globe, Link2, Loader2, Monitor, Wifi } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';
import { copyText, Slider } from '@/components/settings/cloud-machines';
import { errText } from './use-lan';

/** Current reach ('local' | 'internet' | 'both') derived from LAN status. */
function reachOf(status) {
  const local = status?.local_reach !== false;
  const internet = !!status?.internet_mode;
  if (local && internet) return 'both';
  if (internet) return 'internet';
  return 'local';
}

/** Labelled row inside the dialog. */
function Row({ label, hint, children }) {
  return (
    <div className="space-y-1.5">
      <div className="text-[12px] font-medium">{label}</div>
      {children}
      {hint && <p className="text-[11px] leading-relaxed text-muted-foreground">{hint}</p>}
    </div>
  );
}

/** Address with a copy button. */
function CopyField({ value, label }) {
  return (
    <div className="flex items-center gap-1">
      <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[11.5px]" title={value}>{value}</code>
      <Button size="icon-sm" variant="ghost" className="size-7 shrink-0" title="Copy" onClick={() => copyText(value, `${label} copied`)}>
        <Copy className="size-3.5" />
      </Button>
    </div>
  );
}

/** Centered dialog editing this machine's name and reachability. */
export function MyMachineDialog({ open, onOpenChange, status, onSaved }) {
  const [name, setName] = useState('');
  const [reach, setReach] = useState('local');
  const [mode, setMode] = useState('cloudflare');
  const [saving, setSaving] = useState(false);
  const [download, setDownload] = useState(null);
  const enabled = !!status?.enabled;

  useEffect(() => {
    if (!open) return;
    setName(status?.device_name || '');
    setReach(reachOf(status));
    setMode(status?.internet_mode || 'cloudflare');
    // Re-seed only when the dialog opens, never mid-edit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return undefined;
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('lan-cloudflared-download', (e) => setDownload(e.payload)))
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, [open]);

  const wantLocal = reach !== 'internet';
  const wantInternet = reach !== 'local';
  const targetMode = wantInternet ? mode : 'off';
  const curMode = status?.internet_mode || 'off';
  const trimmed = name.trim();
  const dirty = trimmed !== (status?.device_name || '')
    || wantLocal !== (status?.local_reach !== false)
    || (!IS_WEB && targetMode !== curMode);
  const internetBlocked = wantInternet && !enabled;

  const save = async () => {
    if (!trimmed) { toast.error('Machine name cannot be empty'); return; }
    setSaving(true);
    setDownload(null);
    try {
      if (trimmed !== (status?.device_name || '')) {
        await invoke('lan_set_device_name', { name: trimmed });
      }
      if (wantLocal !== (status?.local_reach !== false)) {
        await invoke('lan_set_local_reach', { enabled: wantLocal });
      }
      if (!IS_WEB && targetMode !== curMode) {
        const url = await invoke('lan_set_internet_mode', { mode: targetMode });
        if (targetMode === 'cloudflare' && url) copyText(url, 'Tunnel URL copied');
      }
      toast.success('My machine saved');
      await onSaved?.();
      onOpenChange(false);
    } catch (e) {
      toast.error(errText(e), { duration: 10000 });
      await onSaved?.();
    } finally {
      setSaving(false);
      setDownload(null);
    }
  };

  const port = status?.default_port || 47820;

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!saving) onOpenChange(o); }}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <span className="flex size-7 items-center justify-center rounded-md bg-muted text-primary"><Monitor className="size-4" /></span>
            My machine
          </DialogTitle>
          <DialogDescription>
            {enabled ? 'Discoverable — other machines can find and pair with this one.' : 'Not discoverable. Turn on the switch in the Cloud & Sync header to let other machines find you.'}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-1">
          <Row label="Machine name" hint="What other machines see when they find or pair with this one.">
            <Input value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && dirty && !saving && save()} className="h-8 text-[12.5px]" />
          </Row>

          {IS_WEB ? (
            <Row label="Server URL" hint="Other machines add this under Add machine → Over the internet.">
              <CopyField value={window.location.origin} label="URL" />
            </Row>
          ) : (
            <>
              <Row
                label="Reachable via"
                hint={reach === 'local'
                  ? 'Only machines on the same network can find you.'
                  : reach === 'internet'
                    ? 'Hidden from local discovery; machines outside your network add you by address. Pairing still needs your approval and the matching code.'
                    : 'Found on your network, and reachable from outside it too.'}
              >
                <Slider
                  value={reach}
                  onChange={setReach}
                  options={[
                    { id: 'local', label: 'Local', icon: Wifi },
                    { id: 'internet', label: 'Internet', icon: Globe },
                    { id: 'both', label: 'Both', icon: Link2 },
                  ]}
                />
              </Row>

              {wantInternet && (
                <Row
                  label="Internet access"
                  hint={mode === 'cloudflare'
                    ? 'A free Cloudflare tunnel gives you a public URL. If cloudflared isn\u2019t installed, Rustic downloads it from Cloudflare\u2019s official GitHub releases.'
                    : null}
                >
                  <Slider
                    value={mode}
                    onChange={setMode}
                    options={[
                      { id: 'cloudflare', label: 'Cloudflare tunnel', icon: Globe },
                      { id: 'portforward', label: 'Port forwarding', icon: Link2 },
                    ]}
                  />
                  {mode === 'portforward' && (
                    <ol className="list-decimal space-y-1 pl-4 pt-1 text-[11.5px] text-muted-foreground">
                      <li>In your router, forward <b>TCP port {port}</b> to <code className="rounded bg-muted px-1 font-mono">{status?.ip || 'this machine'}:{status?.port || port}</code>.</li>
                      <li>Allow port {port} through this machine's firewall.</li>
                      <li>Share <code className="rounded bg-muted px-1 font-mono">your-public-IP:{port}</code> — the other machine adds it under Add machine.</li>
                    </ol>
                  )}
                  {mode === 'cloudflare' && status?.tunnel_url && curMode === 'cloudflare' && (
                    <CopyField value={status.tunnel_url} label="Tunnel URL" />
                  )}
                </Row>
              )}

              {enabled && status?.address && wantLocal && (
                <Row label="IP and port" hint="Share this so a machine on your network can add you by address.">
                  <CopyField value={status.address} label="Address" />
                </Row>
              )}

              {internetBlocked && (
                <p className="rounded-md bg-warning/10 px-2 py-1.5 text-[11.5px] text-warning">
                  Internet access goes through the discovery listener — turn on Discoverable in the header first.
                </p>
              )}
            </>
          )}

          {saving && download?.total > 0 && (
            <div className="space-y-1">
              <div className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
                <Loader2 className="size-3 animate-spin" /> Downloading cloudflared (one-time)…
                <span className="ml-auto tabular-nums">{Math.round((download.done / download.total) * 100)}%</span>
              </div>
              <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
                <div className="h-full rounded-full bg-primary transition-[width] duration-200" style={{ width: `${Math.min(100, (download.done / download.total) * 100)}%` }} />
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" className="h-8 text-xs" disabled={saving} onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button size="sm" className={cn('h-8 min-w-20 text-xs')} disabled={!dirty || saving || internetBlocked} onClick={save}>
            {saving ? <Loader2 className="size-3.5 animate-spin" /> : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default MyMachineDialog;
