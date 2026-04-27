import { el, icon } from '../../utils/dom.js';
import { loadChildren, getCachedChildren, clearChildrenCache, workspaceStore, toggleProject, expandedDirs, refreshAffectedDirectory, refreshProject } from '../../state/workspace.js';
import { showConfirmDialog } from '../confirm-dialog.js';
import { createFileTree } from './file-tree.js';
import { showContextMenu } from '../dropdown-menu.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { setDragType, clearDragType } from '../../utils/drag-state.js';
import { closeBuffersForPath } from '../../state/editor.js';
import { registerCommand } from '../../lib/commands.js';
import { registerWhen } from '../../lib/keybindings.js';
import { debug } from '../../lib/log.js';
import {
  copyItems as clipCopyItems,
  cutItems as clipCutItems,
  pasteIntoDir as clipPasteIntoDir,
  hasClipboard as clipHasClipboard,
  isCutPath as clipIsCutPath,
  subscribe as clipSubscribe,
} from '../../state/explorer-clipboard.js';


// expandedDirs is imported from workspace.js (shared state)

/** Pixels per indent level. Keep in sync with CSS and file-tree.js. */
export const INDENT_PX = 12;

// ===================== MULTI-SELECT STATE =====================

/** Map<path, { name, is_dir, projectName }> */
const selectedPaths = new Map();

/**
 * The most recently single-clicked/right-clicked item — used as a fallback
 * paste target & clipboard source when the user hasn't multi-selected with
 * Ctrl-click. Cleared when the user clicks empty explorer space or in another
 * scope (we listen for non-explorer focus changes below).
 */
let lastFocusedNode = null;

export function getSelectedPaths() { return selectedPaths; }


function toggleSelection(path, nodeInfo) {
  if (selectedPaths.has(path)) {
    selectedPaths.delete(path);
  } else {
    selectedPaths.set(path, nodeInfo);
  }
  refreshSelectionUI();
}

function clearSelection() {
  if (selectedPaths.size === 0) return;
  selectedPaths.clear();
  refreshSelectionUI();
}

function refreshSelectionUI() {
  // Remove all selected classes, then re-apply for current selection
  document.querySelectorAll('.file-tree-item--selected').forEach(el => {
    el.classList.remove('file-tree-item--selected');
  });
  for (const path of selectedPaths.keys()) {
    const wrapper = findWrapperByPath(path);
    if (wrapper) {
      const item = wrapper.querySelector(':scope > .file-tree-item');
      if (item) item.classList.add('file-tree-item--selected');
    }
  }
}

// Clear selection (and forget the last-focused item) when clicking outside
// the explorer entirely. Clicks on empty explorer space only clear selection,
// not the last-focused node — that way a user can click a file, click empty
// area in the tree, and Ctrl+V still works as "paste into that file's parent".
document.addEventListener('click', (e) => {
  const inExplorer = !!e.target.closest('.explorer') || !!e.target.closest('.project-section');
  if (!inExplorer) {
    // Click landed outside the explorer entirely → clipboard ops should
    // route to whatever component owns that scope, not us.
    lastFocusedNode = null;
  }
  if (selectedPaths.size === 0) return;
  // If click is inside a file-tree-item, let the item handler deal with it
  if (e.target.closest('.file-tree-item')) return;
  clearSelection();
});

// Explorer-scoped commands (Delete, F2 rename, Cut/Copy/Paste) flow through
// the central keybinding dispatcher. The `explorerFocus` when-clause is true
// when:
//   - the user has multi-selected items via Ctrl-click, OR
//   - they've recently single-clicked / right-clicked an item, OR
//   - keyboard focus is currently inside the explorer DOM
// This matches VS Code: once you've interacted with the tree, the next
// Ctrl+C / Delete / F2 acts on that interaction even if focus wandered.
registerWhen('explorerFocus', () => {
  if (selectedPaths.size > 0) return true;
  if (lastFocusedNode) return true;
  const a = document.activeElement;
  return !!(a && (a.closest('.file-tree') || a.closest('.project-section')));
});


registerCommand({
  id: 'explorer.deleteSelected',
  title: 'Delete Selected',
  category: 'Explorer',
  run: () => deleteSelectedPaths(),
});

// ===================== CLIPBOARD HELPERS =====================

/**
 * Resolve the entries the user wants to copy/cut. Priority:
 *   1. Multi-selection (Ctrl-clicked items)
 *   2. The most recently focused/right-clicked item — passed in as `fallback`
 *
 * Returns [{ path, name, is_dir, projectName }].
 */
function resolveClipboardSources(fallback) {
  if (selectedPaths.size > 0) {
    return Array.from(selectedPaths.entries()).map(([path, info]) => ({
      path,
      name: info.name,
      is_dir: info.is_dir,
      projectName: info.projectName,
    }));
  }
  if (fallback) return [fallback];
  if (lastFocusedNode) return [lastFocusedNode];
  return [];
}


/**
 * Decide which directory a paste should target. Priority:
 *   1. Hint argument (e.g. right-click target)
 *   2. Single selected entry: the dir itself if it's a folder, else its parent
 *   3. The project root of the (single) selected entry / hint
 */
function resolvePasteTargetDir(hint) {
  if (hint) {
    if (hint.is_dir) return hint.path;
    return getParentDir(hint.path);
  }
  if (selectedPaths.size === 1) {
    const [path, info] = Array.from(selectedPaths.entries())[0];
    return info.is_dir ? path : getParentDir(path);
  }
  if (lastFocusedNode) {
    return lastFocusedNode.is_dir
      ? lastFocusedNode.path
      : getParentDir(lastFocusedNode.path);
  }
  // Fall back to the first project's root if exactly one project is open.
  const projects = workspaceStore.getState('projects');
  if (projects.length === 1) return projects[0].root_path;
  return null;
}


async function pasteAtTarget(targetDir) {
  if (!targetDir) {
    console.warn('[explorer] paste: no target directory resolved');
    return;
  }
  // NOTE: we don't early-return on `!clipHasClipboard()` — clipPasteIntoDir
  // falls back to reading absolute paths from the OS clipboard so paste
  // works even when the user copied a file from another app (Windows
  // Explorer's "Copy as path", VS Code, another Rustic window, etc.).
  debug('explorer', 'paste', { targetDir, internalClipEmpty: !clipHasClipboard() });
  const created = await clipPasteIntoDir(targetDir);
  debug('explorer', 'paste created', { count: created.length, items: created });
  // Refresh the destination dir so new entries appear immediately. The
  // backend file-watcher will also fire for OS-level changes, but we don't
  // want to wait for it — internal pastes update the UI right away.
  await refreshAffectedDirectory(targetDir + '/.x'); // any child path triggers parent-dir refresh
  for (const dst of created) {
    // Best-effort: also nudge each created path's parent (handles cross-dir
    // edge cases like collision-renamed targets).
    try { await refreshAffectedDirectory(dst); } catch { /* ignore */ }
  }
}


registerCommand({
  id: 'explorer.copy',
  title: 'Copy',
  category: 'Explorer',
  run: () => {
    const items = resolveClipboardSources(null);
    if (items.length === 0) return;
    clipCopyItems(items);
    refreshClipboardCutUI();
  },
});

registerCommand({
  id: 'explorer.cut',
  title: 'Cut',
  category: 'Explorer',
  run: () => {
    const items = resolveClipboardSources(null);
    if (items.length === 0) return;
    clipCutItems(items);
    refreshClipboardCutUI();
  },
});

registerCommand({
  id: 'explorer.paste',
  title: 'Paste',
  category: 'Explorer',
  run: async () => {
    const target = resolvePasteTargetDir(null);
    await pasteAtTarget(target);
  },
});

/** Apply/remove the `--cut` class on items currently in the cut clipboard. */
function refreshClipboardCutUI() {
  document.querySelectorAll('.file-tree-item--cut').forEach(el => {
    el.classList.remove('file-tree-item--cut');
  });
  const wrappers = document.querySelectorAll('.file-tree-item-wrapper[data-path]');
  for (const w of wrappers) {
    if (clipIsCutPath(w.dataset.path)) {
      const item = w.querySelector(':scope > .file-tree-item');
      if (item) item.classList.add('file-tree-item--cut');
    }
  }
}

// Re-apply cut styling whenever the clipboard changes (e.g. after a paste
// that consumed the clipboard, or after the user cuts a different set).
clipSubscribe(refreshClipboardCutUI);

// Also re-apply on DOM mutations under the explorer so newly-rendered items
// pick up the cut styling without us having to thread state through every
// render path.
const cutObserver = new MutationObserver(() => refreshClipboardCutUI());
// Defer until DOMContentLoaded so document.body exists.
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => {
    cutObserver.observe(document.body, { childList: true, subtree: true });
  });
} else {
  cutObserver.observe(document.body, { childList: true, subtree: true });
}


registerCommand({
  id: 'explorer.rename',
  title: 'Rename',
  category: 'Explorer',
  run: () => {
    // Rename is only meaningful for a single entry — prefer multi-selection,
    // fall back to the last focused (single-clicked) item.
    let path, info;
    const entries = Array.from(selectedPaths.entries());
    if (entries.length === 1) {
      [path, info] = entries[0];
    } else if (entries.length === 0 && lastFocusedNode) {
      path = lastFocusedNode.path;
      info = lastFocusedNode;
    } else {
      return;
    }
    const wrapper = findWrapperByPath(path);
    if (!wrapper) return;
    const item = wrapper.querySelector(':scope > .file-tree-item');
    const nameEl = item?.querySelector('.file-tree-item__name');
    if (!item || !nameEl) return;
    const node = { path, name: info.name, is_dir: info.is_dir };
    startInlineRename(item, nameEl, node, async (newName) => {
      try {
        await api.renameEntry(path, newName);
        await refreshAffectedDirectory(path);
      } catch (err) {
        console.error('Rename failed:', err);
      }
    });
  },
});

async function deleteSelectedPaths() {
  let entries;
  if (selectedPaths.size > 0) {
    entries = Array.from(selectedPaths.entries());
  } else if (lastFocusedNode) {
    entries = [[lastFocusedNode.path, lastFocusedNode]];
  } else {
    return;
  }
  const names = entries.map(([, info]) => info.name);

  const listing = names.length <= 5
    ? names.map(n => `  • ${n}`).join('\n')
    : names.slice(0, 4).map(n => `  • ${n}`).join('\n') + `\n  … and ${names.length - 4} more`;

  const label = entries.length === 1 ? `"${names[0]}"` : `${entries.length} items`;
  const confirmed = await showConfirmDialog(
    'Delete',
    `Are you sure you want to delete ${label}?\n\n${listing}`,
  );
  if (!confirmed) return;

  for (const [path] of entries) {
    try {
      await api.deleteEntry(path);
      await closeBuffersForPath(path);
    } catch (err) {
      console.error('Delete failed for', path, err);
    }
  }
  clearSelection();
  for (const [path] of entries) {
    await refreshAffectedDirectory(path);
  }
}

function getParentDir(filePath) {
  return filePath.replace(/[\\/][^\\/]+$/, '');
}

function getRelativePath(filePath, projectName) {
  const projects = workspaceStore.getState('projects');
  const project = projects.find(p => p.name === projectName);
  if (project && filePath.startsWith(project.root_path)) {
    return filePath.substring(project.root_path.length).replace(/^[\\/]/, '');
  }
  return filePath;
}

// Simple file extension to icon color mapping
const EXT_COLORS = {
  js: 'var(--bright-yellow)',
  ts: 'var(--bright-blue)',
  jsx: 'var(--bright-yellow)',
  tsx: 'var(--bright-blue)',
  rs: 'var(--bright-orange)',
  py: 'var(--bright-green)',
  go: 'var(--bright-aqua)',
  json: 'var(--bright-yellow)',
  toml: 'var(--bright-orange)',
  md: 'var(--bright-blue)',
  css: 'var(--bright-purple)',
  html: 'var(--bright-red)',
  svg: 'var(--bright-orange)',
  lock: 'var(--fg4)',
};

// ===================== FILE/FOLDER CREATION =====================

function openCreatedFile(fullPath, name, projectName) {
  window.dispatchEvent(new CustomEvent('rustic:open-file', {
    detail: { path: fullPath, name, projectName },
  }));
}

/**
 * Create a file, refresh the parent cache, and return the created path.
 * Callers should dispatch rustic:open-file AFTER rebuilding the tree.
 */
async function doCreateFile(dirPath, name) {
  try {
    const fullPath = await api.createFile(dirPath, name);
    if (fullPath) {
      clearChildrenCache(dirPath);
      await loadChildren(dirPath);
    }
    return fullPath || null;
  } catch (e) {
    console.error('Failed to create file:', e);
    return null;
  }
}

async function doCreateFolder(dirPath, name) {
  try {
    await api.createFolder(dirPath, name);
    clearChildrenCache(dirPath);
    await loadChildren(dirPath);
  } catch (e) {
    console.error('Failed to create folder:', e);
  }
}

// ===================== INLINE INPUT (VS Code style) =====================

export function insertInlineInput(container, depth, isFolder, onSubmit) {
  // Remove any existing inline input
  const existing = container.querySelector('.inline-input-wrapper');
  if (existing) existing.remove();

  const wrapper = el('div', { class: 'file-tree-item-wrapper inline-input-wrapper' });
  const item = el('div', {
    class: 'file-tree-item',
    style: { paddingLeft: (depth + 1) * INDENT_PX + 'px' },
  });

  const spacer = el('span', { class: 'file-tree-item__spacer' });
  const iconEl = el('span', {
    class: isFolder ? 'file-tree-item__icon file-tree-item__icon--folder' : 'file-tree-item__icon',
  });
  iconEl.appendChild(icon(
    isFolder
      ? 'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z'
      : 'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z',
    14,
  ));

  const input = el('input', {
    class: 'file-tree-inline-input',
    type: 'text',
    spellcheck: 'false',
    autocomplete: 'off',
  });

  item.appendChild(spacer);
  item.appendChild(iconEl);
  item.appendChild(input);
  wrapper.appendChild(item);

  container.insertBefore(wrapper, container.firstChild);
  input.focus();

  let done = false;
  const cleanup = () => {
    if (done) return;
    done = true;
    wrapper.remove();
  };

  input.addEventListener('keydown', async (e) => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      const name = input.value.trim();
      if (name) {
        done = true;
        wrapper.remove();
        await onSubmit(name);
      } else {
        cleanup();
      }
    } else if (e.key === 'Escape') {
      cleanup();
    }
  });

  input.addEventListener('blur', () => setTimeout(cleanup, 150));
}

// ===================== INLINE RENAME =====================

function startInlineRename(item, nameEl, node, onSubmit) {
  const input = el('input', {
    class: 'file-tree-inline-input',
    type: 'text',
    value: node.name,
    spellcheck: 'false',
    autocomplete: 'off',
  });

  nameEl.style.display = 'none';
  const actionsEl = item.querySelector('.file-tree-item__actions');
  if (actionsEl) actionsEl.style.display = 'none';
  nameEl.parentNode.insertBefore(input, nameEl.nextSibling);
  input.focus();

  // Select name without extension for files
  if (!node.is_dir) {
    const dot = node.name.lastIndexOf('.');
    input.setSelectionRange(0, dot > 0 ? dot : node.name.length);
  } else {
    input.select();
  }

  let done = false;
  const cleanup = () => {
    if (done) return;
    done = true;
    input.remove();
    nameEl.style.display = '';
    if (actionsEl) actionsEl.style.display = '';
  };

  input.addEventListener('keydown', async (e) => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      const newName = input.value.trim();
      if (newName && newName !== node.name) {
        done = true;
        input.remove();
        await onSubmit(newName);
      } else {
        cleanup();
      }
    } else if (e.key === 'Escape') {
      cleanup();
    }
  });

  input.addEventListener('blur', () => setTimeout(cleanup, 150));
}

// ===================== HELPERS =====================

async function ensureExpanded(wrapper, node, depth, projectName, caret) {
  if (expandedDirs.has(node.path)) return;
  expandedDirs.add(node.path);
  await loadChildren(node.path);
  const tree = createFileTree(node.path, depth + 1, projectName);
  wrapper.appendChild(tree);
  if (caret) {
    caret.innerHTML = '';
    caret.appendChild(icon('M6 9l6 6 6-6', 12));
  }
}

// ===================== REVEAL FILE IN EXPLORER =====================

function findWrapperByPath(path) {
  const wrappers = document.querySelectorAll('.file-tree-item-wrapper[data-path]');
  for (const w of wrappers) {
    if (w.dataset.path === path) return w;
  }
  return null;
}

function highlightAndScroll(wrapper) {
  const item = wrapper.querySelector('.file-tree-item');
  if (item) {
    item.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    item.classList.add('file-tree-item--revealed');
    setTimeout(() => item.classList.remove('file-tree-item--revealed'), 1500);
  }
}

let revealGeneration = 0;

export async function revealFileInExplorer(filePath) {
  const gen = ++revealGeneration;
  debug('FileTree', 'revealFileInExplorer', { filePath, gen });
  const projects = workspaceStore.getState('projects');

  // Find which project owns this file
  let project = null;
  for (const p of projects) {
    if (filePath.startsWith(p.root_path)) {
      project = p;
      break;
    }
  }
  if (!project) return;

  // Ensure project section is expanded (this is the only setState we do)
  if (!project.isExpanded) {
    toggleProject(project.id);
    await new Promise(r => setTimeout(r, 50));
    if (gen !== revealGeneration) return;
  }

  // Check if file is already visible in DOM — fast path
  let fileWrapper = findWrapperByPath(filePath);
  if (fileWrapper) {
    highlightAndScroll(fileWrapper);
    return;
  }

  // File not visible — expand ancestor folders one by one in the DOM
  let relative = filePath.substring(project.root_path.length);
  relative = relative.replace(/\\/g, '/');
  const segments = relative.split('/').filter(Boolean);
  segments.pop(); // Remove file name

  const sep = project.root_path.includes('\\') ? '\\' : '/';
  let currentPath = project.root_path;

  for (const segment of segments) {
    currentPath = currentPath + sep + segment;
    if (gen !== revealGeneration) return;

    // Find the folder's wrapper in the DOM
    let folderWrapper = findWrapperByPath(currentPath);
    if (!folderWrapper) {
      // Folder not in DOM yet — wait for any pending async render
      await new Promise(r => setTimeout(r, 60));
      if (gen !== revealGeneration) return;
      folderWrapper = findWrapperByPath(currentPath);
      if (!folderWrapper) return; // Still not found, give up
    }

    // Check if already expanded (has a child .file-tree)
    if (!folderWrapper.querySelector(':scope > .file-tree')) {
      // Expand this folder in-place
      expandedDirs.add(currentPath);
      await loadChildren(currentPath);
      if (gen !== revealGeneration) return;

      // Compute depth from the item's padding
      const item = folderWrapper.querySelector('.file-tree-item');
      const paddingPx = parseInt(item?.style.paddingLeft) || INDENT_PX;
      const depth = Math.round(paddingPx / INDENT_PX) - 1;

      const tree = createFileTree(currentPath, depth + 1, project.name);
      folderWrapper.appendChild(tree);

      // Update caret icon to expanded
      const caret = folderWrapper.querySelector('.file-tree-item__caret');
      if (caret) {
        caret.innerHTML = '';
        caret.appendChild(icon('M6 9l6 6 6-6', 12));
      }

      // Wait for tree to render
      await new Promise(r => setTimeout(r, 10));
      if (gen !== revealGeneration) return;
    }
  }

  // Now find and highlight the file
  fileWrapper = findWrapperByPath(filePath);
  if (fileWrapper) {
    highlightAndScroll(fileWrapper);
  }
}

// ===================== MAIN EXPORT =====================

/** Render vertical indent guide lines for a given depth. */
function renderIndentGuides(depth) {
  if (depth <= 0) return null;
  const guides = el('div', { class: 'indent-guides' });
  for (let i = 1; i <= depth; i++) {
    const line = el('div', { class: 'indent-guides__line' });
    // Each guide sits at the left edge of its indent level's column
    line.style.left = (i * INDENT_PX + Math.floor(INDENT_PX / 2)) + 'px';
    guides.appendChild(line);
  }
  return guides;
}

export function createFileTreeItem(node, depth, projectName) {
  const wrapper = el('div', { class: 'file-tree-item-wrapper', dataset: { path: node.path } });
  const parentDir = getParentDir(node.path);

  const item = el('div', {
    class: `file-tree-item ${node.is_dir ? 'file-tree-item--dir' : 'file-tree-item--file'}`,
    style: { paddingLeft: (depth + 1) * INDENT_PX + 'px' },
  });

  // Add indent guide lines
  const guides = renderIndentGuides(depth);
  if (guides) item.appendChild(guides);

  if (node.is_dir) {
    const isExpanded = expandedDirs.has(node.path);

    // Caret
    const caret = el('span', { class: 'file-tree-item__caret' });
    caret.appendChild(icon(isExpanded ? 'M6 9l6 6 6-6' : 'M9 18l6-6-6-6', 12));

    // Folder icon
    const folderIcon = el('span', { class: 'file-tree-item__icon file-tree-item__icon--folder' });
    folderIcon.appendChild(icon(
      'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z',
      14,
    ));

    const name = el('span', { class: 'file-tree-item__name' }, node.name);

    // Hover action buttons for folders
    const actions = el('div', { class: 'file-tree-item__actions' });

    const newFileBtn = el('button', { title: 'New File' });
    newFileBtn.appendChild(icon('M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M12 18v-6M9 15h6', 12));
    newFileBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      await ensureExpanded(wrapper, node, depth, projectName, caret);
      const fileTree = wrapper.querySelector(':scope > .file-tree');
      if (fileTree) {
        insertInlineInput(fileTree, depth + 1, false, async (fileName) => {
          const created = await doCreateFile(node.path, fileName);
          await reloadChildren(wrapper, node, depth, projectName);
          if (created) openCreatedFile(created, fileName, projectName);
        });
      }
    });

    const newFolderBtn = el('button', { title: 'New Folder' });
    newFolderBtn.appendChild(icon('M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM12 11v6M9 14h6', 12));
    newFolderBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      await ensureExpanded(wrapper, node, depth, projectName, caret);
      const fileTree = wrapper.querySelector(':scope > .file-tree');
      if (fileTree) {
        insertInlineInput(fileTree, depth + 1, true, async (folderName) => {
          await doCreateFolder(node.path, folderName);
          await reloadChildren(wrapper, node, depth, projectName);
        });
      }
    });

    const termBtn = el('button', { title: 'Open Terminal Here' });
    termBtn.appendChild(icon('M4 17l6-6-6-6M12 19h8', 12));
    termBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      createTerminal(node.path, node.name);
    });

    actions.appendChild(newFileBtn);
    actions.appendChild(newFolderBtn);
    actions.appendChild(termBtn);

    item.appendChild(caret);
    item.appendChild(folderIcon);
    item.appendChild(name);
    item.appendChild(actions);

    item.addEventListener('click', async (e) => {
      if (e.ctrlKey || e.metaKey) {
        e.stopPropagation();
        toggleSelection(node.path, { name: node.name, is_dir: true, projectName });
        lastFocusedNode = { path: node.path, name: node.name, is_dir: true, projectName };
        return;
      }
      clearSelection();
      lastFocusedNode = { path: node.path, name: node.name, is_dir: true, projectName };
      if (expandedDirs.has(node.path)) {

        expandedDirs.delete(node.path);
        const childContainer = wrapper.querySelector(':scope > .file-tree');
        if (childContainer) childContainer.remove();
        caret.innerHTML = '';
        caret.appendChild(icon('M9 18l6-6-6-6', 12));
      } else {
        expandedDirs.add(node.path);
        await loadChildren(node.path);
        // Guard: user may have collapsed while loading
        if (!expandedDirs.has(node.path)) return;
        // Guard: tree may already exist from another path
        if (wrapper.querySelector(':scope > .file-tree')) return;
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
        caret.innerHTML = '';
        caret.appendChild(icon('M6 9l6 6 6-6', 12));
      }
    });

    // Append the folder item BEFORE any subtree so the DOM order is correct
    wrapper.appendChild(item);

    // If already expanded (e.g. after re-render), render children immediately
    if (isExpanded) {
      const cached = getCachedChildren(node.path);
      if (cached) {
        debug('FileTree', 'expanded folder SYNC', { node: node.name, depth });
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
      } else {
        debug('FileTree', 'expanded folder ASYNC', { node: node.name, depth });
        loadChildren(node.path).then(() => {
          if (!expandedDirs.has(node.path)) return;
          if (wrapper.querySelector(':scope > .file-tree')) return;
          debug('FileTree', 'expanded folder ASYNC resolved', { node: node.name, depth, wrapperInDOM: document.body.contains(wrapper) });
          const tree = createFileTree(node.path, depth + 1, projectName);
          wrapper.appendChild(tree);
        });
      }
    }
  } else {
    // File icon
    const ext = node.name.split('.').pop().toLowerCase();
    const color = EXT_COLORS[ext] || 'var(--fg4)';

    const fileIcon = el('span', { class: 'file-tree-item__icon' });
    const svg = icon('M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z', 14);
    svg.style.color = color;
    fileIcon.appendChild(svg);

    const name = el('span', { class: 'file-tree-item__name' }, node.name);

    // Spacer for alignment with folders (caret width)
    const spacer = el('span', { class: 'file-tree-item__spacer' });

    // Hover action buttons for files (creates in parent directory)
    const actions = el('div', { class: 'file-tree-item__actions' });

    const newFileBtn = el('button', { title: 'New File' });
    newFileBtn.appendChild(icon('M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M12 18v-6M9 15h6', 12));
    newFileBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const parentFileTree = wrapper.parentElement;
      if (parentFileTree && parentFileTree.classList.contains('file-tree')) {
        insertInlineInput(parentFileTree, depth, false, async (fileName) => {
          const created = await doCreateFile(parentDir, fileName);
          if (created) {
            await refreshAffectedDirectory(created);
            openCreatedFile(created, fileName, projectName);
          }
        });
      }
    });

    const newFolderBtn = el('button', { title: 'New Folder' });
    newFolderBtn.appendChild(icon('M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM12 11v6M9 14h6', 12));
    newFolderBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const parentFileTree = wrapper.parentElement;
      if (parentFileTree && parentFileTree.classList.contains('file-tree')) {
        insertInlineInput(parentFileTree, depth, true, async (folderName) => {
          await doCreateFolder(parentDir, folderName);
          const sep = parentDir.includes('/') ? '/' : '\\';
          await refreshAffectedDirectory(parentDir + sep + folderName);
        });
      }
    });

    actions.appendChild(newFileBtn);
    actions.appendChild(newFolderBtn);

    item.appendChild(spacer);
    item.appendChild(fileIcon);
    item.appendChild(name);
    item.appendChild(actions);

    item.addEventListener('click', (e) => {
      if (e.ctrlKey || e.metaKey) {
        e.stopPropagation();
        toggleSelection(node.path, { name: node.name, is_dir: false, projectName });
        lastFocusedNode = { path: node.path, name: node.name, is_dir: false, projectName };
        return;
      }
      clearSelection();
      lastFocusedNode = { path: node.path, name: node.name, is_dir: false, projectName };
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: node.path, name: node.name, projectName },
      }));
    });


    // Make files draggable into editor groups
    item.draggable = true;
    item.addEventListener('dragstart', (e) => {
      const payload = JSON.stringify({
        __rustic: 'file',
        path: node.path,
        name: node.name,
        projectName,
      });
      e.dataTransfer.setData('text/plain', payload);
      e.dataTransfer.effectAllowed = 'copyMove';
      setDragType('file');
      debug('DnD', 'file dragstart', { path: node.path, effectAllowed: e.dataTransfer.effectAllowed, types: Array.from(e.dataTransfer.types) });
    });
    item.addEventListener('dragend', (e) => {
      debug('DnD', 'file dragend', { dropEffect: e.dataTransfer.dropEffect });
      clearDragType();
    });
  }

  // ===================== CONTEXT MENU =====================
  item.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();

    // Track this as the "active" item for clipboard ops triggered from the
    // keyboard after the menu closes (e.g. user right-clicks → Cut, then
    // Ctrl+V on a different folder).
    lastFocusedNode = { path: node.path, name: node.name, is_dir: node.is_dir, projectName };

    // Multi-select context menu: show when right-clicking on a selected item with 2+ selections
    if (selectedPaths.size >= 2 && selectedPaths.has(node.path)) {

      const entries = Array.from(selectedPaths.entries());
      const files = entries.filter(([, info]) => !info.is_dir);
      const sources = entries.map(([path, info]) => ({
        path, name: info.name, is_dir: info.is_dir, projectName: info.projectName,
      }));
      const menuItems = [];

      if (files.length > 0) {
        menuItems.push({
          label: `Open ${files.length} File${files.length > 1 ? 's' : ''} to Side`,
          action: () => {
            for (const [path, info] of files) {
              window.dispatchEvent(new CustomEvent('rustic:open-file', {
                detail: { path, name: info.name, projectName: info.projectName },
              }));
            }
            clearSelection();
          },
        });
        menuItems.push({ separator: true });
      }

      menuItems.push(
        {
          label: `Cut ${entries.length} Item${entries.length > 1 ? 's' : ''}`,
          shortcut: 'Ctrl+X',
          action: () => clipCutItems(sources),
        },
        {
          label: `Copy ${entries.length} Item${entries.length > 1 ? 's' : ''}`,
          shortcut: 'Ctrl+C',
          action: () => clipCopyItems(sources),
        },
        { separator: true },
      );

      menuItems.push({
        label: `Delete ${entries.length} Item${entries.length > 1 ? 's' : ''}`,
        action: () => deleteSelectedPaths(),
      });

      menuItems.push({ separator: true });

      menuItems.push({
        label: `Reveal ${entries.length} in File Manager`,
        action: () => {
          for (const [path] of entries) {
            api.revealInFileManager(path).catch((err) => console.error('Reveal failed:', err));
          }
          clearSelection();
        },
      });

      showContextMenu(menuItems, e.clientX, e.clientY);
      return;
    }


    // Single-item right-click: clear multi-selection and show normal menu
    clearSelection();

    const menuItems = [];

    const selfSource = { path: node.path, name: node.name, is_dir: node.is_dir, projectName };

    if (node.is_dir) {
      const caret = item.querySelector('.file-tree-item__caret');
      menuItems.push(
        { label: 'New File...', action: async () => {
          await ensureExpanded(wrapper, node, depth, projectName, caret);
          const fileTree = wrapper.querySelector(':scope > .file-tree');
          if (fileTree) {
            insertInlineInput(fileTree, depth + 1, false, async (fileName) => {
              const created = await doCreateFile(node.path, fileName);
              await reloadChildren(wrapper, node, depth, projectName);
              if (created) openCreatedFile(created, fileName, projectName);
            });
          }
        }},
        { label: 'New Folder...', action: async () => {
          await ensureExpanded(wrapper, node, depth, projectName, caret);
          const fileTree = wrapper.querySelector(':scope > .file-tree');
          if (fileTree) {
            insertInlineInput(fileTree, depth + 1, true, async (folderName) => {
              await doCreateFolder(node.path, folderName);
              await reloadChildren(wrapper, node, depth, projectName);
            });
          }
        }},
        { separator: true },
      );
    } else {
      menuItems.push(
        { label: 'Open File', action: () => {
          window.dispatchEvent(new CustomEvent('rustic:open-file', {
            detail: { path: node.path, name: node.name, projectName },
          }));
        }},
        { separator: true },
        { label: 'New File...', action: () => {
          const pft = wrapper.parentElement;
          if (pft && pft.classList.contains('file-tree')) {
            insertInlineInput(pft, depth, false, async (fileName) => {
              const created = await doCreateFile(parentDir, fileName);
              if (created) {
                await refreshAffectedDirectory(created);
                openCreatedFile(created, fileName, projectName);
              }
            });
          }
        }},
        { label: 'New Folder...', action: () => {
          const pft = wrapper.parentElement;
          if (pft && pft.classList.contains('file-tree')) {
            insertInlineInput(pft, depth, true, async (folderName) => {
              await doCreateFolder(parentDir, folderName);
              // Use parentDir + folderName as a path for refreshAffectedDirectory
              const sep = parentDir.includes('/') ? '/' : '\\';
              await refreshAffectedDirectory(parentDir + sep + folderName);
            });
          }
        }},
        { separator: true },
      );
    }

    // Cut/Copy/Paste — sit between New… and Copy Path so the menu mirrors
    // the OS file-manager grouping (clipboard ops first, then path ops).
    // Paste is always enabled because if the internal clipboard is empty
    // we fall back to reading absolute paths off the OS clipboard (e.g. a
    // path the user copied from Windows Explorer's "Copy as path", VS Code,
    // or another instance of this app).
    menuItems.push(
      { label: 'Cut', shortcut: 'Ctrl+X', action: () => clipCutItems([selfSource]) },
      { label: 'Copy', shortcut: 'Ctrl+C', action: () => clipCopyItems([selfSource]) },
      {
        label: 'Paste',
        shortcut: 'Ctrl+V',
        action: () => pasteAtTarget(resolvePasteTargetDir(selfSource)),
      },

      { separator: true },
    );


    menuItems.push(
      { label: 'Copy Path', action: () => navigator.clipboard.writeText(node.path) },
      { label: 'Copy Relative Path', action: () => navigator.clipboard.writeText(getRelativePath(node.path, projectName)) },
      { label: 'Copy Name', action: () => navigator.clipboard.writeText(node.name) },
      { separator: true },
      { label: 'Rename', action: () => {
        const nameEl = item.querySelector('.file-tree-item__name');
        startInlineRename(item, nameEl, node, async (newName) => {
          try {
            await api.renameEntry(node.path, newName);
            await refreshAffectedDirectory(node.path);
          } catch (err) {
            console.error('Rename failed:', err);
          }
        });
      }},
      { label: 'Delete', action: async () => {
        // Select this item so deleteSelectedPaths() handles it uniformly
        selectedPaths.set(node.path, { name: node.name, is_dir: node.is_dir, projectName });
        await deleteSelectedPaths();
      }},
      { separator: true },
      { label: 'Reveal in File Manager', action: () => {
        api.revealInFileManager(node.path).catch((e) => console.error('Reveal failed:', e));
      }},
    );


    showContextMenu(menuItems, e.clientX, e.clientY);
  });

  // For files, append item here. For directories, it was already appended
  // before the isExpanded block to ensure correct DOM order (item before subtree).
  if (!node.is_dir) {
    wrapper.appendChild(item);
  }
  return wrapper;
}

// ===================== RELOAD HELPERS =====================

async function reloadChildren(wrapper, node, depth, projectName) {
  debug('FileTree', 'reloadChildren', { path: node.path, depth, wrapperInDOM: document.body.contains(wrapper) });
  if (!expandedDirs.has(node.path)) {
    expandedDirs.add(node.path);
  }
  const oldTree = wrapper.querySelector(':scope > .file-tree');
  debug('FileTree', 'reloadChildren removing old tree', { hadOldTree: !!oldTree });
  if (oldTree) oldTree.remove();
  const tree = createFileTree(node.path, depth + 1, projectName);
  wrapper.appendChild(tree);
  const caret = wrapper.querySelector('.file-tree-item__caret');
  if (caret) {
    caret.innerHTML = '';
    caret.appendChild(icon('M6 9l6 6 6-6', 12));
  }
}
