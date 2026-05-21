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
  Search,
  GitBranch,
  Sparkles,
  Settings,
  Terminal,
  FilePlus,
  RefreshCw,
  FolderPlus,
  FileText,
  Save,
} from 'lucide-react';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import { useEditor } from '@/state/editor';
import { useTerminal } from '@/state/terminal';

const usePalette = create((set) => ({
  open: false,
  mode: 'commands',
  setOpen: (open, mode = 'commands') => set({ open, mode }),
  toggle: (mode = 'commands') => set((s) => ({ open: !s.open, mode: s.open ? s.mode : mode })),
}));

export function openCommandPalette() {
  usePalette.getState().setOpen(true, 'commands');
}

export function openFilePalette() {
  usePalette.getState().setOpen(true, 'files');
}

const BUILTIN_COMMANDS = [
  { id: 'view.explorer', label: 'View: Show Explorer', icon: Files, run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.EXPLORER) },
  { id: 'view.search', label: 'View: Show Search', icon: Search, run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.SEARCH) },
  { id: 'view.scm', label: 'View: Show Source Control', icon: GitBranch, run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.SCM) },
  { id: 'view.agent', label: 'View: Show Agent', icon: Sparkles, run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.AGENT) },
  { id: 'view.settings', label: 'View: Settings', icon: Settings, run: () => useLayout.getState().setActiveSidebarPanel(SIDEBAR_PANELS.SETTINGS) },
  { id: 'view.toggle-sidebar', label: 'View: Toggle Sidebar', icon: Files, run: () => useLayout.getState().toggleSidebar() },
  { id: 'view.toggle-bottom', label: 'View: Toggle Bottom Panel', icon: Terminal, run: () => useLayout.getState().toggleBottomPanel() },
  { id: 'terminal.new', label: 'Terminal: New', icon: Terminal, run: async () => {
    const { activeProjectId, projects } = useExplorer.getState();
    const activeProject = projects.find((p) => p.id === activeProjectId);
    const cwd = activeProject?.root_path;
    const label = activeProject?.name ?? 'shell';
    const info = await useTerminal.getState().createTerminal({ cwd, label });
    const tabTitle = info.pid != null ? `${label} • ${info.pid}` : label;
    useEditor.getState().openTerminal(info.id, tabTitle);
  } },
  { id: 'file.new-scratch', label: 'File: New Scratch Buffer', icon: FilePlus, run: () => useEditor.getState().openScratch('Untitled', 'plaintext') },
  { id: 'file.save', label: 'File: Save', icon: Save, run: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 's', ctrlKey: true })); } },
  { id: 'workspace.refresh-projects', label: 'Workspace: Refresh Projects', icon: RefreshCw, run: () => useExplorer.getState().loadProjects() },
];

export function CommandPalette() {
  const open = usePalette((s) => s.open);
  const mode = usePalette((s) => s.mode);
  const setOpen = usePalette((s) => s.setOpen);
  const [query, setQuery] = useState('');
  const [files, setFiles] = useState([]);
  const [loadingFiles, setLoadingFiles] = useState(false);

  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const activeProject = projects.find((p) => p.id === activeProjectId);

  useEffect(() => {
    if (!open) {
      setQuery('');
      return;
    }
    if (mode === 'files' && activeProject?.root_path) {
      setLoadingFiles(true);
      invoke('list_project_files', { rootPath: activeProject.root_path, maxFiles: 5000 })
        .then((list) => setFiles(Array.isArray(list) ? list : []))
        .catch(() => setFiles([]))
        .finally(() => setLoadingFiles(false));
    }
  }, [open, mode, activeProject?.root_path]);

  useEffect(() => {
    const onKey = (e) => {
      const mod = e.ctrlKey || e.metaKey;
      if (!mod) return;
      if (e.shiftKey && (e.key === 'P' || e.key === 'p')) {
        e.preventDefault();
        setOpen(true, 'commands');
      } else if (!e.shiftKey && (e.key === 'P' || e.key === 'p')) {
        e.preventDefault();
        setOpen(true, 'files');
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setOpen]);

  const fileMatches = useMemo(() => {
    if (mode !== 'files') return [];
    const q = query.trim().toLowerCase();
    const root = activeProject?.root_path ?? '';
    return files
      .filter((f) => !q || f.toLowerCase().includes(q))
      .slice(0, 200)
      .map((f) => ({ path: f, label: stripRoot(f, root) }));
  }, [mode, query, files, activeProject?.root_path]);

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
            <CommandGroup heading="Commands">
              {BUILTIN_COMMANDS.map((c) => {
                const Icon = c.icon ?? FileText;
                return (
                  <CommandItem key={c.id} value={c.label} onSelect={() => run(c.run)}>
                    <Icon className="size-3.5" />
                    <span>{c.label}</span>
                  </CommandItem>
                );
              })}
            </CommandGroup>
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
            {fileMatches.length > 0 && (
              <CommandGroup heading={activeProject?.name ?? 'Files'}>
                {fileMatches.map((m) => (
                  <CommandItem
                    key={m.path}
                    value={m.path}
                    onSelect={() => run(() => useEditor.getState().openFile(m.path))}
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
