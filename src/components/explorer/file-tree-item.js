import { el, icon } from '../../utils/dom.js';
import { loadChildren, getCachedChildren, clearChildrenCache, workspaceStore, toggleProject, expandedDirs, refreshAffectedDirectory, refreshProject } from '../../state/workspace.js';
import { showConfirmDialog } from '../confirm-dialog.js';
import { createFileTree } from './file-tree.js';
import { showContextMenu } from '../dropdown-menu.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { setDragType, clearDragType } from '../../utils/drag-state.js';
import { closeBuffersForPath } from '../../state/editor.js';

// expandedDirs is imported from workspace.js (shared state)

/** Pixels per indent level. Keep in sync with CSS and file-tree.js. */
export const INDENT_PX = 12;

// ===================== MULTI-SELECT STATE =====================

/** Map<path, { name, is_dir, projectName }> */
const selectedPaths = new Map();

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

// Clear selection when clicking empty explorer area
document.addEventListener('click', (e) => {
  if (selectedPaths.size === 0) return;
  // If click is inside a file-tree-item, let the item handler deal with it
  if (e.target.closest('.file-tree-item')) return;
  clearSelection();
});

// Delete selected files/folders with the Delete key
document.addEventListener('keydown', async (e) => {
  if (e.key !== 'Delete') return;
  if (selectedPaths.size === 0) return;
  // Don't interfere with text editing in inputs / textareas / contenteditable
  const tag = document.activeElement?.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || document.activeElement?.isContentEditable) return;

  await deleteSelectedPaths();
});

async function deleteSelectedPaths() {
  if (selectedPaths.size === 0) return;

  const entries = Array.from(selectedPaths.entries());
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
  console.log('[FileTree] revealFileInExplorer path=%s gen=%d', filePath, gen);
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
        return;
      }
      clearSelection();
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
        console.log('[FileTree] expanded folder SYNC node=%s depth=%d', node.name, depth);
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
      } else {
        console.log('[FileTree] expanded folder ASYNC node=%s depth=%d', node.name, depth);
        loadChildren(node.path).then(() => {
          // Guard: directory may have been collapsed while loading
          if (!expandedDirs.has(node.path)) return;
          // Guard: tree may have already been appended by another path
          if (wrapper.querySelector(':scope > .file-tree')) return;
          console.log('[FileTree] expanded folder ASYNC resolved node=%s depth=%d wrapperInDOM=%s', node.name, depth, document.body.contains(wrapper));
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
        return;
      }
      clearSelection();
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
      console.log('[DnD] file dragstart', { path: node.path, effectAllowed: e.dataTransfer.effectAllowed, types: Array.from(e.dataTransfer.types) });
    });
    item.addEventListener('dragend', (e) => {
      console.log('[DnD] file dragend', { dropEffect: e.dataTransfer.dropEffect });
      clearDragType();
    });
  }

  // ===================== CONTEXT MENU =====================
  item.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();

    // Multi-select context menu: show when right-clicking on a selected item with 2+ selections
    if (selectedPaths.size >= 2 && selectedPaths.has(node.path)) {
      const entries = Array.from(selectedPaths.entries());
      const files = entries.filter(([, info]) => !info.is_dir);
      const names = entries.map(([, info]) => info.name);
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
  console.log('[FileTree] reloadChildren node=%s depth=%d wrapperInDOM=%s', node.path, depth, document.body.contains(wrapper));
  if (!expandedDirs.has(node.path)) {
    expandedDirs.add(node.path);
  }
  const oldTree = wrapper.querySelector(':scope > .file-tree');
  console.log('[FileTree] reloadChildren removing old tree=%s', !!oldTree);
  if (oldTree) oldTree.remove();
  const tree = createFileTree(node.path, depth + 1, projectName);
  wrapper.appendChild(tree);
  const caret = wrapper.querySelector('.file-tree-item__caret');
  if (caret) {
    caret.innerHTML = '';
    caret.appendChild(icon('M6 9l6 6 6-6', 12));
  }
}
