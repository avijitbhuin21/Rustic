import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { refreshGitStatus } from './git.js';

export const workspaceStore = createStore({
  projects: [],
});

const childrenCache = new Map();

/** Tracks which directory paths are currently expanded in the explorer. */
export const expandedDirs = new Set();

const normPath = (p) => (p ? p.replace(/\\/g, '/') : p);

export async function addProject(path) {
  try {
    if (!path) {
      try {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const selected = await open({ directory: true, multiple: false });
        if (!selected) return null;
        path = selected;
      } catch {
        path = prompt('Enter project folder path:');
        if (!path) return null;
      }
    }

    const project = await api.addProject(path);
    if (project) {
      const projects = [...workspaceStore.getState('projects')];
      if (!projects.find(p => p.id === project.id)) {
        projects.push({ ...project, isExpanded: true, children: null });
        workspaceStore.setState({ projects });
      }
      await loadChildren(project.root_path);
      // Load git status in the background so the status-bar branch pill and
      // source-control panel reflect the branch immediately — without waiting
      // for the user to open Source Control. Errors are swallowed (non-git
      // projects are handled inside refreshGitStatus).
      refreshGitStatus(project.id).catch(() => {});
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
  const key = normPath(path);
  if (childrenCache.has(key)) {
    return childrenCache.get(key);
  }

  try {
    const children = await api.readDir(path);
    if (children) {
      childrenCache.set(key, children);
    }
    return children;
  } catch (e) {
    console.error('Failed to load children:', e);
    return [];
  }
}

export function getCachedChildren(path) {
  return childrenCache.get(normPath(path)) || null;
}

export function clearChildrenCache(path) {
  childrenCache.delete(normPath(path));
}

export async function refreshProject(projectPath) {
  console.log('[FileTree] refreshProject projectPath=%s', projectPath);
  const normRoot = normPath(projectPath);

  // Collect which expanded dirs we need to re-fetch
  const expandedPaths = [...expandedDirs]
    .filter(p => normPath(p).startsWith(normRoot) || normPath(p) === normRoot);

  // Clear all caches under project
  for (const key of childrenCache.keys()) {
    if (key.startsWith(normRoot) || key === normRoot) {
      childrenCache.delete(key);
    }
  }

  // Pre-fetch root + all expanded dirs so rebuild is synchronous
  const pathsToFetch = [projectPath, ...expandedPaths];
  await Promise.all(pathsToFetch.map(p => loadChildren(p)));

  _notifyTreeRefresh(projectPath);
}

/**
 * Invalidate and re-fetch the parent directory of a file path that was
 * created / modified / deleted by the agent. Only re-renders the affected
 * directory in the DOM, preserving the rest of the tree state.
 */
export async function refreshAffectedDirectory(filePath) {
  console.log('[FileTree] refreshAffectedDirectory filePath=%s', filePath);
  if (!filePath) return;

  const normalize = (p) => p.replace(/\\/g, '/');
  const norm = normalize(filePath);

  const projects = workspaceStore.getState('projects');
  const project = projects.find((p) => norm.startsWith(normalize(p.root_path)));
  if (!project) return;

  const parentDir = norm.includes('/')
    ? norm.replace(/\/[^/]+$/, '')
    : norm;

  // Re-fetch only the parent directory cache
  childrenCache.delete(parentDir);
  try {
    const children = await api.readDir(parentDir);
    if (children) childrenCache.set(parentDir, children);
  } catch {
    // directory may have been deleted — leave cache empty
  }

  // If the parent is the project root, also fire targeted refresh for root
  // Otherwise refresh the specific parent dir in-place
  _notifyDirRefresh(parentDir, project.root_path);
}

function _notifyTreeRefresh(projectPath) {
  window.dispatchEvent(
    new CustomEvent('rustic:file-tree-refresh', { detail: { projectPath } })
  );
}

/**
 * Fire a targeted refresh for a single directory — only that directory's
 * children are re-rendered in the DOM, preserving the rest of the tree.
 */
function _notifyDirRefresh(dirPath, projectPath) {
  window.dispatchEvent(
    new CustomEvent('rustic:file-tree-dir-refresh', {
      detail: { dirPath, projectPath },
    })
  );
}

// Load saved projects on startup
export async function initWorkspace() {
  try {
    const projects = await api.listProjects();
    if (projects && projects.length > 0) {
      workspaceStore.setState({
        projects: projects.map(p => ({ ...p, isExpanded: true, children: null })),
      });
      for (const p of projects) {
        await loadChildren(p.root_path);
      }
      // Kick off git status for every saved project in parallel, in the
      // background. The status-bar branch pill needs this to populate on
      // startup — otherwise the user has to open Source Control once before
      // the branch shows up.
      for (const p of projects) {
        refreshGitStatus(p.id).catch(() => {});
      }
    }
  } catch (e) {
    console.error('Failed to init workspace:', e);
  }

  // Listen for file system changes from the backend watcher
  startFsChangeListener();
}

/** Subscribe to backend file-system watcher events and auto-refresh affected dirs. */
async function startFsChangeListener() {
  try {
    await api.onFsChange((payload) => {
      const { project_path, changed_dirs } = payload;
      if (!project_path || !changed_dirs || changed_dirs.length === 0) return;

      const normRoot = normPath(project_path);

      // For each changed directory, invalidate cache and re-fetch if it's expanded
      for (const dir of changed_dirs) {
        const normDir = normPath(dir);
        const key = normDir;

        // Only refresh dirs that are cached (i.e. user has seen them)
        if (childrenCache.has(key) || normDir === normRoot) {
          childrenCache.delete(key);

          // Re-fetch in background then notify the UI
          api.readDir(dir).then((children) => {
            if (children) childrenCache.set(key, children);
            _notifyDirRefresh(dir, project_path);
          }).catch(() => {
            // Directory may have been deleted — clear cache and notify
            childrenCache.delete(key);
            _notifyDirRefresh(dir, project_path);
          });
        }
      }
    });
  } catch (e) {
    console.error('Failed to start fs change listener:', e);
  }
}
