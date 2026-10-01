// Settings > Cloud & Sync. Top: "My machine" (name, address, local-network
// sync, internet tunnel). Below, a Sync | Backends slider:
//  - Sync: every machine you can push to / pull from (paired desktops and
//    remote backends, which are tinted). Opening one shows Push | Pull
//    (| Sharing for desktops) with searchable, selectable projects and
//    metadata. Desktop transfers need approval on the other machine.
//  - Backends: saved remote rustic-servers; click to open in a new window.
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Loader2, CloudUpload, CloudDownload, Plus, ExternalLink, LogOut, Pencil, Trash2,
  Link2, Copy, ChevronDown, Monitor, ShieldCheck, Globe,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { useExplorer } from '@/state/explorer';
import { IS_WEB } from '@/lib/platform';
import { GROUP_BOX, GROUP_TITLE, SettingRow } from './setting-row';
import {
  useSavedBackends, copyText, InlineRename, Slider, SearchBox, ItemPicker, MachineCard,
  AddMachineDialog,
} from './cloud-machines';

const errText = (e) => String(e?.message || e);

const META_LABELS = {
  provider: 'Provider & key', model: 'Model settings', rule: 'Rule', skill: 'Skill', workflow: 'Workflow', mcp_server: 'MCP server',
};

const PHASE_LABELS = {
  connecting: 'Connecting', preparing: 'Preparing', archiving: 'Packing files', compressing: 'Compressing',
  uploading: 'Uploading', applying: 'Applying', packing: 'Packing', downloading: 'Downloading',
  extracting: 'Extracting', installing: 'Installing', writing: 'Writing files', finalizing: 'Finalizing', done: 'Done',
};

/** `scheme://host[:port]` of a URL, matching what the backend reports for open windows. */
function originOf(url) {
  try { return new URL(url).origin; } catch { return url; }
}

/** Ask for a folder to place a project that doesn't exist here yet. */
async function chooseFolder(name) {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    return await open({ directory: true, multiple: false, title: `Choose where to put "${name}"` });
  } catch {
    return null;
  }
}

/** Expandable "My machine" card: name, LAN address, local-network sync and internet tunnel. */
function MyMachine({ status, busy, onToggleLan, onRename, onRefresh }) {
  const [open, setOpen] = useState(true);
  const [name, setName] = useState('');
  const [tunnelBusy, setTunnelBusy] = useState(false);
  const [download, setDownload] = useState(null);
  useEffect(() => { setName(status?.device_name || ''); }, [status?.device_name]);
  useEffect(() => {
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('lan-cloudflared-download', (e) => setDownload(e.payload)))
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, []);
  const enabled = !!status?.enabled;
  const mode = status?.internet_mode || null;

  const setMode = async (next) => {
    setTunnelBusy(true);
    setDownload(null);
    try {
      const url = await invoke('lan_set_internet_mode', { mode: next });
      if (next === 'cloudflare' && url) {
        toast.success('Tunnel is live — URL copied');
        copyText(url, 'Tunnel URL copied');
      }
      await onRefresh();
    } catch (e) {
      toast.error(errText(e), { duration: 10000 });
      await onRefresh();
    } finally {
      setTunnelBusy(false);
      setDownload(null);
    }
  };

  return (
    <section className="mb-5">
      <div className={GROUP_BOX}>
        <button type="button" onClick={() => setOpen((v) => !v)} className="flex w-full items-center gap-3 px-3 py-3 text-left hover:bg-muted/30">
          <span className="flex size-8 items-center justify-center rounded-lg bg-primary/15 text-primary"><Monitor className="size-4" /></span>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium">{status?.device_name || 'My machine'}</div>
            <div className="text-[11px] text-muted-foreground">
              {IS_WEB
                ? `${window.location.origin}${enabled ? '' : ' · pairing off'}`
                : enabled ? (status?.address || `port ${status?.port}`) : 'Local-network sync is off'}
              {!IS_WEB && (mode === 'cloudflare' && status?.tunnel_url ? ' · reachable via Cloudflare tunnel' : mode === 'portforward' ? ' · port forwarding' : '')}
            </div>
          </div>
          <ChevronDown className={cn('size-4 text-muted-foreground transition-transform', open && 'rotate-180')} />
        </button>
        {open && (
          <div className="divide-y divide-border/40 border-t border-border/40 px-3">
            <SettingRow label="Machine name" description="What other machines see when they find or pair with this one.">
              <div className="flex items-center gap-1.5">
                <Input value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && onRename(name)} className="h-7 w-48 text-xs" />
                {name !== (status?.device_name || '') && (
                  <Button size="sm" className="h-7 text-xs" onClick={() => onRename(name)}>Save</Button>
                )}
              </div>
            </SettingRow>
            <SettingRow
              label={IS_WEB ? 'Allow other machines to pair and sync' : 'Local network sync'}
              description={IS_WEB
                ? 'Lets desktops and other servers pair with this server and push or pull. Pushes are accepted automatically; pulls ask here first and only include what you share.'
                : 'Lets other Rustic desktops find this machine, pair with it and ask to sync. Windows may ask to allow network access.'}
            >
              <Switch checked={enabled} disabled={busy} onCheckedChange={onToggleLan} />
            </SettingRow>
            {IS_WEB && (
              <SettingRow label="Server URL" description="Other machines add this under Add machine → Over the internet.">
                <div className="flex items-center gap-1">
                  <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11.5px]">{window.location.origin}</code>
                  <Button size="icon-sm" variant="ghost" className="size-7" title="Copy" onClick={() => copyText(window.location.origin, 'URL copied')}><Copy className="size-3.5" /></Button>
                </div>
              </SettingRow>
            )}
            {enabled && !IS_WEB && (
              <SettingRow label="IP and port" description="Share this so another machine can add you by address.">
                {status?.address ? (
                  <div className="flex items-center gap-1">
                    <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11.5px]">{status.address}</code>
                    <Button size="icon-sm" variant="ghost" className="size-7" title="Copy" onClick={() => copyText(status.address, 'Address copied')}><Copy className="size-3.5" /></Button>
                  </div>
                ) : <span className="text-[11.5px] text-muted-foreground">port {status?.port || '—'}</span>}
              </SettingRow>
            )}
            {enabled && !IS_WEB && (
              <SettingRow
                label="Reachable over the internet"
                description="Let machines outside your network add you — through a free Cloudflare tunnel, or by forwarding a port on your router."
              >
                <div className="flex items-center gap-1.5">
                  {tunnelBusy && <Loader2 className="size-3.5 animate-spin text-muted-foreground" />}
                  <Switch
                    checked={!!mode}
                    disabled={tunnelBusy}
                    onCheckedChange={(on) => setMode(on ? 'cloudflare' : 'off')}
                  />
                </div>
              </SettingRow>
            )}
            {enabled && mode && !IS_WEB && (
              <div className="space-y-2.5 py-3" data-setting-row>
                <Slider
                  value={mode}
                  onChange={(m) => m !== mode && !tunnelBusy && setMode(m)}
                  options={[
                    { id: 'cloudflare', label: 'Cloudflare tunnel', icon: Globe },
                    { id: 'portforward', label: 'Port forwarding', icon: Link2 },
                  ]}
                />
                {mode === 'cloudflare' ? (
                  tunnelBusy ? (
                    <div className="space-y-1.5">
                      <div className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
                        <Loader2 className="size-3 animate-spin" />
                        {download ? 'Downloading cloudflared (one-time)…' : 'Starting tunnel…'}
                        {download?.total > 0 && (
                          <span className="ml-auto tabular-nums">{Math.round((download.done / download.total) * 100)}%</span>
                        )}
                      </div>
                      {download?.total > 0 && (
                        <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
                          <div className="h-full rounded-full bg-primary transition-[width] duration-200" style={{ width: `${Math.min(100, (download.done / download.total) * 100)}%` }} />
                        </div>
                      )}
                    </div>
                  ) : status?.tunnel_url ? (
                    <div className="flex items-center gap-2">
                      <span className="text-[11.5px] text-muted-foreground">Public URL</span>
                      <code className="min-w-0 flex-1 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[11.5px]" title={status.tunnel_url}>{status.tunnel_url}</code>
                      <Button size="icon-sm" variant="ghost" className="size-7" title="Copy" onClick={() => copyText(status.tunnel_url, 'URL copied')}><Copy className="size-3.5" /></Button>
                    </div>
                  ) : (
                    <div className="flex items-center justify-between gap-2 text-[11.5px] text-amber-500">
                      Tunnel isn't running.
                      <Button size="sm" variant="outline" className="h-7 text-xs" onClick={() => setMode('cloudflare')}>Start</Button>
                    </div>
                  )
                ) : (
                  <ol className="list-decimal space-y-1 pl-4 text-[11.5px] text-muted-foreground">
                    <li>
                      In your router, forward <b>TCP port {status?.default_port || 47820}</b> to{' '}
                      <code className="rounded bg-muted px-1 font-mono">{status?.ip || 'this machine'}:{status?.port || 47820}</code>.
                    </li>
                    <li>Allow port {status?.default_port || 47820} through this machine's firewall.</li>
                    <li>Share <code className="rounded bg-muted px-1 font-mono">your-public-IP:{status?.default_port || 47820}</code> — the other machine adds it under Add machine → Over the internet.</li>
                  </ol>
                )}
                {mode === 'cloudflare' && (
                  <p className="text-[10.5px] text-muted-foreground">
                    Anyone with the link can ask to pair — pairing still needs your approval and the matching code. If cloudflared isn't installed, Rustic downloads it from Cloudflare's official GitHub releases.
                  </p>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/** Push / Pull / Sharing for one machine, with searchable selectable projects + metadata. */
function MachineView({ machine, localProjects, localMeta, onRunning }) {
  const isLan = machine.kind === 'lan';
  const [pane, setPane] = useState('pull');
  const [theirProjects, setTheirProjects] = useState(null);
  const [theirMeta, setTheirMeta] = useState(null);
  const [selP, setSelP] = useState(new Set());
  const [selM, setSelM] = useState(new Set());
  const [share, setShare] = useState(null);
  const [shareP, setShareP] = useState(new Set());
  const [shareM, setShareM] = useState(new Set());
  const [running, setRunning] = useState(null);

  useEffect(() => { setSelP(new Set()); setSelM(new Set()); }, [pane]);

  useEffect(() => {
    if (pane !== 'pull') return undefined;
    let alive = true;
    setTheirProjects(null); setTheirMeta(null);
    const listCmd = isLan ? invoke('lan_list_projects', { deviceId: machine.id }) : invoke('cloud_list_remote_projects', { url: machine.url });
    const metaCmd = isLan ? invoke('lan_meta_preview', { deviceId: machine.id, direction: 'pull' }) : invoke('cloud_meta_preview', { url: machine.url, direction: 'pull' });
    listCmd.then((l) => alive && setTheirProjects(Array.isArray(l) ? l : [])).catch((e) => { if (alive) { setTheirProjects([]); toast.error(errText(e)); } });
    metaCmd.then((d) => alive && setTheirMeta((Array.isArray(d) ? d : []).filter((x) => x.status !== 'local_only'))).catch(() => alive && setTheirMeta([]));
    return () => { alive = false; };
  }, [pane, machine.key]);

  useEffect(() => {
    if (pane !== 'share' || !isLan) return;
    invoke('lan_get_share', { deviceId: machine.id })
      .then((s) => { setShare(s); setShareP(new Set(s?.projects || [])); setShareM(new Set(s?.meta || [])); })
      .catch((e) => toast.error(errText(e)));
  }, [pane, machine.key]);

  const localIds = useMemo(() => new Set((localProjects || []).map((p) => p.id)), [localProjects]);
  const myProjectItems = (localProjects || []).map((p) => ({ id: p.id, label: p.name, sub: p.root_path }));
  const myMetaItems = (localMeta || []).map((m) => ({ id: m.key, label: m.name, sub: META_LABELS[m.category] || m.category }));
  const theirProjectItems = (theirProjects || []).map((p) => ({
    id: p.id, label: p.name, sub: p.root_path,
    badge: localIds.has(p.id) ? 'replaces yours' : 'new', badgeTone: localIds.has(p.id) ? 'text-amber-500' : 'text-emerald-500',
  }));
  const theirMetaItems = (theirMeta || []).map((m) => ({
    id: m.key, label: m.name, sub: META_LABELS[m.category] || m.category,
    badge: m.status === 'new' ? 'new' : m.status === 'conflict' ? 'differs' : 'same',
    badgeTone: m.status === 'new' ? 'text-emerald-500' : m.status === 'conflict' ? 'text-amber-500' : 'text-muted-foreground',
  }));

  const run = async (direction) => {
    const projectSource = direction === 'push' ? (localProjects || []) : (theirProjects || []);
    const metaSource = direction === 'push' ? (localMeta || []) : (theirMeta || []);
    const projects = projectSource.filter((p) => selP.has(p.id)).map((p) => ({ id: p.id, name: p.name }));
    const meta = metaSource.filter((m) => selM.has(m.key)).map((m) => ({ key: m.key, category: m.category, name: m.name }));
    if (!projects.length && !meta.length) return;
    if (direction === 'pull') {
      for (const p of projects) {
        if (!localIds.has(p.id)) {
          p.targetParent = await chooseFolder(p.name);
          if (!p.targetParent) { toast.error(`Skipped ${p.name}: no folder chosen`); p.skip = true; }
        }
      }
    }
    const todo = projects.filter((p) => !p.skip);
    setRunning(isLan ? 'approval' : direction);
    onRunning(true);
    try {
      let ok = 0; const failures = []; let metaRes = null;
      if (isLan) {
        const res = await invoke('lan_sync_items', { deviceId: machine.id, direction, projects: todo, meta });
        ok = res?.projectsOk ?? 0; failures.push(...(res?.failures || [])); metaRes = res?.meta;
      } else {
        for (const p of todo) {
          try {
            await invoke(direction === 'push' ? 'cloud_sync_push_project' : 'cloud_sync_pull_project', {
              url: machine.url, projectId: p.id, ...(p.targetParent ? { targetParent: p.targetParent } : {}),
            });
            ok += 1;
          } catch (e) { failures.push(`${p.name}: ${errText(e)}`); }
        }
        if (meta.length) {
          try {
            metaRes = await invoke('cloud_meta_apply', { url: machine.url, direction, overwrite: [], only: meta.map((m) => m.key) });
          } catch (e) { failures.push(`metadata: ${errText(e)}`); }
        }
      }
      const parts = [];
      if (todo.length) parts.push(`${ok}/${todo.length} project${todo.length === 1 ? '' : 's'}`);
      if (metaRes) parts.push(`${(metaRes.added || 0) + (metaRes.replaced || 0)} metadata item(s)`);
      const verb = direction === 'push' ? 'Pushed' : 'Pulled';
      if (failures.length) toast.error(`${verb} ${parts.join(' and ') || 'nothing'} — ${failures.length} failed: ${failures[0]}`, { duration: 9000 });
      else toast.success(`${verb} ${parts.join(' and ')} ${direction === 'push' ? 'to' : 'from'} ${machine.name}`);
      if (direction === 'pull' && metaRes) window.dispatchEvent(new CustomEvent('rustic:extensions-changed'));
      setSelP(new Set()); setSelM(new Set());
    } catch (e) {
      toast.error(errText(e), { duration: 9000 });
    } finally {
      setRunning(null);
      onRunning(false);
    }
  };

  const saveShare = async () => {
    try {
      await invoke('lan_set_share', { deviceId: machine.id, share: { projects: [...shareP], meta: [...shareM] } });
      setShare({ projects: [...shareP], meta: [...shareM] });
      toast.success(`Sharing with ${machine.name} updated`);
    } catch (e) { toast.error(errText(e)); }
  };
  const shareDirty = share && (JSON.stringify([...shareP].sort()) !== JSON.stringify([...(share.projects || [])].sort())
    || JSON.stringify([...shareM].sort()) !== JSON.stringify([...(share.meta || [])].sort()));

  const count = selP.size + selM.size;
  const options = [
    { id: 'pull', label: 'Pull', icon: CloudDownload },
    { id: 'push', label: 'Push', icon: CloudUpload },
    ...(isLan ? [{ id: 'share', label: 'Sharing', icon: ShieldCheck }] : []),
  ];

  return (
    <div className="space-y-3 border-t border-border/40 bg-background/40 px-3 py-3">
      <Slider value={pane} onChange={setPane} options={options} />
      {pane === 'share' ? (
        <>
          <p className="text-[11px] text-muted-foreground">
            Choose what {machine.name} can see and pull from you. Nothing is shared until you tick it, and every pull still asks you first.
          </p>
          <div className="grid grid-cols-2 gap-3">
            <ItemPicker title="Projects" items={myProjectItems} selected={shareP} onChange={setShareP} loading={!share} empty="No projects open." />
            <ItemPicker title="Metadata" items={myMetaItems} selected={shareM} onChange={setShareM} loading={!share || !localMeta} empty="No metadata." />
          </div>
          <div className="flex justify-end">
            <Button size="sm" className="h-7 text-xs" disabled={!shareDirty} onClick={saveShare}>Save sharing</Button>
          </div>
        </>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3">
            <ItemPicker
              title="Projects"
              items={pane === 'push' ? myProjectItems : theirProjectItems}
              selected={selP}
              onChange={setSelP}
              loading={pane === 'pull' && theirProjects === null}
              empty={pane === 'push' ? 'No projects open.' : isLan ? `${machine.name} hasn't shared any projects with you.` : 'No projects there.'}
            />
            <ItemPicker
              title="Metadata"
              items={pane === 'push' ? myMetaItems : theirMetaItems}
              selected={selM}
              onChange={setSelM}
              loading={pane === 'pull' ? theirMeta === null : !localMeta}
              empty={pane === 'push' ? 'No metadata.' : isLan ? `${machine.name} hasn't shared any metadata with you.` : 'No metadata there.'}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="text-[11px] text-muted-foreground">
              {running === 'approval'
                ? `Waiting for ${machine.name} to approve…`
                : pane === 'push'
                  ? `Received projects replace ${machine.name}'s copy; selected metadata overwrites same-name items.${isLan ? ` ${machine.name} must accept first.` : ''}`
                  : `Pulled projects replace your copy; selected metadata overwrites same-name items.${isLan ? ` ${machine.name} must approve first.` : ''}`}
            </span>
            <Button size="sm" className="h-7 shrink-0 text-xs" disabled={!count || !!running} onClick={() => run(pane)}>
              {running ? <Loader2 className="size-3 animate-spin" /> : pane === 'push' ? <CloudUpload className="size-3" /> : <CloudDownload className="size-3" />}
              {pane === 'push' ? 'Push' : 'Pull'}{count ? ` ${count}` : ''}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

/** Settings › Cloud & Sync. */
export function CloudSettings() {
  const projects = useExplorer((s) => s.projects);
  const [savedBackends, setBackends] = useSavedBackends();
  // The server IS the backend — it syncs with machines, it doesn't save backends.
  const backends = IS_WEB ? [] : savedBackends;
  const [backendOnline, setBackendOnline] = useState({});
  const [openOrigins, setOpenOrigins] = useState([]);
  const [lanStatus, setLanStatus] = useState(null);
  const [lanBusy, setLanBusy] = useState(false);
  const [devices, setDevices] = useState([]);
  const [localMeta, setLocalMeta] = useState(null);
  const [pairing, setPairing] = useState(null);
  const [tab, setTab] = useState('sync');
  const [query, setQuery] = useState('');
  const [add, setAdd] = useState(null);
  const [expanded, setExpanded] = useState(null);
  const [renaming, setRenaming] = useState(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState(null);

  const lanEnabled = !!lanStatus?.enabled;
  const refreshStatus = useCallback(() => invoke('lan_status').then(setLanStatus).catch(() => {}), []);
  const refreshDevices = useCallback(() => invoke('lan_devices').then((l) => setDevices(Array.isArray(l) ? l : [])).catch(() => {}), []);

  useEffect(() => { refreshStatus(); }, [refreshStatus]);
  useEffect(() => { invoke('lan_local_meta').then(setLocalMeta).catch(() => setLocalMeta([])); }, []);
  useEffect(() => {
    if (!lanEnabled) { setDevices([]); return undefined; }
    refreshDevices();
    const t = setInterval(refreshDevices, 3000);
    return () => clearInterval(t);
  }, [lanEnabled, refreshDevices]);
  useEffect(() => {
    if (IS_WEB) return undefined;
    const load = () => invoke('remote_backend_open_urls').then((u) => setOpenOrigins(Array.isArray(u) ? u : [])).catch(() => {});
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);
  const backendUrls = backends.map((b) => b.url).join('\n');
  useEffect(() => {
    let alive = true;
    const ping = () => backends.forEach((b) => {
      invoke('cloud_backend_ping', { url: b.url })
        .then(() => alive && setBackendOnline((s) => ({ ...s, [b.url]: true })))
        .catch(() => alive && setBackendOnline((s) => ({ ...s, [b.url]: false })));
    });
    ping();
    const t = setInterval(ping, 30000);
    return () => { alive = false; clearInterval(t); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backendUrls]);
  useEffect(() => {
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:sync-progress', (e) => setProgress(e.payload)))
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, []);
  useEffect(() => {
    if (busy || !progress || progress.phase !== 'done') return undefined;
    const t = setTimeout(() => setProgress(null), 3000);
    return () => clearTimeout(t);
  }, [busy, progress]);

  const machines = useMemo(() => [
    ...devices.map((d) => ({
      key: `lan:${d.device_id}`, kind: 'lan', id: d.device_id, name: d.name, nickname: d.nickname,
      subtitle: d.addr, online: d.online, paired: d.paired, manual: d.manual,
      viaInternet: typeof d.addr === 'string' && /^https?:\/\//.test(d.addr),
    })),
    ...backends.map((b) => ({
      key: `remote:${b.id}`, kind: 'remote', id: b.id, name: b.name, subtitle: b.url, url: b.url,
      online: backendOnline[b.url] ?? null, windowOpen: openOrigins.includes(originOf(b.url)),
    })),
  ], [devices, backends, backendOnline, openOrigins]);

  const q = query.trim().toLowerCase();
  const visible = machines
    .filter((m) => (tab === 'backends' ? m.kind === 'remote' : true))
    .filter((m) => !q || `${m.name} ${m.subtitle || ''}`.toLowerCase().includes(q));
  const syncCount = machines.length;
  const backendCount = machines.filter((m) => m.kind === 'remote').length;

  const toggleLan = async (v) => {
    setLanBusy(true);
    try { await invoke('lan_set_enabled', { enabled: v }); await refreshStatus(); }
    catch (e) { toast.error(errText(e)); }
    finally { setLanBusy(false); }
  };
  const renameThisMachine = async (name) => {
    try { await invoke('lan_set_device_name', { name }); await refreshStatus(); toast.success('Name saved'); }
    catch (e) { toast.error(errText(e)); }
  };
  const pair = async (m) => {
    try {
      const code = await invoke('lan_pair_code', { deviceId: m.id });
      setPairing({ name: m.name, code });
      const name = await invoke('lan_pair', { deviceId: m.id });
      toast.success(`Paired with ${name}. Choose what to share with it under Sharing.`);
      await refreshDevices();
      setExpanded(m.key);
    } catch (e) { toast.error(errText(e)); }
    finally { setPairing(null); }
  };
  const rename = async (m, name) => {
    setRenaming(null);
    if (m.kind === 'remote') { setBackends((l) => l.map((b) => (b.id === m.id ? { ...b, name: name || b.name } : b))); return; }
    try { await invoke('lan_rename', { deviceId: m.id, nickname: name }); await refreshDevices(); }
    catch (e) { toast.error(errText(e)); }
  };
  const remove = async (m) => {
    if (expanded === m.key) setExpanded(null);
    try {
      if (m.kind === 'remote') {
        await invoke('remote_backend_close', { url: m.url }).catch(() => {});
        await invoke('cloud_sync_remember', { password: '', url: m.url }).catch(() => {});
        setBackends((l) => l.filter((b) => b.id !== m.id));
      } else {
        await invoke('lan_forget', { deviceId: m.id });
        await refreshDevices();
      }
      toast.success(`Removed ${m.name}`);
    } catch (e) { toast.error(errText(e)); }
  };
  const openWindow = async (m) => {
    try {
      await invoke('remote_backend_open', { url: m.url, name: m.name });
      setOpenOrigins((o) => [...new Set([...o, originOf(m.url)])]);
    } catch (e) { toast.error(errText(e)); }
  };
  const closeWindow = async (m) => {
    try { await invoke('remote_backend_close', { url: m.url }); setOpenOrigins((o) => o.filter((x) => x !== originOf(m.url))); }
    catch (e) { toast.error(errText(e)); }
  };

  const rowActions = (m) => (
    <>
      {m.kind === 'lan' && !m.paired && m.online && (
        <Button size="sm" variant="outline" className="h-7 text-[11px]" onClick={() => pair(m)}><Link2 className="size-3" /> Pair</Button>
      )}
      {m.kind === 'remote' && (m.windowOpen ? (
        <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Close its window" onClick={() => closeWindow(m)}><LogOut className="size-3.5" /></Button>
      ) : (
        <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Open in a new window" onClick={() => openWindow(m)}><ExternalLink className="size-3.5" /></Button>
      ))}
      {(m.kind === 'remote' || m.paired) && (
        <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Rename" onClick={() => setRenaming(m.key)}><Pencil className="size-3.5" /></Button>
      )}
      {(m.kind === 'remote' || m.paired) && (
        <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-destructive" title={m.kind === 'remote' ? 'Remove' : 'Forget (must pair again)'} onClick={() => remove(m)}><Trash2 className="size-3.5" /></Button>
      )}
    </>
  );

  const onRowClick = (m) => {
    if (tab === 'backends') { openWindow(m); return; }
    const usable = m.online && (m.kind === 'remote' || m.paired);
    if (!usable) {
      if (m.kind === 'lan' && !m.paired) toast.info(`Pair with ${m.name} first`);
      else toast.info(`${m.name} is offline`);
      return;
    }
    setExpanded((k) => (k === m.key ? null : m.key));
  };

  return (
    <>
      <MyMachine status={lanStatus} busy={lanBusy} onToggleLan={toggleLan} onRename={renameThisMachine} onRefresh={refreshStatus} />

      <section data-settings-anchor="machines">
        {!IS_WEB && (
        <Slider
          className="mb-3"
          value={tab}
          onChange={(v) => { setTab(v); setExpanded(null); }}
          options={[
            { id: 'sync', label: 'Sync', icon: CloudDownload, count: syncCount },
            { id: 'backends', label: 'Backends', icon: Globe, count: backendCount },
          ]}
        />
        )}
        <div className="mb-2 flex items-center gap-2">
          <SearchBox className="flex-1" value={query} onChange={setQuery} placeholder={tab === 'sync' ? 'Search machines…' : 'Search backends…'} />
          <Button size="sm" className="h-8 text-xs" onClick={() => setAdd(tab === 'sync' ? 'sync' : 'backend')}>
            <Plus className="size-3.5" /> Add machine
          </Button>
        </div>
        <p className={cn(GROUP_TITLE, 'mb-2 px-1 normal-case tracking-normal font-normal')}>
          {tab === 'sync'
            ? IS_WEB
              ? 'Click a machine to push or pull projects and metadata. Add desktops by their address or tunnel URL, or other servers by URL.'
              : 'Click a machine to push or pull projects and metadata. Backends are tinted — you can sync with them too.'
            : 'Click a backend to open it in its own window and work on it there. Drag files between its window and your desktop.'}
        </p>
        <div className={cn(GROUP_BOX, 'divide-y divide-border/40')}>
          {visible.length === 0 && (
            <div className="flex items-center gap-2 px-3 py-3 text-[12px] text-muted-foreground">
              {q ? 'No matches.' : tab === 'backends'
                ? 'No backends yet — Add machine to save a remote rustic-server.'
                : lanEnabled
                  ? <><Loader2 className="size-3 animate-spin" /> Looking for desktops on this network — or Add machine.</>
                  : 'No machines yet. Turn on local network sync under My machine, or Add machine.'}
            </div>
          )}
          {visible.map((m) => (
            <MachineCard
              key={m.key}
              machine={renaming === m.key ? { ...m, name: '' } : m}
              selected={expanded === m.key}
              onClick={() => renaming !== m.key && onRowClick(m)}
              actions={renaming === m.key ? (
                <InlineRename value={m.nickname ?? m.name} placeholder="Name" onCancel={() => setRenaming(null)} onSave={(n) => rename(m, n)} />
              ) : rowActions(m)}
            >
              {tab === 'sync' && expanded === m.key && (
                <MachineView machine={m} localProjects={projects} localMeta={localMeta} onRunning={setBusy} />
              )}
            </MachineCard>
          ))}
        </div>
        {progress && busy && <SyncProgressRow progress={progress} />}
      </section>

      <AddMachineDialog
        open={!!add}
        mode={add === 'backend' ? 'backend' : 'sync'}
        onClose={() => setAdd(null)}
        lanEnabled={lanEnabled}
        onLanAdded={(d) => { refreshDevices(); toast.success(d.paired ? `${d.name} is reachable again` : `Found ${d.name} — pair it to sync`); }}
        onBackendAdded={(b) => {
          setBackends((l) => [...l.filter((x) => x.url !== b.url), b]);
          setBackendOnline((s) => ({ ...s, [b.url]: true }));
          toast.success(`Saved ${b.name}`);
        }}
      />

      <Dialog open={!!pairing} onOpenChange={() => {}}>
        <DialogContent className="max-w-sm" showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Pairing with {pairing?.name}</DialogTitle>
            <DialogDescription>Accept the request on {pairing?.name}. Check that it shows the same code before accepting.</DialogDescription>
          </DialogHeader>
          <div className="py-2 text-center font-mono text-3xl tracking-[0.35em]">{pairing?.code}</div>
          <div className="flex items-center justify-center gap-2 text-[12px] text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" /> Waiting for the other device…
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}

/** Live sync progress: phase, current item and a bar when the total is known. */
function SyncProgressRow({ progress }) {
  const { direction, phase, detail, done = 0, total = 0 } = progress || {};
  const label = PHASE_LABELS[phase] || phase;
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : null;
  const finished = phase === 'done';
  return (
    <div className={cn(GROUP_BOX, 'mt-2 space-y-1.5 px-3 py-2.5')}>
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="flex items-center gap-1.5 font-medium text-foreground">
          {finished ? <CloudUpload className="size-3 text-emerald-500" /> : <Loader2 className="size-3 animate-spin text-muted-foreground" />}
          {direction === 'pull' ? 'Pull' : 'Push'} — {label}
        </span>
        {pct !== null && !finished && <span className="tabular-nums text-muted-foreground">{pct}%</span>}
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className={pct === null && !finished ? 'h-full w-1/3 animate-pulse rounded-full bg-primary' : 'h-full rounded-full bg-primary transition-[width] duration-200'}
          style={pct === null && !finished ? undefined : { width: `${finished ? 100 : pct}%` }}
        />
      </div>
      {detail && <div className="truncate text-[11px] text-muted-foreground" title={detail}>{detail}</div>}
    </div>
  );
}

export default CloudSettings;
