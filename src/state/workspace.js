import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const workspaceStore = createStore({
  projects: [],
});

// Cache for loaded directory children: path -> FileNode[]
const childrenCache = new Map();

export async function addProject(path) {
  try {
    // If no path, use the dialog picker
    if (!path) {
      try {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const selected = await open({ directory: true, multiple: false });
        if (!selected) return null;
        path = selected;
      } catch {
        // Not in Tauri — prompt fallback
        path = prompt('Enter project folder path:');
        if (!path) return null;
      }
    }

    const project = await api.addProject(path);
    if (project) {
      const projects = [...workspaceStore.getState('projects')];
      // Avoid duplicates
      if (!projects.find(p => p.id === project.id)) {
        projects.push({ ...project, isExpanded: true, children: null });
        workspaceStore.setState({ projects });
      }
      // Load root children
      await loadChildren(project.root_path);
    }
    return project;
  } catch (e) {
    console.error('Failed to add project:', e);
    return null;
  }
}

export async function removeProject(id) {
  try {
    await api.removeProject(id);
    const projects = workspaceStore.getState('projects').filter(p => p.id !== id);
    workspaceStore.setState({ projects });
  } catch (e) {
    console.error('Failed to remove project:', e);
  }
}

export function toggleProject(id) {
  const projects = workspaceStore.getState('projects').map(p => {
    if (p.id === id) return { ...p, isExpanded: !p.isExpanded };
    return p;
  });
  workspaceStore.setState({ projects });
}

export async function loadChildren(path) {
  if (childrenCache.has(path)) {
    return childrenCache.get(path);
  }

  try {
    const children = await api.readDir(path);
    if (children) {
      childrenCache.set(path, children);
    }
    return children;
  } catch (e) {
    console.error('Failed to load children:', e);
    return [];
  }
}

export function getCachedChildren(path) {
  return childrenCache.get(path) || null;
}

export function clearChildrenCache(path) {
  childrenCache.delete(path);
}

export async function refreshProject(projectPath) {
  // Clear all cache entries that start with this project path
  for (const key of childrenCache.keys()) {
    if (key.startsWith(projectPath) || key === projectPath) {
      childrenCache.delete(key);
    }
  }
  await loadChildren(projectPath);
}

// Load saved projects on startup
export async function initWorkspace() {
  try {
    const projects = await api.listProjects();
    if (projects && projects.length > 0) {
      workspaceStore.setState({
        projects: projects.map(p => ({ ...p, isExpanded: true, children: null })),
      });
      // Load children for each project
      for (const p of projects) {
        await loadChildren(p.root_path);
      }
    }
  } catch (e) {
    console.error('Failed to init workspace:', e);
  }
}
