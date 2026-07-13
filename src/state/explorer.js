import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

// Backend registers a synthetic "Global" project for the agent orchestrator
// (rustic_agent::GLOBAL_PROJECT_ID). It is not a user-facing workspace —
// strip it from any list shown in the file explorer / search scope dropdown.
const GLOBAL_PROJECT_ID = '__global__';

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

  loadProjects: async () => {
    if (get().loading) return;
    set({ loading: true, error: null });
    try {
      const raw = await invoke('list_projects');
      const projects = raw.filter((p) => p.id !== GLOBAL_PROJECT_ID);
      const currentActive = get().activeProjectId;
      const activeStillValid = projects.some((p) => p.id === currentActive);
      set({
        projects,
        activeProjectId: activeStillValid ? currentActive : (projects[0]?.id ?? null),
        loading: false,
        hasLoaded: true,
      });
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
    set({ projects: [...reordered, ...rest] });
    try {
      await invoke('reorder_projects', { projectIds: orderedIds });
    } catch (err) {
      set({ projects: prev });
      throw err;
    }
  },

  addProject: async (path) => {
    const project = await invoke('add_project', { path });
    set((s) => ({
      projects: [...s.projects, project],
      activeProjectId: s.activeProjectId ?? project.id,
    }));
    return project;
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
