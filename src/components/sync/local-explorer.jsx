// "My explorer" column of the sync overlay: this machine's projects as a
// lazy tree. Drop items from the remote tree onto a folder to import them
// there; drag local items onto the remote tree to send them.
import React, { useCallback, useEffect, useState } from 'react';
import { ChevronRight, Folder, FolderOpen, FileText, Loader2, CheckSquare, Square, Upload } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { readDir, useExplorer } from '@/state/explorer';
import { Column, REMOTE_MIME, LOCAL_PATHS_MIME } from './remote-panels';
import { errText } from './use-lan';

/** Local tree; `onDropRemote(payload, destDir)`, `onSend(paths)` uploads to the open remote folder. */
export function LocalExplorer({ onDropRemote, onSend, canSend }) {
  const projects = useExplorer((s) => s.projects);
  const [entries, setEntries] = useState({});
  const [open, setOpen] = useState(new Set());
  const [loading, setLoading] = useState(new Set());
  const [sel, setSel] = useState(new Set());
  const [dropDir, setDropDir] = useState(null);

  const load = useCallback(async (path) => {
    setLoading((s) => new Set(s).add(path));
    try {
      const list = await readDir(path);
      setEntries((e) => ({ ...e, [path]: Array.isArray(list) ? list : [] }));
    } catch (e) {
      toast.error(errText(e));
    } finally {
      setLoading((s) => { const n = new Set(s); n.delete(path); return n; });
    }
  }, []);

  // Refresh open folders after something lands here.
  useEffect(() => {
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:transfer', (e) => {
        const t = e.payload;
        if (t?.direction === 'pull' && t.state === 'done' && t.location) {
          const norm = (p) => String(p).replace(/\\/g, '/').toLowerCase();
          setEntries((cur) => {
            Object.keys(cur).filter((k) => norm(k) === norm(t.location)).forEach((k) => load(k));
            return cur;
          });
        }
      }))
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => unlisten?.();
  }, [load]);

  const toggle = (path) => setOpen((s) => {
    const n = new Set(s);
    if (n.has(path)) n.delete(path); else { n.add(path); if (!entries[path]) load(path); }
    return n;
  });
  const toggleSel = (path) => setSel((s) => { const n = new Set(s); if (n.has(path)) n.delete(path); else n.add(path); return n; });

  const dragStart = (ev, path) => {
    const paths = sel.has(path) ? [...sel] : [path];
    ev.dataTransfer.setData(LOCAL_PATHS_MIME, JSON.stringify(paths));
    ev.dataTransfer.setData('application/x-rustic-file', paths[0]);
    ev.dataTransfer.effectAllowed = 'copy';
  };
  const dropProps = (dir) => ({
    onDragOver: (ev) => {
      if (!Array.from(ev.dataTransfer.types || []).includes(REMOTE_MIME)) return;
      ev.preventDefault(); ev.stopPropagation();
      ev.dataTransfer.dropEffect = 'copy';
      setDropDir(dir);
    },
    onDragLeave: () => setDropDir((d) => (d === dir ? null : d)),
    onDrop: (ev) => {
      const raw = ev.dataTransfer.getData(REMOTE_MIME);
      setDropDir(null);
      if (!raw) return;
      ev.preventDefault(); ev.stopPropagation();
      try { onDropRemote(JSON.parse(raw), dir); } catch { /* malformed drag */ }
    },
  });

  const row = (path, name, isDir, depth, key) => {
    const isOpen = open.has(path);
    return (
      <React.Fragment key={key}>
        <div
          draggable
          onDragStart={(ev) => dragStart(ev, path)}
          {...(isDir ? dropProps(path) : {})}
          onClick={() => isDir && toggle(path)}
          className={cn(
            'group flex h-7 cursor-default items-center gap-1 pr-2 text-[12px] hover:bg-ink/[0.05]',
            dropDir === path && 'bg-primary/20 ring-1 ring-inset ring-primary/60',
          )}
          style={{ paddingLeft: 6 + depth * 14 }}
        >
          <button type="button" className="shrink-0 text-muted-foreground hover:text-foreground" onClick={(ev) => { ev.stopPropagation(); toggleSel(path); }}>
            {sel.has(path) ? <CheckSquare className="size-3.5 text-primary" /> : <Square className="size-3.5 opacity-40 group-hover:opacity-100" />}
          </button>
          {isDir
            ? <ChevronRight className={cn('size-3.5 shrink-0 text-muted-foreground transition-transform', isOpen && 'rotate-90')} />
            : <span className="w-3.5 shrink-0" />}
          {isDir
            ? (isOpen ? <FolderOpen className="size-3.5 shrink-0 text-primary/80" /> : <Folder className="size-3.5 shrink-0 text-primary/80" />)
            : <FileText className="size-3.5 shrink-0 text-muted-foreground" />}
          <span className={cn('min-w-0 flex-1 truncate', depth === 0 && 'font-medium')}>{name}</span>
          {loading.has(path) && <Loader2 className="size-3 animate-spin text-muted-foreground" />}
        </div>
        {isDir && isOpen && (entries[path] || []).map((e) => row(e.path, e.name, !!e.is_dir, depth + 1, e.path))}
      </React.Fragment>
    );
  };

  return (
    <Column
      className="w-72 shrink-0"
      title="My machine"
      footer={sel.size > 0 && (
        <div className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">{sel.size} selected</span>
          <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => setSel(new Set())}>Clear</Button>
          <Button size="sm" className="h-7 text-xs" disabled={!canSend} title={canSend ? 'Send to the open remote folder' : 'Open a project on the other machine first'} onClick={() => { onSend([...sel]); setSel(new Set()); }}>
            <Upload className="size-3" /> Send
          </Button>
        </div>
      )}
    >
      {projects.length === 0 && <p className="p-3 text-[11.5px] text-muted-foreground">No projects open on this machine.</p>}
      {projects.map((p) => row(p.root_path, p.name, true, 0, p.id))}
    </Column>
  );
}
