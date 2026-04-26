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

export async function cloneAndAddProject(url, targetDir) {
  const clonedPath = await api.gitClone(url, targetDir || null);
  if (!clonedPath) return null;
  return addProject(clonedPath);
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
    const raw = await api.listProjects();
    // The backend always registers a "__global__" pseudo-project so tasks in
    // the Global orchestrator scope can use a valid FK. That row isn't a
    // real project — filter it out of the sidebar, explorer, and pickers.
    const projects = (raw || []).filter(p => p.id !== '__global__');
    if (projects.length > 0) {
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

      console.log('[FsWatcher] event project=%s dirs=%o', project_path, changed_dirs);
      const normRoot = normPath(project_path);

      for (const dir of changed_dirs) {
        const normDir = normPath(dir);

        // 1) If this dir is itself cached (user has expanded it before),
        //    invalidate + re-fetch and tell the UI to re-render its children.
        if (childrenCache.has(normDir) || normDir === normRoot) {
          childrenCache.delete(normDir);
          api.readDir(dir).then((children) => {
            if (children) childrenCache.set(normDir, children);
            _notifyDirRefresh(dir, project_path);
          }).catch(() => {
            childrenCache.delete(normDir);
            _notifyDirRefresh(dir, project_path);
          });
          continue;
        }

        // 2) The exact dir isn't cached, but one of its ancestors might be —
        //    e.g. user expanded `src/`, then created `src/newdir/file.ts` from
        //    OS file manager. The watcher reports `src/newdir` as changed, but
        //    only `src/` is in our cache. Refresh the closest cached ancestor
        //    so the new subdirectory shows up.
        const ancestor = findCachedAncestor(normDir, normRoot);
        if (ancestor) {
          console.log('[FsWatcher] uncached dir=%s, refreshing ancestor=%s', normDir, ancestor);
          childrenCache.delete(ancestor);
          api.readDir(ancestor).then((children) => {
            if (children) childrenCache.set(ancestor, children);
            _notifyDirRefresh(ancestor, project_path);
          }).catch(() => {
            childrenCache.delete(ancestor);
            _notifyDirRefresh(ancestor, project_path);
          });
        }
      }
    });
  } catch (e) {
    console.error('Failed to start fs change listener:', e);
  }

  // Window-focus fallback: when the user switches back to our window from
  // the OS file manager, refresh every cached directory. This catches the
  // case where the native file watcher missed events (Windows network
  // drives, antivirus interference, OneDrive folders, etc.) so the tree
  // always matches reality at least as soon as the app is foregrounded.
  // Throttled so multiple focus events in quick succession don't hammer the
  // backend.
  let lastFocusRefresh = 0;
  window.addEventListener('focus', () => {
    const now = Date.now();
    if (now - lastFocusRefresh < 500) return;
    lastFocusRefresh = now;
    refreshAllCachedDirs().catch((e) => {
      console.warn('[FsWatcher] focus refresh failed:', e);
    });
  });
}

/**
 * Walk up `normDir` looking for a cached ancestor inside the same project.
 * Returns the first cached ancestor's normalized path, or null.
 */
function findCachedAncestor(normDir, normRoot) {
  let cur = normDir;
  // Cap iterations to avoid pathological inputs.
  for (let i = 0; i < 64; i++) {
    const slash = cur.lastIndexOf('/');
    if (slash <= 0) return null;
    cur = cur.substring(0, slash);
    if (cur.length < normRoot.length) return null;
    if (childrenCache.has(cur)) return cur;
    if (cur === normRoot) return childrenCache.has(normRoot) ? normRoot : null;
  }
  return null;
}

/**
 * Re-fetch every cached directory and emit dir-refresh events for the ones
 * that actually changed (different file count or different names). Used as
 * the window-focus fallback when the OS-level watcher missed events.
 */
async function refreshAllCachedDirs() {
  const projects = workspaceStore.getState('projects');
  if (!projects || projects.length === 0) return;

  const projectByDir = (dir) => {
    return projects.find((p) => {
      const root = normPath(p.root_path);
      return dir === root || dir.startsWith(root + '/');
    });
  };

  const keys = Array.from(childrenCache.keys());
  for (const key of keys) {
    const project = projectByDir(key);
    if (!project) continue;

    const before = childrenCache.get(key);
    let after;
    try {
      after = await api.readDir(key);
    } catch {
      // Directory might have been deleted while the app was unfocused.
      childrenCache.delete(key);
      _notifyDirRefresh(key, project.root_path);
      continue;
    }

    if (childrenChanged(before, after)) {
      childrenCache.set(key, after || []);
      _notifyDirRefresh(key, project.root_path);
    }
  }
}

function childrenChanged(a, b) {
  if (!a && !b) return false;
  if (!a || !b) return true;
  if (a.length !== b.length) return true;
  // Compare names + is_dir, order-independent — defensive against backend
  // sort changes.
  const sigA = a.map(n => `${n.name}|${n.is_dir ? 1 : 0}`).sort().join(',');
  const sigB = b.map(n => `${n.name}|${n.is_dir ? 1 : 0}`).sort().join(',');
  return sigA !== sigB;
}

