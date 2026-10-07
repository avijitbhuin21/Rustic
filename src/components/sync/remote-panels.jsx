// Remote-machine panels for the sync overlay: Projects | Metadata | Sharing,
// a lazily loaded file tree with multi-select + drag-drop both ways, and an
// on-demand preview pane.
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  ChevronRight, Folder, FolderOpen, FileText, Loader2, Download, Upload, FolderUp, ShieldCheck,
  KeyRound, CheckSquare, Square, RefreshCw, X, Lock,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { cn } from '@/lib/utils';
import { formatBytes } from '@/lib/transfer-format';
import { InfoTip } from '@/components/ui/info-tip';
import { Slider, SearchBox, ItemPicker } from '@/components/settings/cloud-machines';
import { errText } from './use-lan';

export const REMOTE_MIME = 'application/x-rustic-remote';
export const LOCAL_PATHS_MIME = 'application/x-rustic-local-paths';
const LOCAL_FILE_MIME = 'application/x-rustic-file';

const META_LABELS = {
  provider: 'Provider & key', model: 'Model settings', rule: 'Rule', skill: 'Skill', workflow: 'Workflow', mcp_server: 'MCP server',
};

/** Panel chrome shared by every column. */
export function Column({ title, actions, children, className, footer }) {
  return (
    <div className={cn('flex h-full min-w-0 flex-col border-r border-ink/[0.06] last:border-r-0', className)}>
      {(title || actions) && (
        <div className="flex h-10 shrink-0 items-center gap-2 border-b border-ink/[0.06] px-3">
          <div className="min-w-0 flex-1 truncate text-[12px] font-semibold">{title}</div>
          {actions}
        </div>
      )}
      <div className="explorer-scroll min-h-0 flex-1 overflow-y-auto">{children}</div>
      {footer && <div className="shrink-0 border-t border-ink/[0.06] px-3 py-2">{footer}</div>}
    </div>
  );
}

/** Local paths dragged from Rustic (our panel, main explorer). */
export function localPathsFrom(dt) {
  try {
    const multi = dt.getData(LOCAL_PATHS_MIME);
    if (multi) return JSON.parse(multi);
  } catch { /* fall through */ }
  const one = dt.getData(LOCAL_FILE_MIME);
  return one ? [one] : [];
}

/** Whether a drag carries local Rustic paths. */
export function hasLocalPaths(dt) {
  const types = Array.from(dt?.types || []);
  return types.includes(LOCAL_PATHS_MIME) || types.includes(LOCAL_FILE_MIME);
}

/** Projects | Metadata | Sharing column for one paired machine. */
export function MachineColumn({ device, tab, setTab, project, onProject, metaItem, onMetaItem, metaSel, setMetaSel, localProjects, localMeta, onImportMeta }) {
  const [projects, setProjects] = useState(null);
  const [error, setError] = useState(null);
  const [query, setQuery] = useState('');
  const load = useCallback(() => {
    setProjects(null); setError(null);
    invoke('lan_list_projects', { deviceId: device.device_id })
      .then((l) => setProjects(Array.isArray(l) ? l : []))
      .catch((e) => { setProjects([]); setError(errText(e)); });
  }, [device.device_id]);
  useEffect(() => { if (tab === 'projects') load(); }, [tab, load]);

  const q = query.trim().toLowerCase();
  return (
    <Column
      title={device.name}
      actions={tab === 'projects' && (
        <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Refresh" onClick={load}><RefreshCw className="size-3.5" /></Button>
      )}
    >
      <div className="space-y-2 p-2">
        <Slider
          value={tab}
          onChange={setTab}
          options={[
            { id: 'projects', label: 'Projects', icon: Folder },
            { id: 'meta', label: 'Metadata', icon: KeyRound },
            { id: 'share', label: 'Sharing', icon: ShieldCheck },
          ]}
        />
        {tab === 'projects' && (
          <>
            <SearchBox value={query} onChange={setQuery} placeholder="Search projects…" />
            {projects === null && <Skeleton rows={4} />}
            {error && <ErrorNote text={error} onRetry={load} />}
            {projects && !error && projects.length === 0 && (
              <p className="px-1 py-2 text-[11.5px] text-muted-foreground">{device.name} hasn't shared any projects with you yet. They can share them from Cloud &amp; Sync → your machine → Sharing.</p>
            )}
            <div className="space-y-0.5">
              {(projects || []).filter((p) => !q || p.name.toLowerCase().includes(q)).map((p) => (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => onProject(p)}
                  className={cn(
                    'flex w-full flex-col rounded-md px-2 py-1.5 text-left transition-colors hover:bg-ink/[0.06]',
                    project?.id === p.id && 'bg-primary/15 text-primary',
                  )}
                >
                  <span className="flex items-center gap-1.5 text-[12px] font-medium"><Folder className="size-3.5 shrink-0" />{p.name}</span>
                  <span className="truncate pl-5 text-[10.5px] text-muted-foreground" title={p.root_path}>{p.root_path}</span>
                </button>
              ))}
            </div>
          </>
        )}
        {tab === 'meta' && (
          <MetaList device={device} selected={metaItem} onSelect={onMetaItem} sel={metaSel} setSel={setMetaSel} onImport={onImportMeta} />
        )}
        {tab === 'share' && <SharingPane device={device} localProjects={localProjects} localMeta={localMeta} />}
      </div>
    </Column>
  );
}

/** Loading placeholder rows. */
export function Skeleton({ rows = 3 }) {
  return (
    <div className="space-y-1.5 p-1">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="h-6 animate-pulse rounded bg-ink/[0.05]" style={{ width: `${70 + ((i * 13) % 30)}%` }} />
      ))}
    </div>
  );
}

/** Inline error with retry. */
function ErrorNote({ text, onRetry }) {
  return (
    <div className="space-y-1.5 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-2 text-[11.5px] text-destructive">
      <div>{text}</div>
      {onRetry && <Button size="sm" variant="outline" className="h-6 text-[11px]" onClick={onRetry}>Retry</Button>}
    </div>
  );
}

/** Their metadata: needs a one-time grant; then browse, preview, import. */
function MetaList({ device, selected, onSelect, sel, setSel, onImport }) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [asking, setAsking] = useState(false);
  const load = useCallback(() => {
    setData(null); setError(null);
    invoke('lan_meta_browse', { deviceId: device.device_id }).then(setData).catch((e) => setError(errText(e)));
  }, [device.device_id]);
  useEffect(() => { load(); }, [load]);
  const ask = async () => {
    setAsking(true);
    try {
      await invoke('lan_request_meta_access', { deviceId: device.device_id });
      toast.success(`${device.name} allowed browsing its metadata`);
      load();
    } catch (e) {
      if (errText(e) !== 'Request cancelled') toast.error(errText(e));
    } finally { setAsking(false); }
  };
  if (error) return <ErrorNote text={error} onRetry={load} />;
  if (!data) return <Skeleton rows={5} />;
  const items = data.items || [];
  const groups = items.reduce((acc, it) => { (acc[it.category] ||= []).push(it); return acc; }, {});
  const key = (it) => `${it.category}/${it.name}`;
  return (
    <div className="space-y-2">
      {!data.granted && (
        <div className="space-y-2 rounded-md border border-ink/[0.08] bg-ink/[0.03] p-2 text-[11.5px] text-muted-foreground">
          <div className="flex items-center gap-1.5 font-medium text-foreground">
            <Lock className="size-3.5" /> Browsing needs permission
            <InfoTip>
              {device.name} decides once whether you may view all of its metadata. Copying items still asks each time.
              {items.length > 0 ? ' Until then you only see what it already shares with you (names only).' : ''}
            </InfoTip>
          </div>
          <div className="flex items-center gap-2">
            <Button size="sm" className="h-7 text-xs" disabled={asking} onClick={ask}>
              {asking ? <Loader2 className="size-3 animate-spin" /> : <KeyRound className="size-3" />} Request access
            </Button>
            {asking && (
              <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => invoke('lan_cancel_outgoing', { deviceId: device.device_id })}>Cancel</Button>
            )}
          </div>
        </div>
      )}
      {Object.entries(groups).map(([cat, list]) => (
        <div key={cat} className="space-y-0.5">
          <div className="px-1 text-[10px] uppercase tracking-wider text-muted-foreground/70">{META_LABELS[cat] || cat}</div>
          {list.map((it) => {
            const k = key(it);
            const checked = sel.has(k);
            return (
              <div key={k} className={cn('flex items-center gap-1.5 rounded-md px-1.5 py-1 hover:bg-ink/[0.06]', selected && key(selected) === k && 'bg-primary/15')}>
                <button type="button" onClick={() => setSel((s) => { const n = new Set(s); if (n.has(k)) n.delete(k); else n.add(k); return n; })} className="text-muted-foreground hover:text-foreground">
                  {checked ? <CheckSquare className="size-3.5 text-primary" /> : <Square className="size-3.5" />}
                </button>
                <button type="button" className="min-w-0 flex-1 truncate text-left font-mono text-[11.5px]" onClick={() => data.granted && onSelect(it)}>{it.name}</button>
              </div>
            );
          })}
        </div>
      ))}
      {items.length === 0 && data.granted && <p className="px-1 text-[11.5px] text-muted-foreground">No metadata there.</p>}
      {sel.size > 0 && (
        <Button size="sm" className="h-7 w-full text-xs" onClick={() => onImport(items.filter((it) => sel.has(key(it))))}>
          <Download className="size-3" /> Import {sel.size} item{sel.size === 1 ? '' : 's'}
        </Button>
      )}
    </div>
  );
}

/** What this machine shares with `device`, plus its metadata-browsing grant. */
function SharingPane({ device, localProjects, localMeta }) {
  const [share, setShare] = useState(null);
  const [p, setP] = useState(new Set());
  const [m, setM] = useState(new Set());
  const [metaView, setMetaView] = useState(false);
  useEffect(() => {
    invoke('lan_get_share', { deviceId: device.device_id })
      .then((s) => { setShare(s); setP(new Set(s?.projects || [])); setM(new Set(s?.meta || [])); })
      .catch((e) => toast.error(errText(e)));
    invoke('lan_get_meta_view', { deviceId: device.device_id }).then((v) => setMetaView(!!v)).catch(() => {});
  }, [device.device_id]);
  const dirty = share && (JSON.stringify([...p].sort()) !== JSON.stringify([...(share.projects || [])].sort())
    || JSON.stringify([...m].sort()) !== JSON.stringify([...(share.meta || [])].sort()));
  const save = async () => {
    try {
      await invoke('lan_set_share', { deviceId: device.device_id, share: { projects: [...p], meta: [...m] } });
      setShare({ projects: [...p], meta: [...m] });
      toast.success(`Sharing with ${device.name} updated`);
    } catch (e) { toast.error(errText(e)); }
  };
  const toggleMetaView = async (v) => {
    try { await invoke('lan_set_meta_view', { deviceId: device.device_id, allowed: v }); setMetaView(v); }
    catch (e) { toast.error(errText(e)); }
  };
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-1.5 px-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">
        Sharing with {device.name}
        <InfoTip>What {device.name} can browse and ask to pull from you. Every pull still asks first.</InfoTip>
      </div>
      <ItemPicker title="Projects" items={(localProjects || []).map((x) => ({ id: x.id, label: x.name, sub: x.root_path }))} selected={p} onChange={setP} loading={!share} empty="No projects open." />
      <ItemPicker title="Metadata" items={(localMeta || []).map((x) => ({ id: x.key, label: x.name, sub: META_LABELS[x.category] || x.category }))} selected={m} onChange={setM} loading={!share || !localMeta} empty="No metadata." />
      <div className="flex items-center justify-between gap-2 rounded-md border border-ink/[0.08] px-2 py-1.5 text-[11.5px]">
        <span>Allow browsing all my metadata</span>
        <Switch checked={metaView} onCheckedChange={toggleMetaView} />
      </div>
      <Button size="sm" className="h-7 w-full text-xs" disabled={!dirty} onClick={save}>Save sharing</Button>
    </div>
  );
}

/** Lazily loaded file tree of one remote project. */
export function RemoteFiles({ device, project, onPreview, previewPath, onImport, onUpload, onDropLocal }) {
  const [entries, setEntries] = useState({});
  const [open, setOpen] = useState(new Set(['']));
  const [loading, setLoading] = useState(new Set());
  const [sel, setSel] = useState(new Map());
  const [dropDir, setDropDir] = useState(null);
  const [error, setError] = useState(null);

  const load = useCallback(async (path) => {
    setLoading((s) => new Set(s).add(path));
    try {
      const list = await invoke('lan_list_files', { deviceId: device.device_id, projectId: project.id, path });
      setEntries((e) => ({ ...e, [path]: list }));
      setError(null);
    } catch (e) {
      if (path === '') setError(errText(e)); else toast.error(errText(e));
    } finally {
      setLoading((s) => { const n = new Set(s); n.delete(path); return n; });
    }
  }, [device.device_id, project.id]);

  useEffect(() => { setEntries({}); setOpen(new Set([''])); setSel(new Map()); load(''); }, [load]);

  const toggleDir = (path) => {
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path); else { n.add(path); if (!entries[path]) load(path); }
      return n;
    });
  };
  const toggleSel = (e) => setSel((m) => { const n = new Map(m); if (n.has(e.path)) n.delete(e.path); else n.set(e.path, e); return n; });
  const allTop = entries[''] || [];
  const selectAll = () => setSel(new Map([['', { path: '', name: project.name, is_dir: true }]]));
  const items = useMemo(() => [...sel.values()].map((e) => ({
    project_id: project.id, project_name: project.name, path: e.path, is_dir: !!e.is_dir,
  })), [sel, project]);

  const onDragStart = (ev, e) => {
    const list = sel.has(e.path) ? items : [{ project_id: project.id, project_name: project.name, path: e.path, is_dir: !!e.is_dir }];
    ev.dataTransfer.setData(REMOTE_MIME, JSON.stringify({ deviceId: device.device_id, items: list }));
    ev.dataTransfer.effectAllowed = 'copy';
  };
  const dropProps = (dir) => ({
    onDragOver: (ev) => {
      if (!hasLocalPaths(ev.dataTransfer)) return;
      ev.preventDefault(); ev.stopPropagation();
      ev.dataTransfer.dropEffect = 'copy';
      setDropDir(dir);
    },
    onDragLeave: () => setDropDir((d) => (d === dir ? null : d)),
    onDrop: (ev) => {
      const paths = localPathsFrom(ev.dataTransfer);
      setDropDir(null);
      if (!paths.length) return;
      ev.preventDefault(); ev.stopPropagation();
      onDropLocal(paths, dir);
    },
  });

  const renderDir = (path, depth) => (entries[path] || []).map((e) => {
    const isOpen = open.has(e.path);
    const checked = sel.has(e.path) || sel.has('');
    return (
      <React.Fragment key={e.path}>
        <div
          draggable
          onDragStart={(ev) => onDragStart(ev, e)}
          {...(e.is_dir ? dropProps(e.path) : {})}
          className={cn(
            'group flex h-7 cursor-default items-center gap-1 pr-2 text-[12px] hover:bg-ink/[0.05]',
            previewPath === e.path && 'bg-primary/15',
            dropDir === e.path && 'bg-primary/20 ring-1 ring-inset ring-primary/60',
          )}
          style={{ paddingLeft: 6 + depth * 14 }}
          onClick={() => (e.is_dir ? toggleDir(e.path) : onPreview(e))}
        >
          <button type="button" className="shrink-0 text-muted-foreground hover:text-foreground" onClick={(ev) => { ev.stopPropagation(); toggleSel(e); }}>
            {checked ? <CheckSquare className="size-3.5 text-primary" /> : <Square className="size-3.5 opacity-40 group-hover:opacity-100" />}
          </button>
          {e.is_dir
            ? <ChevronRight className={cn('size-3.5 shrink-0 text-muted-foreground transition-transform', isOpen && 'rotate-90')} />
            : <span className="w-3.5 shrink-0" />}
          {e.is_dir
            ? (isOpen ? <FolderOpen className="size-3.5 shrink-0 text-primary/80" /> : <Folder className="size-3.5 shrink-0 text-primary/80" />)
            : <FileText className="size-3.5 shrink-0 text-muted-foreground" />}
          <span className="min-w-0 flex-1 truncate">{e.name}</span>
          {loading.has(e.path) && <Loader2 className="size-3 animate-spin text-muted-foreground" />}
          {!e.is_dir && <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70">{formatBytes(e.size)}</span>}
        </div>
        {e.is_dir && isOpen && renderDir(e.path, depth + 1)}
      </React.Fragment>
    );
  });

  return (
    <Column
      title={project.name}
      actions={(
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Select whole project" onClick={selectAll}><CheckSquare className="size-3.5" /></Button>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Upload files here" onClick={() => onUpload('', false)}><Upload className="size-3.5" /></Button>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Upload a folder here" onClick={() => onUpload('', true)}><FolderUp className="size-3.5" /></Button>
        </>
      )}
      footer={sel.size > 0 && (
        <div className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
            {sel.has('') ? 'Whole project' : `${sel.size} selected`}
          </span>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Clear selection" onClick={() => setSel(new Map())}><X className="size-3.5" /></Button>
          <Button size="sm" className="h-7 text-xs" onClick={() => onImport(items)}><Download className="size-3" /> Import to my machine</Button>
        </div>
      )}
    >
      <div className={cn('min-h-full pb-6', dropDir === '' && 'bg-primary/10')} {...dropProps('')}>
        {error && <div className="p-2"><ErrorNote text={error} onRetry={() => load('')} /></div>}
        {!entries[''] && !error && <Skeleton rows={8} />}
        {entries[''] && allTop.length === 0 && <p className="p-3 text-[11.5px] text-muted-foreground">Empty project.</p>}
        {renderDir('', 0)}
      </div>
    </Column>
  );
}

/** Preview of one remote file (or metadata item), loaded on click. */
export function PreviewColumn({ device, project, file, metaItem, onClose }) {
  const [state, setState] = useState({ loading: false });
  useEffect(() => {
    if (!file) return undefined;
    let alive = true;
    setState({ loading: true });
    invoke('lan_preview_file', { deviceId: device.device_id, projectId: project.id, path: file.path })
      .then((p) => alive && setState({ loading: false, preview: p }))
      .catch((e) => alive && setState({ loading: false, error: errText(e) }));
    return () => { alive = false; };
  }, [file?.path, project?.id, device.device_id]);

  const title = file ? file.name : metaItem ? metaItem.name : '';
  const p = state.preview;
  return (
    <Column
      className="flex-1"
      title={title}
      actions={<Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" title="Close preview" onClick={onClose}><X className="size-3.5" /></Button>}
    >
      {metaItem && (
        <pre className="whitespace-pre-wrap break-all p-3 font-mono text-[11.5px] leading-relaxed">{JSON.stringify(metaItem.payload, null, 2)}</pre>
      )}
      {file && state.loading && (
        <div className="flex h-full flex-col items-center justify-center gap-2 text-[12px] text-muted-foreground">
          <Loader2 className="size-5 animate-spin" />
          Loading {file.name}{file.size ? ` (${formatBytes(file.size)})` : ''}…
        </div>
      )}
      {file && state.error && <div className="p-3"><ErrorNote text={state.error} /></div>}
      {file && p?.kind === 'text' && (
        <pre className="whitespace-pre p-3 font-mono text-[11.5px] leading-relaxed">{p.content}</pre>
      )}
      {file && p?.kind === 'image' && (
        <div className="flex h-full items-center justify-center p-4">
          <img alt={file.name} src={`data:${p.mime};base64,${p.content}`} className="max-h-full max-w-full rounded object-contain" />
        </div>
      )}
      {file && (p?.kind === 'binary' || p?.kind === 'too_large') && (
        <div className="flex h-full flex-col items-center justify-center gap-1 text-[12px] text-muted-foreground">
          <FileText className="size-6" />
          {p.kind === 'binary' ? 'Binary file' : 'Too large to preview'} · {formatBytes(p.size)}
          <span className="text-[11px]">Import it to open it locally.</span>
        </div>
      )}
    </Column>
  );
}
