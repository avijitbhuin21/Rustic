// Settings > Workspace > Cloud: remote backend + sync (issue #15).
// Replaces the old "Remote Backend" and "Cloud Sync" blocks in General.
import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, Globe, ExternalLink, CloudUpload, CloudDownload, LogOut, Wifi, Server } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { useExplorer } from '@/state/explorer';
import { SettingsSection, SettingRow } from './setting-row';

const URL_KEY = 'rustic.remoteBackend.url';
const ENABLED_KEY = 'rustic.remoteBackend.enabled';

const PHASE_LABELS = {
  connecting: 'Connecting',
  preparing: 'Preparing',
  archiving: 'Packing files',
  compressing: 'Compressing',
  uploading: 'Uploading',
  applying: 'Server applying',
  packing: 'Server packing',
  downloading: 'Downloading',
  extracting: 'Extracting',
  installing: 'Installing',
  writing: 'Writing files',
  finalizing: 'Finalizing',
  done: 'Done',
};

/** Read a localStorage value, tolerating disabled storage. */
function readLocal(key, fallback) {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v;
  } catch {
    return fallback;
  }
}

/** Write a localStorage value, tolerating disabled storage. */
function writeLocal(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}

/** Cloud settings tab: remote backend connection and project sync. */
export function CloudSettings() {
  const projects = useExplorer((s) => s.projects);
  const [enabled, setEnabled] = useState(() => readLocal(ENABLED_KEY, '') === '1' || !!readLocal(URL_KEY, ''));
  const [url, setUrl] = useState(() => readLocal(URL_KEY, ''));
  const [password, setPassword] = useState('');
  const [testing, setTesting] = useState(false);
  const [verified, setVerified] = useState(null);
  const [opening, setOpening] = useState(false);
  const [remoteOpen, setRemoteOpen] = useState(false);

  const [transport, setTransport] = useState('remote'); // 'remote' | 'lan'
  const [scope, setScope] = useState('projects'); // 'projects' | 'everything'
  const [picker, setPicker] = useState(null); // 'push' | 'pull' | null
  const [selected, setSelected] = useState({});
  const [syncing, setSyncing] = useState(null);
  const [progress, setProgress] = useState(null);

  // Local-network sync (issue #15).
  const [lanEnabled, setLanEnabled] = useState(false);
  const [lanBusy, setLanBusy] = useState(false);
  const [devices, setDevices] = useState([]);
  const [deviceId, setDeviceId] = useState(null);
  const [pairing, setPairing] = useState(null); // { name, code } while waiting

  useEffect(() => {
    invoke('lan_status').then((s) => setLanEnabled(!!s?.enabled)).catch(() => {});
  }, []);

  useEffect(() => {
    if (transport !== 'lan' || !lanEnabled) return undefined;
    let alive = true;
    const load = () =>
      invoke('lan_devices')
        .then((list) => alive && setDevices(Array.isArray(list) ? list : []))
        .catch(() => {});
    load();
    const t = setInterval(load, 3000);
    return () => { alive = false; clearInterval(t); };
  }, [transport, lanEnabled]);

  const toggleLan = async (v) => {
    setLanBusy(true);
    try {
      await invoke('lan_set_enabled', { enabled: v });
      setLanEnabled(v);
      if (!v) setDevices([]);
    } catch (e) {
      toast.error(String(e?.message || e));
    } finally {
      setLanBusy(false);
    }
  };

  const pairDevice = async (d) => {
    try {
      const code = await invoke('lan_pair_code', { deviceId: d.device_id });
      setPairing({ name: d.name, code });
      const name = await invoke('lan_pair', { deviceId: d.device_id });
      toast.success(`Paired with ${name}`);
      setDeviceId(d.device_id);
      setDevices(await invoke('lan_devices'));
    } catch (e) {
      toast.error(String(e?.message || e));
    } finally {
      setPairing(null);
    }
  };

  const forgetDevice = async (d) => {
    try {
      await invoke('lan_forget', { deviceId: d.device_id });
      if (deviceId === d.device_id) setDeviceId(null);
      setDevices(await invoke('lan_devices'));
    } catch (e) {
      toast.error(String(e?.message || e));
    }
  };

  const device = devices.find((d) => d.device_id === deviceId);
  const peerLabel = transport === 'lan' ? device?.name || 'the other device' : 'the remote backend';

  // Same sync flows over either transport: the remote backend (password) or
  // a paired LAN device (pinned TLS + token).
  const api = {
    prepare: () => (transport === 'remote' ? invoke('cloud_sync_remember', { password }) : Promise.resolve()),
    metaPreview: (direction) =>
      transport === 'lan'
        ? invoke('lan_meta_preview', { deviceId, direction })
        : invoke('cloud_meta_preview', { url: url.trim(), direction }),
    metaApply: (direction, overwrite) =>
      transport === 'lan'
        ? invoke('lan_meta_apply', { deviceId, direction, overwrite })
        : invoke('cloud_meta_apply', { url: url.trim(), direction, overwrite }),
    listRemote: () =>
      transport === 'lan'
        ? invoke('lan_list_projects', { deviceId })
        : invoke('cloud_list_remote_projects', { url: url.trim() }),
    everything: (direction) =>
      transport === 'lan'
        ? invoke(direction === 'push' ? 'lan_push' : 'lan_pull', { deviceId })
        : invoke(direction === 'push' ? 'cloud_sync_push' : 'cloud_sync_pull', { url: url.trim(), password }),
    project: (direction, projectId, targetParent) => {
      const extra = targetParent ? { targetParent } : {};
      return transport === 'lan'
        ? invoke(direction === 'push' ? 'lan_push' : 'lan_pull', { deviceId, projectId, ...extra })
        : invoke(direction === 'push' ? 'cloud_sync_push_project' : 'cloud_sync_pull_project', { url: url.trim(), projectId, ...extra });
    },
  };

  useEffect(() => {
    invoke('remote_backend_is_open').then((v) => setRemoteOpen(!!v)).catch(() => {});
  }, []);

  useEffect(() => {
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:sync-progress', (e) => setProgress(e.payload)))
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, []);

  const toggleEnabled = (v) => {
    setEnabled(v);
    writeLocal(ENABLED_KEY, v ? '1' : '');
  };

  const persistUrl = (v) => {
    setUrl(v);
    setVerified(null);
    writeLocal(URL_KEY, v);
  };

  const testConnection = async () => {
    if (!url.trim()) return null;
    setTesting(true);
    setVerified(null);
    try {
      const base = await invoke('remote_backend_test', { url: url.trim(), password });
      setVerified(base);
      invoke('cloud_sync_remember', { password }).catch(() => {});
      toast.success('Connection verified');
      return base;
    } catch (e) {
      toast.error(String(e?.message || e));
      return null;
    } finally {
      setTesting(false);
    }
  };

  const connect = async () => {
    const base = verified || (await testConnection());
    if (!base) return;
    setOpening(true);
    try {
      await invoke('remote_backend_open', { url: base });
      setRemoteOpen(true);
      toast.success('Remote session opened in its own window');
    } catch (e) {
      toast.error(String(e?.message || e));
    } finally {
      setOpening(false);
    }
  };

  const disconnect = async () => {
    try {
      const was = await invoke('remote_backend_close');
      setRemoteOpen(false);
      if (was) toast.success('Remote session closed');
    } catch (e) {
      toast.error(String(e?.message || e));
    }
  };

  const canSync =
    syncing === null &&
    (transport === 'remote'
      ? enabled && !!url.trim() && !!password
      : lanEnabled && !!device?.paired && !!device?.online);

  const [remoteOnly, setRemoteOnly] = useState([]);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [metaDiff, setMetaDiff] = useState(null);
  const [metaOverwrite, setMetaOverwrite] = useState({});

  const openPicker = async (direction) => {
    setSelected(Object.fromEntries((projects || []).map((p) => [p.id, true])));
    setRemoteOnly([]);
    setMetaDiff(null);
    setMetaOverwrite({});
    setPicker(direction);
    if (scope === 'metadata') {
      setRemoteLoading(true);
      try {
        await api.prepare();
        const diff = await api.metaPreview(direction);
        setMetaDiff(Array.isArray(diff) ? diff : []);
      } catch (e) {
        toast.error(`Couldn't compare metadata: ${String(e?.message || e)}`);
        setPicker(null);
      } finally {
        setRemoteLoading(false);
      }
      return;
    }
    // Pull can also bring projects that only exist on the other side (issue #15).
    if (direction === 'pull' && scope === 'projects') {
      setRemoteLoading(true);
      try {
        await api.prepare();
        const remote = await api.listRemote();
        const localIds = new Set((projects || []).map((p) => p.id));
        setRemoteOnly((Array.isArray(remote) ? remote : []).filter((r) => !localIds.has(r.id)));
      } catch (e) {
        toast.error(`Couldn't list ${peerLabel}'s projects: ${String(e?.message || e)}`);
      } finally {
        setRemoteLoading(false);
      }
    }
  };

  // Full-environment sync (old behaviour) — replaces everything on the other side.
  const runEverything = async (direction) => {
    setPicker(null);
    setSyncing(direction);
    setProgress({ direction, phase: 'connecting', detail: transport === 'lan' ? peerLabel : url.trim(), done: 0, total: 0 });
    const toastId = toast.loading(direction === 'push' ? `Pushing to ${peerLabel}…` : `Pulling from ${peerLabel}…`, { duration: Infinity });
    try {
      const msg = await api.everything(direction);
      toast.success(msg, { id: toastId, duration: 4000 });
      if (transport === 'remote') invoke('cloud_sync_remember', { password }).catch(() => {});
      setProgress({ direction, phase: 'done', detail: msg, done: 1, total: 1 });
      if (direction === 'pull') setTimeout(() => window.location.reload(), 800);
      else setTimeout(() => setProgress(null), 4000);
    } catch (e) {
      toast.error(String(e?.message || e), { id: toastId, duration: 8000 });
      setProgress(null);
    } finally {
      setSyncing(null);
    }
  };

  // Per-project sync: only the ticked projects travel, one after another.
  const runProjects = async (direction) => {
    const ids = Object.entries(selected).filter(([, v]) => v).map(([k]) => k);
    setPicker(null);
    if (ids.length === 0) return;
    setSyncing(direction);
    try {
      await api.prepare();
    } catch (e) {
      toast.error(String(e?.message || e));
      setSyncing(null);
      return;
    }
    const remoteById = new Map(remoteOnly.map((r) => [r.id, r]));
    let ok = 0;
    const failures = [];
    for (const id of ids) {
      const remote = remoteById.get(id);
      const name = remote?.name || (projects || []).find((p) => p.id === id)?.name || id;
      let targetParent = null;
      if (direction === 'pull' && remote) {
        try {
          const { open } = await import('@tauri-apps/plugin-dialog');
          targetParent = await open({ directory: true, multiple: false, title: `Choose where to put "${name}"` });
        } catch (e) {
          targetParent = null;
        }
        if (!targetParent) {
          failures.push(`${name}: skipped (no folder chosen)`);
          continue;
        }
      }
      try {
        await api.project(direction, id, targetParent);
        ok += 1;
      } catch (e) {
        failures.push(`${name}: ${String(e?.message || e)}`);
      }
    }
    setSyncing(null);
    setTimeout(() => setProgress(null), 4000);
    if (failures.length === 0) {
      toast.success(`${direction === 'push' ? 'Pushed' : 'Pulled'} ${ok} project${ok === 1 ? '' : 's'}`);
    } else {
      toast.error(`${failures.length} project(s) failed — ${failures[0]}`, { duration: 8000 });
    }
  };

  // Metadata-only sync: merge, overwriting only the conflicts the user ticked.
  const runMetadata = async (direction) => {
    const overwrite = Object.entries(metaOverwrite).filter(([, v]) => v).map(([k]) => k);
    setPicker(null);
    setSyncing(direction);
    try {
      const res = await api.metaApply(direction, overwrite);
      const errs = res?.errors || [];
      const msg = `Metadata ${direction === 'push' ? 'pushed' : 'pulled'}: ${res?.added ?? 0} added, ${res?.replaced ?? 0} replaced, ${res?.kept ?? 0} kept`;
      if (errs.length) toast.error(`${msg} — ${errs.length} error(s): ${errs[0]}`, { duration: 8000 });
      else toast.success(msg);
      if (direction === 'pull') window.dispatchEvent(new CustomEvent('rustic:extensions-changed'));
    } catch (e) {
      toast.error(String(e?.message || e));
    } finally {
      setSyncing(null);
    }
  };

  const allOn = (projects || []).length > 0 && (projects || []).every((p) => selected[p.id]);

  return (
    <>
      <SettingsSection title="Remote Backend">
        <SettingRow
          label="Enable remote backend"
          description="Connect this app to a deployed rustic-server. Required for cloud sync."
        >
          <Switch checked={enabled} onCheckedChange={toggleEnabled} />
        </SettingRow>
        {enabled && (
          <>
            <SettingRow label="Server URL" description="e.g. https://rustic.example.com" htmlFor="cloud-url">
              <Input
                id="cloud-url"
                type="url"
                placeholder="https://rustic.example.com"
                value={url}
                onChange={(e) => persistUrl(e.target.value)}
                className="h-7 w-64 text-xs"
              />
            </SettingRow>
            <SettingRow label="Password" description="The server's access password." htmlFor="cloud-password">
              <Input
                id="cloud-password"
                type="password"
                autoComplete="off"
                value={password}
                onChange={(e) => { setPassword(e.target.value); setVerified(null); }}
                className="h-7 w-64 text-xs"
              />
            </SettingRow>
            <SettingRow
              label="Connection"
              description={
                remoteOpen
                  ? 'The remote session is open in its own window. This local workspace keeps running.'
                  : verified
                    ? `Verified: ${verified}`
                    : 'Test the connection before syncing or connecting.'
              }
            >
              <div className="flex items-center gap-1.5">
                <Button variant="outline" size="sm" className="h-7 text-xs" disabled={testing || !url.trim()} onClick={testConnection}>
                  {testing ? <Loader2 className="size-3 animate-spin" /> : <Globe className="size-3" />}
                  Test
                </Button>
                <Button size="sm" className="h-7 text-xs" disabled={testing || opening || !url.trim()} onClick={connect}>
                  {opening ? <Loader2 className="size-3 animate-spin" /> : <ExternalLink className="size-3" />}
                  {remoteOpen ? 'Focus' : 'Open remote window'}
                </Button>
                {remoteOpen && (
                  <Button variant="outline" size="sm" className="h-7 text-xs" onClick={disconnect}>
                    <LogOut className="size-3" /> Disconnect
                  </Button>
                )}
              </div>
            </SettingRow>
          </>
        )}
      </SettingsSection>

      <SettingsSection title="Sync">
        <SettingRow label="Sync over" description="Where projects are pushed to / pulled from.">
          <div className="flex rounded-md border border-border/60 p-0.5">
            {[
              { id: 'remote', label: 'Remote backend', icon: Server },
              { id: 'lan', label: 'Local network', icon: Wifi },
            ].map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => setTransport(t.id)}
                className={cn(
                  'flex items-center gap-1 rounded px-2.5 py-1 text-[11px]',
                  transport === t.id ? 'bg-muted text-foreground' : 'text-muted-foreground hover:text-foreground',
                )}
              >
                <t.icon className="size-3" /> {t.label}
              </button>
            ))}
          </div>
        </SettingRow>
        {transport === 'lan' ? (
          <LanPanel
            enabled={lanEnabled}
            busy={lanBusy}
            onToggle={toggleLan}
            devices={devices}
            selectedId={deviceId}
            onSelect={setDeviceId}
            onPair={pairDevice}
            onForget={forgetDevice}
          />
        ) : !enabled || !url.trim() || !password ? (
          <div className="px-3 py-2 text-[12px] text-amber-500">
            Enable the remote backend above and enter its URL and password to sync.
          </div>
        ) : null}
        {(transport === 'remote' || lanEnabled) && (
          <>
            <SettingRow
              label="What to sync"
              description={
                scope === 'projects'
                  ? 'Only the projects you pick. Push/Pull replaces those projects on the other side.'
                  : scope === 'metadata'
                    ? 'Providers & API keys, model settings, global rules, skills, workflows and MCP servers — no projects. Merged: nothing is removed, and you choose which same-name items get overwritten.'
                    : 'The whole environment: every project, agent tasks & chat history, API keys and providers, global rules, skills, workflows and MCP servers. Replaces everything on the other side.'
              }
            >
              <div className="flex rounded-md border border-border/60 p-0.5">
                {[
                  { id: 'projects', label: 'Selected projects' },
                  { id: 'metadata', label: 'Metadata only' },
                  { id: 'everything', label: 'Everything' },
                ].map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => setScope(s.id)}
                    className={cn(
                      'rounded px-2.5 py-1 text-[11px]',
                      scope === s.id ? 'bg-muted text-foreground' : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    {s.label}
                  </button>
                ))}
              </div>
            </SettingRow>
            <SettingRow
              label="Push / Pull"
              description={
                transport === 'lan' && !device?.paired
                  ? 'Pick a paired device above to sync with.'
                  : `Push sends this machine's copy to ${peerLabel}; Pull brings ${peerLabel}'s copy here.`
              }
            >
              <div className="flex items-center gap-1.5">
                <Button variant="outline" size="sm" className="h-7 text-xs" disabled={!canSync} onClick={() => openPicker('push')}>
                  {syncing === 'push' ? <Loader2 className="size-3 animate-spin" /> : <CloudUpload className="size-3" />}
                  Push
                </Button>
                <Button variant="outline" size="sm" className="h-7 text-xs" disabled={!canSync} onClick={() => openPicker('pull')}>
                  {syncing === 'pull' ? <Loader2 className="size-3 animate-spin" /> : <CloudDownload className="size-3" />}
                  Pull
                </Button>
              </div>
            </SettingRow>
            {progress && <SyncProgressRow progress={progress} />}
          </>
        )}
      </SettingsSection>

      <Dialog open={!!pairing} onOpenChange={() => {}}>
        <DialogContent className="max-w-sm" showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Pairing with {pairing?.name}</DialogTitle>
            <DialogDescription>
              Accept the request on {pairing?.name}. Check that it shows the same code before accepting.
            </DialogDescription>
          </DialogHeader>
          <div className="py-2 text-center font-mono text-3xl tracking-[0.35em]">{pairing?.code}</div>
          <div className="flex items-center justify-center gap-2 text-[12px] text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" /> Waiting for the other device…
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={picker !== null} onOpenChange={(open) => !open && setPicker(null)}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>
              {picker === 'push' ? `Push to ${peerLabel}` : `Pull from ${peerLabel}`}
            </DialogTitle>
            <DialogDescription>
              {scope === 'everything'
                ? picker === 'push'
                  ? 'The server’s data — projects, tasks, chat history, keys — will be replaced with this machine’s copy.'
                  : 'Everything on this machine — projects, tasks, chat history, keys — will be replaced with the server’s copy. The app reloads when done.'
                : picker === 'push'
                  ? 'The selected projects on the server will be replaced with this machine’s copy (including .env and other git-ignored files).'
                  : 'The selected local projects will be replaced with the server’s copy.'}
            </DialogDescription>
          </DialogHeader>
          {scope === 'metadata' && (
            <MetaReview
              diff={metaDiff}
              loading={remoteLoading}
              overwrite={metaOverwrite}
              setOverwrite={setMetaOverwrite}
              direction={picker}
            />
          )}
          {scope === 'projects' && (
            <div className="space-y-1.5">
              <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                <span>Projects</span>
                <button
                  type="button"
                  className="hover:text-foreground"
                  onClick={() => setSelected(Object.fromEntries((projects || []).map((p) => [p.id, !allOn])))}
                >
                  {allOn ? 'Clear all' : 'Select all'}
                </button>
              </div>
              <div className="max-h-56 space-y-1 overflow-y-auto rounded-md border border-border/50 p-2">
                {(projects || []).length === 0 && (
                  <div className="text-[11px] text-muted-foreground">No projects open.</div>
                )}
                {(projects || []).map((p) => (
                  <label key={p.id} className="flex cursor-pointer items-center gap-2 text-[12px]">
                    <input
                      type="checkbox"
                      className="accent-foreground"
                      checked={!!selected[p.id]}
                      onChange={(e) => setSelected((s) => ({ ...s, [p.id]: e.target.checked }))}
                    />
                    <span className="truncate">{p.name}</span>
                    <span className="ml-auto truncate font-mono text-[10px] text-muted-foreground">{p.root_path}</span>
                  </label>
                ))}
                {picker === 'pull' && remoteLoading && (
                  <div className="pt-1 text-[11px] text-muted-foreground">Loading the server's projects…</div>
                )}
                {picker === 'pull' && remoteOnly.length > 0 && (
                  <>
                    <div className="pt-2 text-[10.5px] uppercase tracking-wider text-muted-foreground/70">
                      On the server only — you'll pick a folder for each
                    </div>
                    {remoteOnly.map((r) => (
                      <label key={r.id} className="flex cursor-pointer items-center gap-2 text-[12px]">
                        <input
                          type="checkbox"
                          className="accent-foreground"
                          checked={!!selected[r.id]}
                          onChange={(e) => setSelected((s) => ({ ...s, [r.id]: e.target.checked }))}
                        />
                        <span className="truncate">{r.name}</span>
                        <span className="ml-auto text-[10px] text-muted-foreground">server</span>
                      </label>
                    ))}
                  </>
                )}
              </div>
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => setPicker(null)}>Cancel</Button>
            <Button
              variant="destructive"
              size="sm"
              className="h-7 text-xs"
              disabled={(scope === 'projects' && !Object.values(selected).some(Boolean)) || (scope === 'metadata' && !metaDiff)}
              onClick={() => (scope === 'everything' ? runEverything(picker) : scope === 'metadata' ? runMetadata(picker) : runProjects(picker))}
            >
              {picker === 'push' ? 'Push' : 'Pull'}
            </Button>
          </DialogFooter>
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
    <div className="space-y-1.5 border-t border-border px-3 py-2.5">
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

/** Local-network section: enable switch and the discovered / paired devices. */
function LanPanel({ enabled, busy, onToggle, devices, selectedId, onSelect, onPair, onForget }) {
  return (
    <>
      <SettingRow
        label="Allow local-network sync"
        description="Lets other Rustic desktops on this network find this machine and ask to pair. Off by default; Windows may ask to allow network access the first time."
      >
        <Switch checked={enabled} disabled={busy} onCheckedChange={onToggle} />
      </SettingRow>
      {enabled && (
        <div className="space-y-1 px-3 pb-2">
          <div className="text-[10.5px] uppercase tracking-wider text-muted-foreground/70">Devices on this network</div>
          {devices.length === 0 && (
            <div className="flex items-center gap-2 py-1 text-[12px] text-muted-foreground">
              <Loader2 className="size-3 animate-spin" /> Looking for other Rustic desktops (they must also allow local-network sync)…
            </div>
          )}
          {devices.map((d) => (
            <div
              key={d.device_id}
              className={cn(
                'flex items-center gap-2 rounded-md border px-2 py-1.5 text-[12px]',
                selectedId === d.device_id ? 'border-foreground/40 bg-muted' : 'border-border/50',
              )}
            >
              <input
                type="radio"
                className="accent-foreground"
                disabled={!d.paired}
                checked={selectedId === d.device_id}
                onChange={() => onSelect(d.device_id)}
              />
              <span className="truncate">{d.name}</span>
              <span className={cn('text-[10px]', d.online ? 'text-emerald-500' : 'text-muted-foreground')}>
                {d.online ? 'online' : 'offline'}
              </span>
              {d.paired && <span className="text-[10px] text-muted-foreground">paired</span>}
              <div className="ml-auto flex items-center gap-1">
                {!d.paired && d.online && (
                  <Button size="sm" variant="outline" className="h-6 text-[11px]" onClick={() => onPair(d)}>
                    Pair
                  </Button>
                )}
                {d.paired && (
                  <Button size="sm" variant="ghost" className="h-6 text-[11px] text-muted-foreground" onClick={() => onForget(d)}>
                    Forget
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

const CATEGORY_LABELS = {
  provider: 'Provider & API key',
  model: 'Model settings',
  rule: 'Rule',
  skill: 'Skill',
  workflow: 'Workflow',
  mcp_server: 'MCP server',
};

/** Review list for metadata-only sync: pick which same-name items to overwrite. */
function MetaReview({ diff, loading, overwrite, setOverwrite, direction }) {
  if (loading || !diff) {
    return <div className="py-3 text-[12px] text-muted-foreground">Comparing metadata…</div>;
  }
  const conflicts = diff.filter((d) => d.status === 'conflict');
  const added = diff.filter((d) => d.status === 'new');
  const kept = diff.filter((d) => d.status === 'local_only');
  const same = diff.filter((d) => d.status === 'same');
  const target = direction === 'push' ? 'the other side' : 'this machine';
  const allOn = conflicts.length > 0 && conflicts.every((c) => overwrite[c.key]);
  const row = (d, extra) => (
    <div key={d.key} className="flex items-center gap-2 text-[12px]">
      {extra}
      <span className="w-28 shrink-0 text-[10.5px] text-muted-foreground">{CATEGORY_LABELS[d.category] || d.category}</span>
      <span className="truncate font-mono">{d.name}</span>
    </div>
  );
  return (
    <div className="max-h-72 space-y-3 overflow-y-auto rounded-md border border-border/50 p-2">
      {conflicts.length > 0 && (
        <div className="space-y-1">
          <div className="flex items-center justify-between text-[11px] font-medium text-amber-500">
            <span>Same name, different content on {target} — tick to overwrite</span>
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground"
              onClick={() => setOverwrite(Object.fromEntries(conflicts.map((c) => [c.key, !allOn])))}
            >
              {allOn ? 'Keep all' : 'Overwrite all'}
            </button>
          </div>
          {conflicts.map((d) =>
            row(
              d,
              <input
                type="checkbox"
                className="accent-foreground"
                checked={!!overwrite[d.key]}
                onChange={(e) => setOverwrite((s) => ({ ...s, [d.key]: e.target.checked }))}
              />,
            ),
          )}
        </div>
      )}
      {added.length > 0 && (
        <div className="space-y-1">
          <div className="text-[11px] font-medium text-emerald-500">Will be added to {target}</div>
          {added.map((d) => row(d))}
        </div>
      )}
      {kept.length > 0 && (
        <div className="space-y-1">
          <div className="text-[11px] font-medium text-muted-foreground">Kept (only on {target})</div>
          {kept.map((d) => row(d))}
        </div>
      )}
      {same.length > 0 && (
        <div className="text-[11px] text-muted-foreground">{same.length} identical item(s) skipped.</div>
      )}
      {diff.length === 0 && <div className="text-[12px] text-muted-foreground">Nothing to sync.</div>}
    </div>
  );
}

export default CloudSettings;
