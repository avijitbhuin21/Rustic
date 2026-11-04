// Cloud & Sync overlay, opened from the left island:
// |machines|projects·metadata·sharing|files|preview (optional)|my explorer|
// The Backends tab swaps the columns for one panel of machine cards.
import React, { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Plus, Link2, Pencil, Trash2, Monitor, Globe, CloudDownload, ExternalLink, LogOut, Loader2,
  AlertTriangle, RefreshCw, Server, Search, X,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';
import { useLayout } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import {
  useSavedBackends, InlineRename, Slider, StatusPill, AddMachineDialog,
} from '@/components/settings/cloud-machines';
import { useLan, errText } from './use-lan';
import { useConflictPrompt } from './conflict-dialog';
import { Column, MachineColumn, RemoteFiles, PreviewColumn } from './remote-panels';
import { LocalExplorer } from './local-explorer';
import { MyMachineDialog } from './my-machine-dialog';
import { TransfersTray } from '@/components/transfers/transfers-tray';

const baseName = (p) => String(p).replace(/[\\/]+$/, '').split(/[\\/]/).pop();

/** One device card in the machines grid. */
function DeviceRow({ d, selected, onClick, onPair, onRename, onForget, renaming, setRenaming }) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => e.key === 'Enter' && onClick()}
      className={cn(
        'group flex min-h-[112px] cursor-pointer flex-col gap-2 rounded-xl border border-ink/[0.08] bg-ink/[0.03] p-3 transition-colors hover:bg-ink/[0.06]',
        selected && 'border-primary/50 bg-primary/10',
      )}
    >
      <div className="flex items-start gap-2.5">
        <span className={cn('flex size-9 shrink-0 items-center justify-center rounded-lg', d.paired ? 'bg-primary/15 text-primary' : 'bg-ink/[0.06] text-muted-foreground')}>
          <Monitor className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          {renaming ? (
            <div onClick={(e) => e.stopPropagation()} role="presentation">
              <InlineRename value={d.nickname ?? d.name} placeholder="Name" onCancel={() => setRenaming(null)} onSave={(n) => onRename(d, n)} />
            </div>
          ) : (
            <>
              <div className="truncate text-[13px] font-medium" title={d.name}>{d.name}</div>
              <div className="truncate font-mono text-[10.5px] text-muted-foreground" title={d.addr || ''}>{d.addr || '—'}</div>
            </>
          )}
        </div>
        {!renaming && <StatusPill online={d.online} />}
      </div>

      <div className="flex flex-wrap items-center gap-1.5 text-[10.5px] text-muted-foreground">
        <span className="rounded-full border border-ink/[0.1] px-1.5 py-px">{d.paired ? 'Paired' : 'Not paired'}</span>
        {d.version && <span className="rounded-full border border-ink/[0.1] px-1.5 py-px font-mono">v{d.version}</span>}
        {d.manual && <span className="rounded-full border border-ink/[0.1] px-1.5 py-px">Added by address</span>}
      </div>

      {d.update_required && (
        <div className="flex items-start gap-1.5 rounded-md bg-warning/10 px-2 py-1 text-[10.5px] text-warning">
          <AlertTriangle className="mt-px size-3 shrink-0" />
          <span>Update required — {d.version ? `it runs v${d.version}` : 'it runs an older version'}. Both must match.</span>
        </div>
      )}
      {d.needs_repair && !d.update_required && (
        <div className="flex items-start gap-1.5 rounded-md bg-warning/10 px-2 py-1 text-[10.5px] text-warning">
          <AlertTriangle className="mt-px size-3 shrink-0" /> <span>It forgot this machine — pair again.</span>
        </div>
      )}

      {!renaming && (
        <div className="mt-auto flex items-center gap-1" onClick={(e) => e.stopPropagation()} role="presentation">
          {(!d.paired || d.needs_repair) && d.online && !d.update_required && (
            <Button size="sm" variant="outline" className="h-6 text-[11px]" onClick={() => onPair(d)}><Link2 className="size-3" /> {d.paired ? 'Pair again' : 'Pair'}</Button>
          )}
          {d.paired && (
            <div className="touch-reveal ml-auto flex items-center opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
              <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground" title="Rename" onClick={() => setRenaming(d.device_id)}><Pencil className="size-3" /></Button>
              <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" title="Forget (both sides)" onClick={() => onForget(d)}><Trash2 className="size-3" /></Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Backends tab: saved rustic-server cards, rendered inside the machines column. */
function BackendsPanel({ query = '' }) {
  const [backends, setBackends] = useSavedBackends();
  const [online, setOnline] = useState({});
  const [openOrigins, setOpenOrigins] = useState([]);
  const origin = (u) => { try { return new URL(u).origin; } catch { return u; } };
  const urls = backends.map((b) => b.url).join('\n');
  useEffect(() => {
    let alive = true;
    const ping = () => backends.forEach((b) => {
      invoke('cloud_backend_ping', { url: b.url })
        .then(() => alive && setOnline((s) => ({ ...s, [b.url]: true })))
        .catch(() => alive && setOnline((s) => ({ ...s, [b.url]: false })));
    });
    const windows = () => invoke('remote_backend_open_urls').then((u) => alive && setOpenOrigins(Array.isArray(u) ? u : [])).catch(() => {});
    ping(); windows();
    const t1 = setInterval(() => { if (!document.hidden) ping(); }, 20000);
    const t2 = setInterval(() => { if (!document.hidden) windows(); }, 3000);
    return () => { alive = false; clearInterval(t1); clearInterval(t2); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urls]);
  const openWin = (b) => invoke('remote_backend_open', { url: b.url, name: b.name }).catch((e) => toast.error(errText(e)));
  const closeWin = (b) => invoke('remote_backend_close', { url: b.url }).catch((e) => toast.error(errText(e)));
  const remove = async (b) => {
    await invoke('remote_backend_close', { url: b.url }).catch(() => {});
    await invoke('cloud_sync_remember', { password: '', url: b.url }).catch(() => {});
    setBackends((l) => l.filter((x) => x.id !== b.id));
  };
  const shown = backends.filter((b) => !query || `${b.name} ${b.url}`.toLowerCase().includes(query));
  return (
    <div>
      {backends.length === 0 ? (
        <p className="px-2 py-3 text-[11.5px] text-muted-foreground">No backends yet — use + to save a remote rustic-server.</p>
      ) : shown.length === 0 ? (
        <p className="px-2 py-3 text-[11.5px] text-muted-foreground">No matches.</p>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-2">
          {shown.map((b) => {
            const isOpen = openOrigins.includes(origin(b.url));
            return (
              <div key={b.id} className="group flex flex-col gap-2 rounded-xl border border-ink/[0.08] bg-ink/[0.03] p-3 transition-colors hover:bg-ink/[0.06]">
                <div className="flex items-center gap-2">
                  <span className="flex size-8 items-center justify-center rounded-lg bg-primary/15 text-primary"><Server className="size-4" /></span>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px] font-medium">{b.name}</div>
                    <div className="truncate text-[10.5px] text-muted-foreground" title={b.url}>{b.url}</div>
                  </div>
                  <StatusPill online={online[b.url] ?? null} />
                </div>
                <div className="flex items-center gap-1.5">
                  {isOpen ? (
                    <Button size="sm" variant="outline" className="h-7 flex-1 text-xs" onClick={() => closeWin(b)}><LogOut className="size-3" /> Close window</Button>
                  ) : (
                    <Button size="sm" className="h-7 flex-1 text-xs" onClick={() => openWin(b)}><ExternalLink className="size-3" /> Open</Button>
                  )}
                  <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-destructive" title="Remove" onClick={() => remove(b)}><Trash2 className="size-3.5" /></Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** Cloud & Sync sidebar panel. Widens the sidebar while a machine is open. */
export function SyncOverlay() {
  const active = useLayout((s) => s.activeSidebarPanel === 'sync' && s.sidebarVisible);
  const localProjects = useExplorer((s) => s.projects);
  const lan = useLan();
  const conflicts = useConflictPrompt();
  const [tab, setTab] = useState('sync');
  const [query, setQuery] = useState('');
  const [add, setAdd] = useState(null);
  const [renaming, setRenaming] = useState(null);
  const [deviceId, setDeviceId] = useState(null);
  const [subTab, setSubTab] = useState('projects');
  const [project, setProject] = useState(null);
  const [file, setFile] = useState(null);
  const [metaItem, setMetaItem] = useState(null);
  const [metaSel, setMetaSel] = useState(new Set());
  const [localMeta, setLocalMeta] = useState(null);
  const [searchOpen, setSearchOpen] = useState(IS_WEB);
  const [myMachineOpen, setMyMachineOpen] = useState(false);
  const searchRef = useRef(null);
  const closeSearch = () => { if (IS_WEB) return; setQuery(''); setSearchOpen(false); };
  useEffect(() => { if (searchOpen && !IS_WEB) searchRef.current?.focus(); }, [searchOpen]);

  useEffect(() => {
    if (!active) return;
    invoke('lan_local_meta').then(setLocalMeta).catch(() => setLocalMeta([]));
    invoke('lan_announce').catch(() => {});
  }, [active]);

  const device = lan.devices.find((d) => d.device_id === deviceId) || null;
  const usable = device && device.paired && device.online && !device.update_required && !device.needs_repair;
  useEffect(() => { setProject(null); setFile(null); setMetaItem(null); setMetaSel(new Set()); }, [deviceId]);
  useEffect(() => { setFile(null); setMetaItem(null); }, [subTab, project?.id]);

  const q = query.trim().toLowerCase();
  const visible = useMemo(
    () => lan.devices.filter((d) => !q || `${d.name} ${d.addr || ''}`.toLowerCase().includes(q)),
    [lan.devices, q],
  );

  const selectDevice = (d) => {
    if (!d.paired) { toast.info(`Pair with ${d.name} first`); return; }
    if (d.update_required) { toast.error(`${d.name} runs a different Rustic version (${d.version ? `v${d.version}` : 'older'}). Update both to the same version.`); return; }
    if (d.needs_repair) { toast.info(`${d.name} forgot this machine — pair again`); return; }
    if (!d.online) { toast.info(`${d.name} is offline`); return; }
    setDeviceId((cur) => (cur === d.device_id ? null : d.device_id));
  };
  const renameDevice = async (d, name) => {
    setRenaming(null);
    try { await invoke('lan_rename', { deviceId: d.device_id, nickname: name }); await lan.refreshDevices(); }
    catch (e) { toast.error(errText(e)); }
  };
  const forget = async (d) => {
    if (deviceId === d.device_id) setDeviceId(null);
    try { await invoke('lan_forget', { deviceId: d.device_id }); await lan.refreshDevices(); toast.success(`Forgot ${d.name} (on both machines)`); }
    catch (e) { toast.error(errText(e)); }
  };

  /** Pull `items` from `fromDevice` into local `destDir`, asking about name clashes. */
  const pullTo = async (fromDevice, items, destDir) => {
    if (!items?.length || !destDir) return;
    let opts = {};
    try {
      const clash = await invoke('lan_local_conflicts', { destDir, items });
      if (clash.length) {
        const r = await conflicts.ask(clash, destDir);
        if (!r) return;
        opts = r;
      }
    } catch { /* conflict check is best effort; backend still auto-renames */ }
    const name = lan.devices.find((d) => d.device_id === fromDevice)?.name || 'the other machine';
    toast.info(`Pulling from ${name}… — track it in Transfers`);
    try {
      const s = await invoke('lan_pull_files', { deviceId: fromDevice, items, destDir, opts });
      const renamed = Object.keys(s?.renamed || {}).length;
      toast.success(`Imported ${s?.files ?? 0} file${s?.files === 1 ? '' : 's'}${renamed ? ` (${renamed} renamed)` : ''}`);
    } catch (e) {
      const m = errText(e);
      if (m !== 'Cancelled' && m !== 'Request cancelled') toast.error(m, { duration: 9000 });
    }
  };

  /** Send local `paths` into `dir` of the open remote project. */
  const sendTo = async (paths, dir) => {
    if (!usable || !project || !paths?.length) return;
    let opts = {};
    try {
      const clash = await invoke('lan_remote_conflicts', { deviceId: device.device_id, projectId: project.id, dir, names: paths.map(baseName) });
      if (clash.length) {
        const r = await conflicts.ask(clash, `${device.name}: ${project.name}/${dir}`);
        if (!r) return;
        opts = r;
      }
    } catch { /* best effort */ }
    toast.info(`Sending to ${device.name}… — track it in Transfers`);
    try {
      await invoke('lan_push_files', { deviceId: device.device_id, localPaths: paths, projectId: project.id, dir, opts });
      toast.success(`Sent ${paths.length} item${paths.length === 1 ? '' : 's'} to ${device.name}`);
    } catch (e) {
      const m = errText(e);
      if (m !== 'Cancelled' && m !== 'Request cancelled') toast.error(m, { duration: 9000 });
    }
  };

  const importMeta = async (items) => {
    try {
      const res = await invoke('lan_sync_items', {
        deviceId: device.device_id, direction: 'pull', projects: [],
        meta: items.map((m) => ({ key: `${m.category}/${m.name}`, category: m.category, name: m.name })),
      });
      if (res?.failures?.length) toast.error(res.failures[0]);
      else toast.success(`Imported ${items.length} metadata item${items.length === 1 ? '' : 's'}`);
      window.dispatchEvent(new CustomEvent('rustic:extensions-changed'));
      setMetaSel(new Set());
    } catch (e) {
      const m = errText(e);
      if (m !== 'Request cancelled') toast.error(m);
    }
  };

  const showBackends = tab === 'backends' && !IS_WEB;
  const [refreshing, setRefreshing] = useState(false);
  const refreshAll = async () => {
    setRefreshing(true);
    const minSpin = new Promise((r) => setTimeout(r, 600));
    try {
      await Promise.allSettled([lan.refreshStatus(), lan.refreshDevices(), invoke('lan_announce'), minSpin]);
    } finally {
      setRefreshing(false);
    }
  };

  const searchRow = (
    <div className="flex items-center gap-1.5">
      {!IS_WEB && (
        <Slider
          compact={searchOpen}
          className={cn('min-w-0 transition-[flex-grow] duration-200 ease-out', searchOpen ? 'grow-0' : 'grow')}
          value={tab}
          onChange={(v) => { setTab(v); setDeviceId(null); }}
          options={[
            { id: 'sync', label: 'Sync', icon: CloudDownload, count: lan.devices.length },
            { id: 'backends', label: 'Backends', icon: Globe },
          ]}
        />
      )}
      <div className={cn('relative flex min-w-0 items-center transition-[flex-grow] duration-200 ease-out', searchOpen ? 'grow' : 'grow-0')}>
        {searchOpen ? (
          <>
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === 'Escape' && closeSearch()}
              placeholder={tab === 'backends' ? 'Search backends…' : 'Search machines…'}
              className="h-8 pl-8 pr-7 text-[12px]"
            />
            {!IS_WEB && (
              <button
                type="button"
                title="Close search"
                onClick={closeSearch}
                className="absolute right-1.5 top-1/2 flex size-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:text-foreground"
              >
                <X className="size-3" />
              </button>
            )}
          </>
        ) : (
          <Button size="icon-sm" variant="ghost" className="size-8 shrink-0 text-muted-foreground" title="Search" onClick={() => setSearchOpen(true)}>
            <Search className="size-3.5" />
          </Button>
        )}
      </div>
    </div>
  );

  const machineList = (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-2">
      {visible.length === 0 && (
        <div className="col-span-full flex items-center gap-2 px-2 py-3 text-[11.5px] text-muted-foreground">
          {q ? 'No matches.' : lan.enabled
            ? <><Loader2 className="size-3 animate-spin" /> Looking for machines — or add one with +.</>
            : 'Turn on Discoverable in the header, or add a machine with +.'}
        </div>
      )}
      {visible.map((d) => (
        <DeviceRow
          key={d.device_id}
          d={d}
          selected={deviceId === d.device_id}
          onClick={() => selectDevice(d)}
          onPair={async (x) => { if (await lan.pair(x)) setDeviceId(x.device_id); }}
          onRename={renameDevice}
          onForget={forget}
          renaming={renaming === d.device_id}
          setRenaming={setRenaming}
        />
      ))}
    </div>
  );

  const cols = !showBackends && usable;
  const showFiles = cols && subTab === 'projects' && project;
  const showPreview = cols && ((subTab === 'projects' && file && project) || (subTab === 'meta' && metaItem));
  // Default widths per open-column combination (machine · files · preview · my machine).
  const layoutKey = !cols ? 'list' : `cols-${showFiles ? 'f' : ''}${showPreview ? 'p' : ''}`;
  const sizes = {
    'cols-': { machine: '25%', local: '75%' },
    'cols-f': { machine: '18%', files: '42%', local: '40%' },
    'cols-fp': { machine: '15%', files: '16%', preview: '50%', local: '19%' },
    'cols-p': { machine: '18%', preview: '60%', local: '22%' },
  }[layoutKey] || {};

  return (
    <>
      <ResizablePanelGroup key={layoutKey} orientation="horizontal" className="overflow-hidden bg-sidebar">
        {!cols && (
        <ResizablePanel id="sync-machines" defaultSize="100%" minSize="220px">
          <Column
            className="w-full"
            title="Cloud & Sync"
            actions={(
              <>
                <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title={tab === 'backends' ? 'Add a remote backend' : 'Add a machine by address'} onClick={() => setAdd(tab === 'backends' ? 'backend' : 'sync')}>
                  <Plus className="size-3.5" />
                </Button>
                <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="My machine" onClick={() => setMyMachineOpen(true)}>
                  <Monitor className="size-3.5" />
                </Button>
                <span className="flex items-center px-0.5" title={lan.enabled ? 'Discoverable — other machines can find this one' : 'Not discoverable — turn on to let other machines find this one'}>
                  <Switch checked={lan.enabled} disabled={lan.busy} onCheckedChange={lan.toggle} aria-label="Discoverable" />
                </span>
                <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Refresh" disabled={refreshing} onClick={refreshAll}><RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} /></Button>
                <TransfersTray alwaysVisible />
              </>
            )}
          >
            <div className="space-y-2 p-2">
              {searchRow}
              {tab === 'backends' && !IS_WEB && <BackendsPanel query={q} />}
              {tab === 'sync' && machineList}
            </div>
          </Column>
        </ResizablePanel>
        )}

        {cols && (
          <>
            <ResizablePanel id="sync-machine" defaultSize={sizes.machine} minSize="220px">
              <MachineColumn
                device={device}
                onBack={() => setDeviceId(null)}
                tab={subTab}
                setTab={setSubTab}
                project={project}
                onProject={setProject}
                metaItem={metaItem}
                onMetaItem={setMetaItem}
                metaSel={metaSel}
                setMetaSel={setMetaSel}
                localProjects={localProjects}
                localMeta={localMeta}
                onImportMeta={importMeta}
              />
            </ResizablePanel>
          </>
        )}
        {showFiles && (
          <>
            <ResizableHandle />
            <ResizablePanel id="sync-files" defaultSize={sizes.files} minSize="180px">
              <RemoteFiles
                key={`${device.device_id}:${project.id}`}
                device={device}
                project={project}
                previewPath={file?.path}
                onPreview={setFile}
                onDropLocal={sendTo}
              />
            </ResizablePanel>
          </>
        )}
        {showPreview && (
          <>
            <ResizableHandle />
            <ResizablePanel id="sync-preview" defaultSize={sizes.preview} minSize="240px">
              <PreviewColumn
                device={device}
                project={project}
                file={subTab === 'projects' ? file : null}
                metaItem={subTab === 'meta' ? metaItem : null}
                onClose={() => { setFile(null); setMetaItem(null); }}
              />
            </ResizablePanel>
          </>
        )}
        {cols && (
          <>
            <ResizableHandle />
            <ResizablePanel id="sync-local" defaultSize={sizes.local} minSize="200px">
              <LocalExplorer
                onDropRemote={(payload, dest) => pullTo(payload.deviceId, payload.items, dest)}
              />
            </ResizablePanel>
          </>
        )}
      </ResizablePanelGroup>

      <MyMachineDialog
        open={myMachineOpen}
        onOpenChange={setMyMachineOpen}
        status={lan.status}
        onSaved={lan.refreshStatus}
      />

      <AddMachineDialog
        open={!!add}
        mode={add === 'backend' ? 'backend' : 'sync'}
        onClose={() => setAdd(null)}
        lanEnabled={lan.enabled}
        onLanAdded={(d) => {
          lan.refreshDevices();
          if (d.update_required) toast.error(`${d.name} runs a different version (${d.version ? `v${d.version}` : 'older'}) — update both to connect`);
          else toast.success(d.paired ? `${d.name} is reachable again` : `Found ${d.name} — pair it to sync`);
        }}
        onBackendAdded={(b) => toast.success(`Saved ${b.name}`)}
      />

      <Dialog open={!!lan.pairing} onOpenChange={(o) => { if (!o) lan.cancelPairing(); }}>
        <DialogContent className="max-w-sm" showCloseButton={false} data-rustic-protected="">
          <DialogHeader>
            <DialogTitle>Pairing with {lan.pairing?.name}</DialogTitle>
            <DialogDescription>Accept the request on {lan.pairing?.name}. Check that it shows the same code before accepting.</DialogDescription>
          </DialogHeader>
          <div className="py-2 text-center font-mono text-3xl tracking-[0.35em]">{lan.pairing?.code}</div>
          <div className="flex items-center justify-center gap-2 text-[12px] text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" /> Waiting for the other device…
          </div>
          <DialogFooter>
            <Button variant="outline" size="sm" className="h-7 w-full text-xs" onClick={lan.cancelPairing}>Cancel</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {conflicts.dialog}
    </>
  );
}

export default SyncOverlay;
