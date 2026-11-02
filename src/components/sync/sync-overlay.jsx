// Cloud & Sync overlay, opened from the left island:
// |machines|projects·metadata·sharing|files|preview (optional)|my explorer|
// The Backends tab swaps the columns for one panel of machine cards.
import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Plus, Link2, Pencil, Trash2, Monitor, Globe, CloudDownload, ExternalLink, LogOut, Loader2,
  AlertTriangle, RefreshCw, Server, ChevronLeft,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';
import { useLayout } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import { MyMachine } from '@/components/settings/cloud-settings';
import {
  useSavedBackends, InlineRename, Slider, SearchBox, StatusPill, AddMachineDialog,
} from '@/components/settings/cloud-machines';
import { useLan, errText } from './use-lan';
import { useConflictPrompt } from './conflict-dialog';
import { Column, MachineColumn, RemoteFiles, PreviewColumn } from './remote-panels';
import { LocalExplorer } from './local-explorer';

/** Pick a local destination folder. */
async function pickFolder(title) {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ directory: true, multiple: false, title });
    return Array.isArray(r) ? r[0] : r;
  } catch { return null; }
}

/** Pick local files or one folder to upload. */
async function pickUpload(directory) {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ directory, multiple: !directory, title: directory ? 'Choose a folder to send' : 'Choose files to send' });
    if (!r) return [];
    return Array.isArray(r) ? r : [r];
  } catch { return []; }
}

const baseName = (p) => String(p).replace(/[\\/]+$/, '').split(/[\\/]/).pop();

/** One device row in the machines column. */
function DeviceRow({ d, selected, onClick, onPair, onRename, onForget, renaming, setRenaming }) {
  const blocked = d.update_required || d.needs_repair;
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => e.key === 'Enter' && onClick()}
      className={cn(
        'group flex cursor-pointer flex-col gap-1 rounded-lg px-2 py-2 transition-colors hover:bg-ink/[0.05]',
        selected && 'bg-primary/15',
      )}
    >
      <div className="flex items-center gap-2">
        <span className={cn('flex size-7 shrink-0 items-center justify-center rounded-md', d.paired ? 'bg-primary/15 text-primary' : 'bg-ink/[0.06] text-muted-foreground')}>
          <Monitor className="size-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          {renaming ? (
            <InlineRename value={d.nickname ?? d.name} placeholder="Name" onCancel={() => setRenaming(null)} onSave={(n) => onRename(d, n)} />
          ) : (
            <>
              <div className="truncate text-[12.5px] font-medium">{d.name}</div>
              <div className="truncate text-[10.5px] text-muted-foreground" title={d.addr || ''}>{d.addr || '—'}</div>
            </>
          )}
        </div>
        {!renaming && <StatusPill online={d.online} />}
      </div>
      {d.update_required && (
        <div className="flex items-center gap-1.5 rounded-md bg-warning/10 px-2 py-1 text-[10.5px] text-warning">
          <AlertTriangle className="size-3 shrink-0" />
          Update required — {d.version ? `it runs v${d.version}` : 'it runs an older version'}. Both must match.
        </div>
      )}
      {d.needs_repair && !d.update_required && (
        <div className="flex items-center gap-1.5 rounded-md bg-warning/10 px-2 py-1 text-[10.5px] text-warning">
          <AlertTriangle className="size-3 shrink-0" /> It forgot this machine — pair again.
        </div>
      )}
      {!renaming && (
        <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()} role="presentation">
          {(!d.paired || d.needs_repair) && d.online && !d.update_required && (
            <Button size="sm" variant="outline" className="h-6 text-[11px]" onClick={() => onPair(d)}><Link2 className="size-3" /> {d.paired ? 'Pair again' : 'Pair'}</Button>
          )}
          {d.paired && (
            <div className="ml-auto flex items-center opacity-0 transition-opacity group-hover:opacity-100">
              <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground" title="Rename" onClick={() => setRenaming(d.device_id)}><Pencil className="size-3" /></Button>
              <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" title="Forget (both sides)" onClick={() => onForget(d)}><Trash2 className="size-3" /></Button>
            </div>
          )}
        </div>
      )}
      {blocked && null}
    </div>
  );
}

/** Backends tab: saved rustic-server cards, rendered inside the machines column. */
function BackendsPanel() {
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
    const t1 = setInterval(ping, 20000);
    const t2 = setInterval(windows, 3000);
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
  return (
    <div>
      {backends.length === 0 ? (
        <p className="px-2 py-3 text-[11.5px] text-muted-foreground">No backends yet — use Add machine to save a remote rustic-server.</p>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-2">
          {backends.map((b) => {
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
  const setSyncWide = useLayout((s) => s.setSyncWide);
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
    toast.info(`Asked ${name} to approve — track it in Transfers (top right)`);
    try {
      const s = await invoke('lan_pull_files', { deviceId: fromDevice, items, destDir, opts });
      const renamed = Object.keys(s?.renamed || {}).length;
      toast.success(`Imported ${s?.files ?? 0} file${s?.files === 1 ? '' : 's'}${renamed ? ` (${renamed} renamed)` : ''}`);
    } catch (e) {
      const m = errText(e);
      if (m !== 'Cancelled' && m !== 'Request cancelled') toast.error(m, { duration: 9000 });
    }
  };
  const importSelected = async (items) => {
    const dest = await pickFolder('Import to…');
    if (dest) pullTo(device.device_id, items, dest);
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
    toast.info(`Asked ${device.name} to accept — track it in Transfers (top right)`);
    try {
      await invoke('lan_push_files', { deviceId: device.device_id, localPaths: paths, projectId: project.id, dir, opts });
      toast.success(`Sent ${paths.length} item${paths.length === 1 ? '' : 's'} to ${device.name}`);
    } catch (e) {
      const m = errText(e);
      if (m !== 'Cancelled' && m !== 'Request cancelled') toast.error(m, { duration: 9000 });
    }
  };
  const upload = async (dir, directory) => {
    const paths = await pickUpload(directory);
    if (paths.length) sendTo(paths, dir);
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
  const wide = !!usable;
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
  useEffect(() => { setSyncWide(active && wide); }, [active, wide, setSyncWide]);
  useEffect(() => () => setSyncWide(false), [setSyncWide]);

  return (
    <>
          <div className="flex h-full w-full overflow-hidden bg-sidebar">
            {/* Machines */}
            <Column
              className={wide ? 'w-80 shrink-0' : 'w-full'}
              title="Cloud & Sync"
              actions={(
                <>
                  {wide && (
                    <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Back to the machine list" onClick={() => setDeviceId(null)}><ChevronLeft className="size-4" /></Button>
                  )}
                  <Button size="sm" variant="ghost" className="h-7 gap-1 px-2 text-[11.5px] text-muted-foreground hover:text-foreground" title={tab === 'backends' ? 'Add a remote backend' : 'Add a machine by address'} onClick={() => setAdd(tab === 'backends' ? 'backend' : 'sync')}>
                    <Plus className="size-3.5" /> Add machine
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Refresh" disabled={refreshing} onClick={refreshAll}><RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} /></Button>
                </>
              )}
            >
              <div className="space-y-2 p-2">
                {!IS_WEB && (
                  <Slider
                    value={tab}
                    onChange={(v) => { setTab(v); setDeviceId(null); }}
                    options={[
                      { id: 'sync', label: 'Sync', icon: CloudDownload, count: lan.devices.length },
                      { id: 'backends', label: 'Backends', icon: Globe },
                    ]}
                  />
                )}
                <div className="[&_section]:mb-2">
                  <MyMachine status={lan.status} busy={lan.busy} onToggleLan={lan.toggle} onRename={lan.rename} onRefresh={lan.refreshStatus} defaultOpen={!wide} />
                </div>
                {tab === 'backends' && !IS_WEB && <BackendsPanel />}
                {tab === 'sync' && (
                  <>
                    <SearchBox value={query} onChange={setQuery} placeholder="Search machines…" />
                    <div className="space-y-0.5">
                      {visible.length === 0 && (
                        <div className="flex items-center gap-2 px-2 py-3 text-[11.5px] text-muted-foreground">
                          {q ? 'No matches.' : lan.enabled
                            ? <><Loader2 className="size-3 animate-spin" /> Looking for machines — or Add machine.</>
                            : 'Turn on sync under My machine, or Add machine.'}
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
                  </>
                )}
              </div>
            </Column>

            {!showBackends && usable && (
              <MachineColumn
                device={device}
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
            )}
            {!showBackends && usable && subTab === 'projects' && project && (
              <RemoteFiles
                key={`${device.device_id}:${project.id}`}
                device={device}
                project={project}
                previewPath={file?.path}
                onPreview={setFile}
                onImport={importSelected}
                onUpload={upload}
                onDropLocal={sendTo}
              />
            )}
            {!showBackends && usable && ((subTab === 'projects' && file && project) || (subTab === 'meta' && metaItem)) && (
              <PreviewColumn
                device={device}
                project={project}
                file={subTab === 'projects' ? file : null}
                metaItem={subTab === 'meta' ? metaItem : null}
                onClose={() => { setFile(null); setMetaItem(null); }}
              />
            )}
            {!showBackends && usable && (
              <>
                <div className="flex-1" />
                <LocalExplorer
                  canSend={!!(usable && project)}
                  onSend={(paths) => sendTo(paths, '')}
                  onDropRemote={(payload, dest) => pullTo(payload.deviceId, payload.items, dest)}
                />
              </>
            )}
          </div>

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
