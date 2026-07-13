import React, { useEffect, useMemo, useState } from 'react';
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command';
import {
  Files,
  Code2,
  Settings,
  Terminal,
  FileText,
  PanelLeft,
  CircleHelp,
  RefreshCw,
  History,
} from 'lucide-react';
import { useExplorer } from '@/state/explorer';
import { useEditor } from '@/state/editor';
import { useSettings } from '@/state/settings';
// Circular with commands.js (it imports openCommandPalette from here) — safe
// because both sides only touch the other's exports at call time, never
// during module evaluation.
import { COMMANDS, effectiveKey, displayKey } from '@/lib/commands';

const usePalette = create((set) => ({
  open: false,
  mode: 'commands',
  initialQuery: '',
  setOpen: (open, mode = 'commands', initialQuery = '') => set({ open, mode, initialQuery }),
  toggle: (mode = 'commands') => set((s) => ({ open: !s.open, mode: s.open ? s.mode : mode, initialQuery: '' })),
}));

export function openCommandPalette() {
  usePalette.getState().setOpen(true, 'commands');
}

export function openFilePalette(initialQuery = '') {
  usePalette.getState().setOpen(true, 'files', initialQuery);
}

const GROUP_ICONS = {
  Editor: Code2,
  Explorer: Files,
  File: FileText,
  Help: CircleHelp,
  Preferences: Settings,
  Terminal: Terminal,
  View: PanelLeft,
  Workspace: RefreshCw,
};

const GROUP_ORDER = ['View', 'File', 'Editor', 'Terminal', 'Explorer', 'Workspace', 'Preferences', 'Help'];

// Per-project file list cache (G15): module-level so it survives palette
// open/close — reopening no longer refetches 5000 paths. Entries older than
// the TTL are refreshed in the background while the stale list stays usable.
const FILE_CACHE_TTL_MS = 60_000;
const fileListCache = new Map();

const MRU_KEY = 'rustic.filePalette.mru';
const MRU_CAP = 8;

/** Load the per-project MRU list of recently opened palette files. */
function loadMru(root) {
  try {
    const all = JSON.parse(localStorage.getItem(MRU_KEY) || '{}');
    return Array.isArray(all[root]) ? all[root] : [];
  } catch {
    return [];
  }
}

/** Record a palette file-open at the front of the project's MRU list. */
function pushMru(root, path) {
  try {
    const all = JSON.parse(localStorage.getItem(MRU_KEY) || '{}');
    const list = Array.isArray(all[root]) ? all[root] : [];
    all[root] = [path, ...list.filter((p) => p !== path)].slice(0, MRU_CAP);
    localStorage.setItem(MRU_KEY, JSON.stringify(all));
  } catch {}
}

/** Subsequence fuzzy score (higher = better); null when q isn't a subsequence of path. */
function fuzzyScore(path, q) {
  let score = 0;
  let pi = 0;
  let prevHit = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const found = path.indexOf(q[qi], pi);
    if (found === -1) return null;
    score += 1;
    if (found === prevHit + 1) score += 4;
    const prev = path[found - 1];
    if (found === 0 || prev === '/' || prev === '.' || prev === '_' || prev === '-') score += 6;
    prevHit = found;
    pi = found + 1;
  }
    // Light penalty for how spread-out the hits are.
  score -= Math.floor((pi - q.length) / 8);
  const base = path.slice(path.lastIndexOf('/') + 1);
  if (base.includes(q)) score += 20;
  if (base.startsWith(q)) score += 10;
  return score;
}

export function CommandPalette() {
  const open = usePalette((s) => s.open);
  const mode = usePalette((s) => s.mode);
  const initialQuery = usePalette((s) => s.initialQuery);
  const setOpen = usePalette((s) => s.setOpen);
  const [query, setQuery] = useState('');
  const [files, setFiles] = useState([]);
  const [loadingFiles, setLoadingFiles] = useState(false);

  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const activeProject = projects.find((p) => p.id === activeProjectId);
  const keybindings = useSettings((s) => s.settings?.keybindings);

  const commandGroups = useMemo(() => {
    const by = new Map();
    for (const c of COMMANDS) {
      if (!c.run) continue;
      const key = effectiveKey(c.id, keybindings);
      if (!by.has(c.group)) by.set(c.group, []);
      by.get(c.group).push({ ...c, kbd: key ? displayKey(key) : null });
    }
    return [...by.entries()].sort(
      (a, b) => GROUP_ORDER.indexOf(a[0]) - GROUP_ORDER.indexOf(b[0]),
    );
  }, [keybindings]);

  useEffect(() => {
    if (!open) {
      setQuery('');
      return;
    }
    if (initialQuery) setQuery(initialQuery);
    if (mode === 'files' && activeProject?.root_path) {
      const root = activeProject.root_path;
      const cached = fileListCache.get(root);
      if (cached) setFiles(cached.files);
      if (cached && Date.now() - cached.at < FILE_CACHE_TTL_MS) return;
      if (!cached) setLoadingFiles(true);
      invoke('list_project_files', { rootPath: root, maxFiles: 5000 })
        .then((list) => {
          const files = Array.isArray(list) ? list : [];
          fileListCache.set(root, { files, at: Date.now() });
          setFiles(files);
        })
        .catch(() => {
          if (!cached) setFiles([]);
        })
        .finally(() => setLoadingFiles(false));
    }
  }, [open, mode, initialQuery, activeProject?.root_path]);

  const fileMatches = useMemo(() => {
    if (mode !== 'files') return [];
    const q = query.trim().toLowerCase();
    const root = activeProject?.root_path ?? '';
    if (!q) {
      return files.slice(0, 200).map((f) => ({ path: f, label: stripRoot(f, root) }));
    }
    const scored = [];
    for (const f of files) {
      const s = fuzzyScore(f.replace(/\\/g, '/').toLowerCase(), q);
      if (s !== null) scored.push([s, f]);
    }
    scored.sort((a, b) => b[0] - a[0]);
    return scored.slice(0, 200).map(([, f]) => ({ path: f, label: stripRoot(f, root) }));
  }, [mode, query, files, activeProject?.root_path]);

  // "Recent" group shown while the query is empty — the per-project MRU,
  // pruned to paths that still exist in the (loaded) file list.
  const recentFiles = useMemo(() => {
    if (mode !== 'files' || query.trim() || !activeProject?.root_path) return [];
    const known = files.length > 0 ? new Set(files) : null;
    return loadMru(activeProject.root_path)
      .filter((p) => !known || known.has(p))
      .map((p) => ({ path: p, label: stripRoot(p, activeProject.root_path) }));
  }, [mode, query, files, activeProject?.root_path, open]);

  const openFileFromPalette = (path) => {
    if (activeProject?.root_path) pushMru(activeProject.root_path, path);
    run(() => useEditor.getState().openFile(path));
  };

  const run = (fn) => {
    setOpen(false);
    setTimeout(() => fn(), 0);
  };

  return (
    <CommandDialog open={open} onOpenChange={(o) => setOpen(o, mode)} title={mode === 'files' ? 'Go to File' : 'Command Palette'}>
      <CommandInput
        placeholder={mode === 'files' ? 'Type a file name…' : 'Type a command…'}
        value={query}
        onValueChange={setQuery}
      />
      <CommandList>
        {mode === 'commands' && (
          <>
            <CommandEmpty>No matching command.</CommandEmpty>
            {commandGroups.map(([group, cmds]) => (
              <CommandGroup key={group} heading={group}>
                {cmds.map((c) => {
                  const Icon = GROUP_ICONS[group] ?? FileText;
                  return (
                    <CommandItem key={c.id} value={`${group}: ${c.label}`} onSelect={() => run(c.run)}>
                      <Icon className="size-3.5" />
                      <span>{c.label}</span>
                      {c.kbd && (
                        <kbd className="ml-auto rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                          {c.kbd}
                        </kbd>
                      )}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            ))}
          </>
        )}
        {mode === 'files' && (
          <>
            {!activeProject && (
              <CommandEmpty>No project active — add one in Explorer first.</CommandEmpty>
            )}
            {activeProject && loadingFiles && (
              <CommandEmpty>Indexing project files…</CommandEmpty>
            )}
            {activeProject && !loadingFiles && fileMatches.length === 0 && (
              <CommandEmpty>No matching files.</CommandEmpty>
            )}
            {recentFiles.length > 0 && (
              <CommandGroup heading="Recent">
                {recentFiles.map((m) => (
                  <CommandItem
                    key={`recent-${m.path}`}
                    value={`recent:${m.path}`}
                    onSelect={() => openFileFromPalette(m.path)}
                  >
                    <History className="size-3.5" />
                    <span className="truncate">{m.label}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {fileMatches.length > 0 && (
              <CommandGroup heading={activeProject?.name ?? 'Files'}>
                {fileMatches.map((m) => (
                  <CommandItem
                    key={m.path}
                    value={m.path}
                    onSelect={() => openFileFromPalette(m.path)}
                  >
                    <FileText className="size-3.5" />
                    <span className="truncate">{m.label}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
          </>
        )}
      </CommandList>
    </CommandDialog>
  );
}

function stripRoot(filePath, root) {
  if (!root) return filePath;
  const norm = filePath.replace(/\\/g, '/');
  const rnorm = root.replace(/\\/g, '/');
  if (norm.startsWith(rnorm)) {
    return norm.slice(rnorm.length + 1);
  }
  return norm;
}
