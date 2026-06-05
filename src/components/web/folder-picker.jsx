import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Folder, File as FileIcon, FolderPlus, FilePlus, ArrowUp, RefreshCw,
  Trash2, Pencil, X, Check, HardDrive, Loader2, CornerDownLeft,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';

/** Parent directory of a path, or null at a drive/filesystem root. */
function parentOf(p) {
  if (!p) return null;
  const trimmed = p.replace(/[\\/]+$/, '');
  const idx = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'));
  if (idx < 0) return null;
  const parent = trimmed.slice(0, idx);
  if (/^[A-Za-z]:$/.test(parent)) return parent + '\\';
  if (parent === '') return '/';
  return parent;
}

/** Last path segment (folder/file name) of a path. */
function basename(p) {
  if (!p) return '';
  const trimmed = p.replace(/[\\/]+$/, '');
  const idx = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'));
  return idx < 0 ? trimmed : trimmed.slice(idx + 1);
}

export function FolderPicker({ open, options, onResolve }) {
  const isDirMode = options?.directory !== false;
  const [roots, setRoots] = useState([]);
  const [path, setPath] = useState('');
  const [pathInput, setPathInput] = useState('');
  const [entries, setEntries] = useState([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [selected, setSelected] = useState(null);
  const [creating, setCreating] = useState(null);
  const [draftName, setDraftName] = useState('');
  const [renaming, setRenaming] = useState(null);
  const [renameValue, setRenameValue] = useState('');
  const [busy, setBusy] = useState(false);
  const draftRef = useRef(null);

  const loadDir = useCallback(async (target) => {
    setLoading(true);
    setError(null);
    setSelected(null);
    setCreating(null);
    setRenaming(null);
    try {
      const nodes = await invoke('read_dir', { path: target });
      setEntries(Array.isArray(nodes) ? nodes : []);
      setPath(target);
      setPathInput(target);
    } catch (e) {
      setError(typeof e === 'string' ? e : (e?.message ?? 'Failed to read directory'));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    (async () => {
      try {
        const info = await invoke('fs_picker_roots');
        if (cancelled) return;
        setRoots(info?.roots ?? []);
        const start = options?.defaultPath || info?.home || (info?.roots?.[0]?.path ?? '');
        await loadDir(start);
      } catch (e) {
        if (!cancelled) setError(typeof e === 'string' ? e : (e?.message ?? 'Failed to load'));
      }
    })();
    return () => { cancelled = true; };
  }, [open, options?.defaultPath, loadDir]);

  useEffect(() => {
    if (creating && draftRef.current) draftRef.current.focus();
  }, [creating]);

  if (!open) return null;

  const parent = parentOf(path);

  const enter = (entry) => {
    if (entry.is_dir) loadDir(entry.path);
    else setSelected(entry.path);
  };

  const confirmCreate = async () => {
    const name = draftName.trim();
    if (!name) { setCreating(null); return; }
    setBusy(true);
    try {
      await invoke(creating === 'folder' ? 'create_folder' : 'create_file', {
        dirPath: path,
        name,
      });
      setDraftName('');
      setCreating(null);
      await loadDir(path);
    } catch (e) {
      setError(typeof e === 'string' ? e : (e?.message ?? 'Create failed'));
    } finally {
      setBusy(false);
    }
  };

  const confirmRename = async (entry) => {
    const name = renameValue.trim();
    if (!name || name === entry.name) { setRenaming(null); return; }
    setBusy(true);
    try {
      await invoke('rename_entry', { oldPath: entry.path, newName: name });
      setRenaming(null);
      await loadDir(path);
    } catch (e) {
      setError(typeof e === 'string' ? e : (e?.message ?? 'Rename failed'));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (entry) => {
    const confirmed = window.confirm(
      `Delete ${entry.is_dir ? 'folder' : 'file'} "${entry.name}"?` +
      (entry.is_dir ? '\nThis removes the folder and everything inside it.' : ''),
    );
    if (!confirmed) return;
    setBusy(true);
    try {
      await invoke('delete_entry', { path: entry.path });
      await loadDir(path);
    } catch (e) {
      setError(typeof e === 'string' ? e : (e?.message ?? 'Delete failed'));
    } finally {
      setBusy(false);
    }
  };

  const canConfirm = isDirMode ? !!path : !!selected;
  const confirmLabel = isDirMode
    ? `Open ${path ? `"${basename(path) || path}"` : 'folder'}`
    : 'Open file';

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 p-4" onMouseDown={() => onResolve(null)}>
      <div
        className="flex h-[560px] max-h-[88vh] w-[760px] max-w-[94vw] flex-col overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-border px-3">
          <span className="text-sm font-semibold">
            {options?.title || (isDirMode ? 'Open a project folder' : 'Select a file')}
          </span>
          <Button variant="ghost" size="icon-sm" onClick={() => onResolve(null)} aria-label="Close">
            <X className="size-4" />
          </Button>
        </div>

        {roots.length > 0 && (
          <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-border/60 px-3 py-1.5">
            <span className="mr-1 text-[11px] uppercase tracking-wide text-muted-foreground">Go to</span>
            {roots.map((r) => (
              <Button key={r.path} variant="outline" size="sm" className="h-6 gap-1 px-2 text-xs" onClick={() => loadDir(r.path)}>
                <HardDrive className="size-3" /> {r.label}
              </Button>
            ))}
          </div>
        )}

        <div className="flex shrink-0 items-center gap-1.5 border-b border-border/60 px-3 py-2">
          <Button variant="ghost" size="icon-sm" disabled={!parent} onClick={() => parent && loadDir(parent)} aria-label="Up one level" title="Up one level">
            <ArrowUp className="size-4" />
          </Button>
          <form
            className="flex min-w-0 flex-1 items-center"
            onSubmit={(e) => { e.preventDefault(); if (pathInput.trim()) loadDir(pathInput.trim()); }}
          >
            <input
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              spellCheck={false}
              className="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 font-mono text-xs outline-none focus:ring-1 focus:ring-ring"
              placeholder="Path on the server"
            />
            <Button type="submit" variant="ghost" size="icon-sm" aria-label="Go" title="Go to path">
              <CornerDownLeft className="size-4" />
            </Button>
          </form>
          <Button variant="ghost" size="icon-sm" onClick={() => loadDir(path)} aria-label="Refresh" title="Refresh">
            <RefreshCw className={cn('size-4', loading && 'animate-spin')} />
          </Button>
        </div>

        <div className="flex shrink-0 items-center gap-1.5 border-b border-border/60 px-3 py-1.5">
          <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs" disabled={busy} onClick={() => { setCreating('folder'); setDraftName(''); }}>
            <FolderPlus className="size-3.5" /> New Folder
          </Button>
          <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs" disabled={busy} onClick={() => { setCreating('file'); setDraftName(''); }}>
            <FilePlus className="size-3.5" /> New File
          </Button>
        </div>

        <div className="min-h-0 flex-1 overflow-auto px-1.5 py-1">
          {error && (
            <div className="m-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </div>
          )}

          {creating && (
            <div className="flex items-center gap-2 rounded-md px-2 py-1.5">
              {creating === 'folder' ? <Folder className="size-4 text-primary/80" /> : <FileIcon className="size-4 text-muted-foreground" />}
              <input
                ref={draftRef}
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') confirmCreate(); if (e.key === 'Escape') setCreating(null); }}
                placeholder={creating === 'folder' ? 'New folder name' : 'New file name'}
                className="min-w-0 flex-1 rounded border border-input bg-background px-2 py-0.5 text-sm outline-none focus:ring-1 focus:ring-ring"
              />
              <Button variant="ghost" size="icon-sm" onClick={confirmCreate} disabled={busy} aria-label="Create"><Check className="size-4" /></Button>
              <Button variant="ghost" size="icon-sm" onClick={() => setCreating(null)} aria-label="Cancel"><X className="size-4" /></Button>
            </div>
          )}

          {!loading && entries.length === 0 && !creating && (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">This folder is empty.</div>
          )}

          {entries.map((entry) => {
            const isRenaming = renaming === entry.path;
            return (
              <div
                key={entry.path}
                className={cn(
                  'group flex items-center gap-2 rounded-md px-2 py-1.5 text-sm',
                  !isRenaming && 'cursor-pointer hover:bg-muted',
                  selected === entry.path && !isRenaming && 'bg-muted',
                  entry.is_ignored && 'opacity-50',
                )}
                onClick={() => !isRenaming && enter(entry)}
              >
                {entry.is_dir
                  ? <Folder className="size-4 shrink-0 text-primary/80" />
                  : <FileIcon className="size-4 shrink-0 text-muted-foreground" />}

                {isRenaming ? (
                  <input
                    autoFocus
                    value={renameValue}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setRenameValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') confirmRename(entry);
                      if (e.key === 'Escape') setRenaming(null);
                    }}
                    className="min-w-0 flex-1 rounded border border-input bg-background px-2 py-0.5 text-sm outline-none focus:ring-1 focus:ring-ring"
                  />
                ) : (
                  <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                )}

                {isRenaming ? (
                  <>
                    <Button variant="ghost" size="icon-sm" onClick={(e) => { e.stopPropagation(); confirmRename(entry); }} aria-label="Save name"><Check className="size-4" /></Button>
                    <Button variant="ghost" size="icon-sm" onClick={(e) => { e.stopPropagation(); setRenaming(null); }} aria-label="Cancel rename"><X className="size-4" /></Button>
                  </>
                ) : (
                  <div className="flex items-center gap-0.5 opacity-50 transition-opacity group-hover:opacity-100">
                    <Button
                      variant="ghost" size="icon-sm" disabled={busy}
                      onClick={(e) => { e.stopPropagation(); setRenaming(entry.path); setRenameValue(entry.name); }}
                      aria-label="Rename"
                    ><Pencil className="size-3.5" /></Button>
                    <Button
                      variant="ghost" size="icon-sm" disabled={busy}
                      onClick={(e) => { e.stopPropagation(); remove(entry); }}
                      aria-label="Delete"
                    ><Trash2 className="size-3.5 text-destructive" /></Button>
                  </div>
                )}
              </div>
            );
          })}

          {loading && (
            <div className="flex items-center justify-center gap-2 px-3 py-6 text-xs text-muted-foreground">
              <Loader2 className="size-4 animate-spin" /> Loading…
            </div>
          )}
        </div>

        <div className="flex shrink-0 items-center justify-between gap-2 border-t border-border px-3 py-2">
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground" title={isDirMode ? path : (selected ?? '')}>
            {isDirMode ? path : (selected || 'No file selected')}
          </span>
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => onResolve(null)}>Cancel</Button>
            <Button size="sm" disabled={!canConfirm} onClick={() => onResolve(isDirMode ? path : selected)}>
              {confirmLabel}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
