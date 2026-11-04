// "My explorer" column of the sync overlay: this machine's projects as a
// lazy tree. Drop items from the remote tree onto a folder to import them
// there; drag local items onto the remote tree to send them.
import React, { useCallback, useEffect, useState } from 'react';
import {
  ChevronRight, Folder, FolderOpen, FileText, Loader2, FilePlus, FolderPlus, Pencil, Trash2, Copy, Scissors,
  Clipboard, TerminalSquare, ExternalLink, Download, Upload,
} from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import {
  readDir, useExplorer, createFile, createFolder, renameEntry, deleteEntry, copyEntry, moveEntry,
  writeClipboardFiles, readClipboardFiles, pasteClipboardImageInto, revealInFileManager,
} from '@/state/explorer';
import { useClipboard } from '@/state/clipboard';
import { useTerminal } from '@/state/terminal';
import { IS_WEB } from '@/lib/platform';
import { downloadPath, pickAndUploadFiles, pickAndUploadFolder } from '@/lib/file-transfer';
import { confirm } from '@/components/confirm-dialog';
import {
  ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuShortcut,
} from '@/components/ui/context-menu';
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Column, REMOTE_MIME, LOCAL_PATHS_MIME } from './remote-panels';
import { errText } from './use-lan';
import { useTreeSelection, setCountDragImage, parentPath } from './use-tree-selection';

/** Small modal asking for a file/folder name; resolves via `onSubmit(name)`. */
function NamePrompt({ prompt, onClose }) {
  const [value, setValue] = useState('');
  useEffect(() => { if (prompt) setValue(prompt.initial || ''); }, [prompt]);
  const submit = async () => {
    const name = value.trim();
    if (!name || /[\\/]/.test(name)) { toast.error('Enter a name without slashes'); return; }
    try { await prompt.onSubmit(name); onClose(); }
    catch (e) { toast.error(errText(e)); }
  };
  return (
    <Dialog open={!!prompt} onOpenChange={(o) => { if (!o) onClose(); }}>
      <DialogContent aria-describedby={undefined} className="max-w-sm">
        <DialogHeader><DialogTitle className="text-[14px]">{prompt?.title}</DialogTitle></DialogHeader>
        <Input autoFocus value={value} onChange={(e) => setValue(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && submit()} className="h-8 text-xs" />
        <DialogFooter>
          <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="h-7 text-xs" onClick={submit}>{prompt?.action || 'OK'}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Local tree; drop remote items on a folder to import them there (`onDropRemote(payload, destDir)`). */
export function LocalExplorer({ onDropRemote }) {
  const projects = useExplorer((s) => s.projects);
  const [entries, setEntries] = useState({});
  const [open, setOpen] = useState(new Set());
  const [loading, setLoading] = useState(new Set());
  const { sel, click } = useTreeSelection();
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

  // Live: reload loaded folders the file watcher reports as changed (edits,
  // agent writes, other apps). Batched so a burst of events reloads each once.
  const entriesRef = React.useRef(entries);
  entriesRef.current = entries;
  useEffect(() => {
    const key = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
    const dirs = new Set();
    const roots = new Set();
    let timer = null;
    let unlisten;
    let disposed = false;
    const flush = () => {
      timer = null;
      Object.keys(entriesRef.current).forEach((k) => {
        const kk = key(k);
        if (dirs.has(kk) || [...roots].some((r) => kk === r || kk.startsWith(`${r}/`))) load(k);
      });
      dirs.clear();
      roots.clear();
    };
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:fs-change', (e) => {
        const p = e.payload || {};
        if (p.rescan && p.project_path) roots.add(key(p.project_path));
        (p.changed_dirs || []).forEach((d) => dirs.add(key(d)));
        (p.changed_paths || []).forEach((f) => dirs.add(key(parentPath(f))));
        if (!timer) timer = setTimeout(flush, 250);
      }))
      .then((fn) => { if (disposed) fn(); else unlisten = fn; })
      .catch(() => {});
    return () => { disposed = true; if (timer) clearTimeout(timer); unlisten?.(); };
  }, [load]);

  const toggle = (path) => setOpen((s) => {
    const n = new Set(s);
    if (n.has(path)) n.delete(path); else { n.add(path); if (!entries[path]) load(path); }
    return n;
  });
  /** Paths of every visible row, top to bottom (for Shift ranges). */
  const visibleOrder = () => {
    const out = [];
    const walk = (p) => (entries[p] || []).forEach((x) => { out.push(x.path); if (x.is_dir && open.has(x.path)) walk(x.path); });
    projects.forEach((p) => { out.push(p.root_path); if (open.has(p.root_path)) walk(p.root_path); });
    return out;
  };

  const dragStart = (ev, path) => {
    const paths = sel.has(path) ? [...sel.keys()] : [path];
    ev.dataTransfer.setData(LOCAL_PATHS_MIME, JSON.stringify(paths));
    ev.dataTransfer.setData('application/x-rustic-file', paths[0]);
    ev.dataTransfer.effectAllowed = 'copy';
    setCountDragImage(ev.dataTransfer, paths.length, 'My machine');
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

  const [namePrompt, setNamePrompt] = useState(null);

  /** Reload `dir` and keep it expanded so the change is visible. */
  const refreshDir = (dir) => {
    setOpen((s) => (s.has(dir) ? s : new Set(s).add(dir)));
    load(dir);
  };
  /** Paths an action applies to: the whole selection when `path` is part of it. */
  const targetsFor = (path) => (sel.has(path) && sel.size > 1 ? [...sel.keys()] : [path]);
  const dirOf = (path, isDir) => (isDir ? path : parentPath(path));

  const newEntry = (dir, kind) => setNamePrompt({
    title: kind === 'file' ? 'New file' : 'New folder',
    action: 'Create',
    initial: '',
    onSubmit: async (name) => {
      if (kind === 'file') await createFile(dir, name); else await createFolder(dir, name);
      refreshDir(dir);
    },
  });
  const rename = (path, name) => setNamePrompt({
    title: `Rename ${name}`,
    action: 'Rename',
    initial: name,
    onSubmit: async (next) => {
      if (next === name) return;
      await renameEntry(path, next);
      refreshDir(parentPath(path));
    },
  });
  const remove = async (path, name) => {
    const paths = targetsFor(path);
    const ok = await confirm({
      title: paths.length > 1 ? `Delete ${paths.length} items?` : `Delete ${name}?`,
      description: 'This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    const parents = new Set();
    let failed = 0;
    for (const p of paths) {
      try { await deleteEntry(p); parents.add(parentPath(p)); } catch { failed += 1; }
    }
    parents.forEach((p) => load(p));
    if (failed) toast.error(`Deleted ${paths.length - failed}, ${failed} failed`);
    else toast.success(paths.length > 1 ? `Deleted ${paths.length} items` : `Deleted ${name}`);
  };
  const toClipboard = async (path, cut) => {
    const paths = targetsFor(path);
    if (cut) useClipboard.getState().cut(paths); else useClipboard.getState().copy(paths);
    if (!IS_WEB) {
      try { await writeClipboardFiles(paths, cut); } catch { /* in-app clipboard still works */ }
    }
  };
  // Same order as the main explorer: in-app clipboard, then OS file list, then OS image.
  const paste = async (dir) => {
    const { paths, isCut, clear } = useClipboard.getState();
    try {
      if (paths.length > 0) {
        for (const src of paths) { if (isCut) await moveEntry(src, dir); else await copyEntry(src, dir); }
        if (isCut) clear();
        toast.success(`${isCut ? 'Moved' : 'Pasted'} ${paths.length} item${paths.length > 1 ? 's' : ''}`);
        refreshDir(dir);
        return;
      }
      if (!IS_WEB) {
        const osDrop = await readClipboardFiles().catch(() => null);
        if (osDrop?.paths?.length) {
          for (const src of osDrop.paths) { if (osDrop.cut) await moveEntry(src, dir); else await copyEntry(src, dir); }
          toast.success(`${osDrop.cut ? 'Moved' : 'Pasted'} ${osDrop.paths.length} file${osDrop.paths.length > 1 ? 's' : ''}`);
          refreshDir(dir);
          return;
        }
        const img = await pasteClipboardImageInto(dir).catch(() => null);
        if (img) { toast.success('Image pasted'); refreshDir(dir); return; }
      }
      toast.info('Nothing to paste');
    } catch (e) {
      toast.error(`Paste failed: ${errText(e)}`);
    }
  };
  const copyPath = async (path) => {
    try { await navigator.clipboard.writeText(path); } catch { toast.error('Copy failed'); }
  };
  const reveal = async (path) => {
    try { await revealInFileManager(path); } catch (e) { toast.error(errText(e)); }
  };
  const openTerminal = async (path, name) => {
    try {
      const info = await useTerminal.getState().createTerminal({ cwd: path, label: name });
      const { useEditor } = await import('@/state/editor');
      useEditor.getState().openTerminal(info.id, info.label ?? name);
      toast.success(`Terminal opened in ${name}`);
    } catch (e) {
      toast.error(errText(e));
    }
  };
  const download = async (path, name, isDir) => {
    const t = toast.loading(isDir ? `Zipping "${name}"…` : `Downloading "${name}"…`);
    try { await downloadPath(path); toast.success(isDir ? `Downloaded "${name}.zip"` : `Downloaded "${name}"`); }
    catch (e) { toast.error(errText(e)); }
    finally { toast.dismiss(t); }
  };
  const upload = async (dir, folder) => {
    try {
      const count = folder ? await pickAndUploadFolder(dir) : await pickAndUploadFiles(dir);
      if (count > 0) { toast.success(`Uploaded ${count} item${count > 1 ? 's' : ''}`); refreshDir(dir); }
    } catch (e) {
      toast.error(errText(e));
    }
  };

  /** Right-click menu matching the main explorer's file menu. */
  const rowMenu = (path, name, isDir, isRoot) => {
    const dir = dirOf(path, isDir);
    const n = sel.has(path) && sel.size > 1 ? sel.size : 1;
    const count = n > 1 ? ` (${n})` : '';
    return (
      <ContextMenuContent className="w-56">
        {isDir && (
          <>
            <ContextMenuItem onSelect={() => newEntry(dir, 'file')}><FilePlus className="size-3.5" /> New File</ContextMenuItem>
            <ContextMenuItem onSelect={() => newEntry(dir, 'folder')}><FolderPlus className="size-3.5" /> New Folder</ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        {!isRoot && (
          <>
            <ContextMenuItem onSelect={() => rename(path, name)}><Pencil className="size-3.5" /> Rename<ContextMenuShortcut>F2</ContextMenuShortcut></ContextMenuItem>
            <ContextMenuItem variant="destructive" onSelect={() => remove(path, name)}><Trash2 className="size-3.5" /> Delete{count}<ContextMenuShortcut>Del</ContextMenuShortcut></ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        {!isRoot && <ContextMenuItem onSelect={() => toClipboard(path, false)}><Copy className="size-3.5" /> Copy{count}</ContextMenuItem>}
        {!isRoot && <ContextMenuItem onSelect={() => toClipboard(path, true)}><Scissors className="size-3.5" /> Cut{count}</ContextMenuItem>}
        <ContextMenuItem onSelect={() => paste(dir)}><Clipboard className="size-3.5" /> Paste</ContextMenuItem>
        <ContextMenuSeparator />
        {IS_WEB && (
          <>
            <ContextMenuItem onSelect={() => download(path, name, isDir)}><Download className="size-3.5" /> Download{isDir ? ' (zip)' : ''}</ContextMenuItem>
            <ContextMenuItem onSelect={() => upload(dir, false)}><Upload className="size-3.5" /> Upload Files{isDir ? '' : ' Here'}</ContextMenuItem>
            <ContextMenuItem onSelect={() => upload(dir, true)}><Upload className="size-3.5" /> Upload Folder</ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem onSelect={() => copyPath(path)}><Copy className="size-3.5" /> Copy Path</ContextMenuItem>
        {!IS_WEB && <ContextMenuItem onSelect={() => reveal(path)}><ExternalLink className="size-3.5" /> Reveal in File Manager</ContextMenuItem>}
        {isDir && <ContextMenuItem onSelect={() => openTerminal(path, name)}><TerminalSquare className="size-3.5" /> Open in Terminal</ContextMenuItem>}
      </ContextMenuContent>
    );
  };

  const row = (path, name, isDir, depth, key) => {
    const isOpen = open.has(path);
    return (
      <React.Fragment key={key}>
        <ContextMenu>
        <ContextMenuTrigger asChild>
        <div
          draggable
          onDragStart={(ev) => dragStart(ev, path)}
          {...dropProps(isDir ? path : parentPath(path))}
          onClick={(ev) => { if (click(ev, path, path, visibleOrder()) && isDir) toggle(path); }}
          onKeyDown={(ev) => {
            if (ev.key === 'F2' && depth > 0) { ev.preventDefault(); rename(path, name); }
            if ((ev.key === 'Delete' || ev.key === 'Backspace') && depth > 0 && (ev.key === 'Delete' || ev.metaKey)) { ev.preventDefault(); remove(path, name); }
          }}
          tabIndex={-1}
          className={cn(
            'group flex h-7 cursor-default select-none items-center gap-1 pr-2 text-[12px] hover:bg-ink/[0.05]',
            sel.has(path) && 'bg-primary/15 hover:bg-primary/20',
            dropDir === path && 'bg-primary/20 ring-1 ring-inset ring-primary/60',
          )}
          style={{ paddingLeft: 8 + depth * 14 }}
        >
          {isDir
            ? <ChevronRight className={cn('size-3.5 shrink-0 text-muted-foreground transition-transform', isOpen && 'rotate-90')} />
            : <span className="w-3.5 shrink-0" />}
          {isDir
            ? (isOpen ? <FolderOpen className="size-3.5 shrink-0 text-primary/80" /> : <Folder className="size-3.5 shrink-0 text-primary/80" />)
            : <FileText className="size-3.5 shrink-0 text-muted-foreground" />}
          <span className={cn('min-w-0 flex-1 truncate', depth === 0 && 'font-medium')}>{name}</span>
          {loading.has(path) && <Loader2 className="size-3 animate-spin text-muted-foreground" />}
        </div>
        </ContextMenuTrigger>
        {rowMenu(path, name, isDir, depth === 0)}
        </ContextMenu>
        {isDir && isOpen && (entries[path] || []).map((e) => row(e.path, e.name, !!e.is_dir, depth + 1, e.path))}
      </React.Fragment>
    );
  };

  return (
    <Column className="w-full" title="My machine">
      {projects.length === 0 && <p className="p-3 text-[11.5px] text-muted-foreground">No projects open on this machine.</p>}
      {projects.map((p) => row(p.root_path, p.name, true, 0, p.id))}
      <NamePrompt prompt={namePrompt} onClose={() => setNamePrompt(null)} />
    </Column>
  );
}
