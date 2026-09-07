import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

// Compare two project roots the way the OS does: separators normalized, no
// trailing separator, case-insensitive on Windows. The folder picker and the
// DB can disagree on any of those and still mean the same directory.
function samePath(a, b) {
  const norm = (p) =>
    String(p || '')
      .replace(/[\\/]+/g, '/')
      .replace(/\/+$/, '');
  const x = norm(a);
  const y = norm(b);
  return navigator.platform?.startsWith('Win') ? x.toLowerCase() === y.toLowerCase() : x === y;
}

// Backend registers a synthetic "Global" project for the agent orchestrator
// (rustic_agent::GLOBAL_PROJECT_ID). It is not a user-facing workspace —
// strip it from any list shown in the file explorer / search scope dropdown.
const GLOBAL_PROJECT_ID = '__global__';

/// Case-insensitive alphabetical order by project name (root folder as a fallback).
function sortProjectsByName(list) {
  const key = (p) => String(p?.name || p?.root_path || '').toLowerCase();
  return [...list].sort((a, b) => key(a).localeCompare(key(b)));
}

// Once the user drags projects into a custom order, that order (persisted by
// the backend via reorder_projects) wins over the alphabetical default in every
// panel — Explorer, Search, Source Control and the Agent tree all read the same
// `projects` array. "Sort A→Z" clears the flag and re-persists alphabetical.
const MANUAL_ORDER_KEY = 'rustic.projects.manualOrder';
const hasManualOrder = () => {
  try {
    return localStorage.getItem(MANUAL_ORDER_KEY) === '1';
  } catch {
    return false;
  }
};
const setManualOrder = (on) => {
  try {
    if (on) localStorage.setItem(MANUAL_ORDER_KEY, '1');
    else localStorage.removeItem(MANUAL_ORDER_KEY);
  } catch {}
};

/// Applies the user's ordering policy: backend order when manually arranged, alphabetical otherwise.
function orderProjects(list) {
  return hasManualOrder() ? [...list] : sortProjectsByName(list);
}

export const useExplorer = create((set, get) => ({
  projects: [],
  activeProjectId: null,
  loading: false,
  error: null,
  expandedProjects: { left: {}, right: {} },
  // The node the user most recently clicked (or right-clicked) on across any
  // project's file tree. Drives Ctrl+V paste destination resolution in the
  // explorer header: file → paste into its parent dir, folder → paste into
  // that folder, nothing selected → fall back to `.rustic/uploaded/`.
  // Shape: { path, isDir, projectId } or null.
  lastSelectedNode: null,

  setLastSelectedNode: (node) => set({ lastSelectedNode: node || null }),
  clearLastSelectedNode: () => set({ lastSelectedNode: null }),

  // Multi-selection (Ctrl/Cmd-click toggles, Shift-click extends) mirrored out
  // of react-arborist so copy/cut/delete can act on the whole set. `rootPath`
  // identifies which project's tree owns the selection — each tree is its own
  // react-arborist instance, so ops must never mix trees. Shape:
  // { rootPath: string|null, items: [{ path, isDir, name }] }.
  selection: { rootPath: null, items: [] },
  setSelection: (rootPath, items) =>
    set((s) => {
      if (!items || items.length === 0) {
        // Only the owning tree may clear — another tree's mount/refresh
        // firing an empty onSelect must not clobber a live selection.
        if (s.selection.rootPath !== rootPath) return {};
        return { selection: { rootPath: null, items: [] } };
      }
      return { selection: { rootPath, items } };
    }),
  clearSelection: () => set({ selection: { rootPath: null, items: [] } }),

  hasLoaded: false,

  toggleProjectExpanded: (side, projectId) =>
    set((s) => ({
      expandedProjects: {
        ...s.expandedProjects,
        [side]: {
          ...s.expandedProjects[side],
          [projectId]: !s.expandedProjects[side]?.[projectId],
        },
      },
    })),

  collapseAllProjects: (side) =>
    set((s) => ({ expandedProjects: { ...s.expandedProjects, [side]: {} } })),

  setProjectExpanded: (side, projectId, expanded) =>
    set((s) => ({
      expandedProjects: {
        ...s.expandedProjects,
        [side]: { ...s.expandedProjects[side], [projectId]: !!expanded },
      },
    })),

  // Expand the given projects on `side` (never collapses anything) and scroll
  // the first one into view. Used when a sidebar panel opens to bring the
  // project of the active file / chat into view.
  revealProjects: (side, ids) => {
    if (!ids?.length) return;
    set((s) => {
      const next = { ...s.expandedProjects[side] };
      for (const id of ids) next[id] = true;
      return { expandedProjects: { ...s.expandedProjects, [side]: next } };
    });
    set({ scrollToProjectId: ids[0], scrollToProjectNonce: Date.now() });
  },
  scrollToProjectId: null,
  scrollToProjectNonce: 0,

  loadProjects: async () => {
    if (get().loading) return;
    set({ loading: true, error: null });
    try {
      const raw = await invoke('list_projects');
      const projects = orderProjects(raw.filter((p) => p.id !== GLOBAL_PROJECT_ID));
      const currentActive = get().activeProjectId;
      const activeStillValid = projects.some((p) => p.id === currentActive);
      set({
        projects,
        activeProjectId: activeStillValid ? currentActive : (projects[0]?.id ?? null),
        loading: false,
        hasLoaded: true,
      });
      // Folders renamed / moved / deleted outside the app leave dead entries
      // behind; drop them instead of making the user remove each by hand.
      get().pruneMissingProjects();
    } catch (err) {
      set({ error: String(err), loading: false, hasLoaded: true });
    }
  },

  setActiveProject: (id) => set({ activeProjectId: id }),

  // Apply a drag-drop reordering of the workspace projects. Updates the local
  // list immediately (optimistic) then persists to the backend so the order
  // survives a restart. `orderedIds` is the full new order of project ids.
  reorderProjects: async (orderedIds) => {
    const prev = get().projects;
    const byId = new Map(prev.map((p) => [p.id, p]));
    const reordered = orderedIds.map((id) => byId.get(id)).filter(Boolean);
    // Preserve any project not present in orderedIds (defensive) at the end.
    const rest = prev.filter((p) => !orderedIds.includes(p.id));
    set({ projects: [...reordered, ...rest], manualOrder: true });
    setManualOrder(true);
    try {
      await invoke('reorder_projects', { projectIds: orderedIds });
    } catch (err) {
      set({ projects: prev });
      throw err;
    }
  },

  manualOrder: hasManualOrder(),

  // Reset to alphabetical order everywhere and persist it so a reload agrees.
  sortProjectsAlphabetically: async () => {
    const sorted = sortProjectsByName(get().projects);
    set({ projects: sorted, manualOrder: false });
    setManualOrder(false);
    try {
      await invoke('reorder_projects', { projectIds: sorted.map((p) => p.id) });
    } catch (err) {
      console.error('persist alphabetical project order failed:', err);
    }
  },

  // Project id briefly flagged for the explorer to scroll to and outline —
  // used to point at the existing entry when the user re-adds a folder that is
  // already in the workspace.
  highlightedProjectId: null,

  flashProject: (projectId) => {
    if (!projectId) return;
    set({ highlightedProjectId: projectId });
    setTimeout(() => {
      if (get().highlightedProjectId === projectId) set({ highlightedProjectId: null });
    }, 2600);
  },

  addProject: async (path) => {
    const known = get().projects.find((p) => samePath(p.root_path, path));
    if (known) {
      toast.warning(`“${known.name}” is already in your workspace.`);
      set({ activeProjectId: known.id });
      get().flashProject(known.id);
      return known;
    }

    const project = await invoke('add_project', { path });
    // The backend returns the existing project when the folder is already
    // registered, so a duplicate can still surface here if the local list was
    // stale (path spelled differently, added from another panel/window).
    const existing = get().projects.find(
      (p) => p.id === project.id || samePath(p.root_path, project.root_path)
    );
    if (existing) {
      toast.warning(`“${existing.name}” is already in your workspace.`);
      set({ activeProjectId: existing.id });
      get().flashProject(existing.id);
      return existing;
    }

    set((s) => ({
      projects: s.manualOrder ? [...s.projects, project] : sortProjectsByName([...s.projects, project]),
      activeProjectId: s.activeProjectId ?? project.id,
    }));
    return project;
  },

  // Drop workspace entries whose folder no longer exists on disk (renamed,
  // moved or deleted outside the app). The backend archives them, so the task
  // history survives and re-adding the folder brings it back.
  pruneMissingProjects: async () => {
    let removed = [];
    try {
      removed = (await invoke('prune_missing_projects')) || [];
    } catch (err) {
      console.error('prune missing projects failed:', err);
      return [];
    }
    if (removed.length === 0) return [];
    const goneIds = new Set(removed.map((p) => p.id));
    set((s) => ({
      projects: s.projects.filter((p) => !goneIds.has(p.id)),
      activeProjectId: goneIds.has(s.activeProjectId) ? null : s.activeProjectId,
    }));
    const names = removed.map((p) => `“${p.name}”`).join(', ');
    toast.warning(
      removed.length === 1
        ? `Removed ${names} — its folder no longer exists on disk.`
        : `Removed ${removed.length} projects whose folders no longer exist: ${names}`
    );
    return removed;
  },

  removeProject: async (projectId) => {
    await invoke('remove_project', { projectId });
    set((s) => ({
      projects: s.projects.filter((p) => p.id !== projectId),
      activeProjectId: s.activeProjectId === projectId ? null : s.activeProjectId,
    }));
  },
}));

export async function readDir(path) {
  return invoke('read_dir', { path });
}

export async function createFile(dirPath, name) {
  return invoke('create_file', { dirPath, name });
}

export async function createFolder(dirPath, name) {
  return invoke('create_folder', { dirPath, name });
}

export async function renameEntry(oldPath, newName) {
  return invoke('rename_entry', { oldPath, newName });
}

export async function deleteEntry(path) {
  return invoke('delete_entry', { path });
}

export async function copyEntry(srcPath, dstDir, newName) {
  return invoke('copy_entry', { srcPath, dstDir, newName });
}

export async function moveEntry(srcPath, dstDir) {
  return invoke('move_entry', { srcPath, dstDir });
}

export async function writeClipboardFiles(paths, cut) {
  return invoke('write_clipboard_files', { paths, cut });
}

// Returns `{ paths: string[], cut: boolean }` — `cut` is true when the source
// app put the files on the clipboard with Ctrl+X (Windows Preferred
// DropEffect MOVE), so paste sites move instead of copy.
export async function readClipboardFiles() {
  return invoke('read_clipboard_files');
}

export async function pasteClipboardImageInto(dstDir) {
  return invoke('paste_clipboard_image_into', { dstDir });
}

export async function revealInFileManager(path) {
  return invoke('reveal_in_file_manager', { path });
}
