import { el, icon } from '../../utils/dom.js';
import { loadChildren, getCachedChildren, clearChildrenCache, workspaceStore } from '../../state/workspace.js';
import { createFileTree } from './file-tree.js';
import { showContextMenu } from '../dropdown-menu.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';

// Track expanded state per path
const expandedDirs = new Set();

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

async function doCreateFile(dirPath, name, projectName) {
  try {
    const fullPath = await api.createFile(dirPath, name);
    if (fullPath) {
      clearChildrenCache(dirPath);
      await loadChildren(dirPath);
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: fullPath, name, projectName },
      }));
    }
  } catch (e) {
    console.error('Failed to create file:', e);
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
    style: { paddingLeft: (depth + 1) * 16 + 'px' },
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

function refreshParentTree(wrapper, parentDir, depth, projectName) {
  const parentFileTree = wrapper.parentElement;
  if (parentFileTree && parentFileTree.classList.contains('file-tree')) {
    const newTree = createFileTree(parentDir, depth, projectName);
    parentFileTree.replaceWith(newTree);
  }
}

// ===================== MAIN EXPORT =====================

export function createFileTreeItem(node, depth, projectName) {
  const wrapper = el('div', { class: 'file-tree-item-wrapper' });
  const parentDir = getParentDir(node.path);

  const item = el('div', {
    class: `file-tree-item ${node.is_dir ? 'file-tree-item--dir' : 'file-tree-item--file'}`,
    style: { paddingLeft: (depth + 1) * 16 + 'px' },
  });

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
      const fileTree = wrapper.querySelector('.file-tree');
      if (fileTree) {
        insertInlineInput(fileTree, depth + 1, false, async (fileName) => {
          await doCreateFile(node.path, fileName, projectName);
          reloadChildren(wrapper, node, depth, projectName);
        });
      }
    });

    const newFolderBtn = el('button', { title: 'New Folder' });
    newFolderBtn.appendChild(icon('M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM12 11v6M9 14h6', 12));
    newFolderBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      await ensureExpanded(wrapper, node, depth, projectName, caret);
      const fileTree = wrapper.querySelector('.file-tree');
      if (fileTree) {
        insertInlineInput(fileTree, depth + 1, true, async (folderName) => {
          await doCreateFolder(node.path, folderName);
          reloadChildren(wrapper, node, depth, projectName);
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

    item.addEventListener('click', async () => {
      if (expandedDirs.has(node.path)) {
        expandedDirs.delete(node.path);
        const childContainer = wrapper.querySelector('.file-tree');
        if (childContainer) childContainer.remove();
        caret.innerHTML = '';
        caret.appendChild(icon('M9 18l6-6-6-6', 12));
      } else {
        expandedDirs.add(node.path);
        await loadChildren(node.path);
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
        caret.innerHTML = '';
        caret.appendChild(icon('M6 9l6 6 6-6', 12));
      }
    });

    // If already expanded (e.g. after re-render), render children immediately
    if (isExpanded) {
      const cached = getCachedChildren(node.path);
      if (cached) {
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
      } else {
        loadChildren(node.path).then(() => {
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
          await doCreateFile(parentDir, fileName, projectName);
          refreshParentTree(wrapper, parentDir, depth, projectName);
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
          refreshParentTree(wrapper, parentDir, depth, projectName);
        });
      }
    });

    actions.appendChild(newFileBtn);
    actions.appendChild(newFolderBtn);

    item.appendChild(spacer);
    item.appendChild(fileIcon);
    item.appendChild(name);
    item.appendChild(actions);

    item.addEventListener('click', () => {
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: node.path, name: node.name, projectName },
      }));
    });
  }

  // ===================== CONTEXT MENU =====================
  item.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const menuItems = [];

    if (node.is_dir) {
      const caret = item.querySelector('.file-tree-item__caret');
      menuItems.push(
        { label: 'New File...', action: async () => {
          await ensureExpanded(wrapper, node, depth, projectName, caret);
          const fileTree = wrapper.querySelector('.file-tree');
          if (fileTree) {
            insertInlineInput(fileTree, depth + 1, false, async (fileName) => {
              await doCreateFile(node.path, fileName, projectName);
              reloadChildren(wrapper, node, depth, projectName);
            });
          }
        }},
        { label: 'New Folder...', action: async () => {
          await ensureExpanded(wrapper, node, depth, projectName, caret);
          const fileTree = wrapper.querySelector('.file-tree');
          if (fileTree) {
            insertInlineInput(fileTree, depth + 1, true, async (folderName) => {
              await doCreateFolder(node.path, folderName);
              reloadChildren(wrapper, node, depth, projectName);
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
              await doCreateFile(parentDir, fileName, projectName);
              refreshParentTree(wrapper, parentDir, depth, projectName);
            });
          }
        }},
        { label: 'New Folder...', action: () => {
          const pft = wrapper.parentElement;
          if (pft && pft.classList.contains('file-tree')) {
            insertInlineInput(pft, depth, true, async (folderName) => {
              await doCreateFolder(parentDir, folderName);
              refreshParentTree(wrapper, parentDir, depth, projectName);
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
            clearChildrenCache(parentDir);
            await loadChildren(parentDir);
            refreshParentTree(wrapper, parentDir, depth, projectName);
          } catch (err) {
            console.error('Rename failed:', err);
          }
        });
      }},
      { label: 'Delete', action: async () => {
        if (!confirm(`Are you sure you want to delete "${node.name}"?`)) return;
        try {
          await api.deleteEntry(node.path);
          clearChildrenCache(parentDir);
          await loadChildren(parentDir);
          refreshParentTree(wrapper, parentDir, depth, projectName);
        } catch (err) {
          console.error('Delete failed:', err);
        }
      }},
      { separator: true },
      { label: 'Reveal in File Manager', action: () => {
        api.revealInFileManager(node.path).catch((e) => console.error('Reveal failed:', e));
      }},
    );

    showContextMenu(menuItems, e.clientX, e.clientY);
  });

  wrapper.appendChild(item);
  return wrapper;
}

// ===================== RELOAD HELPERS =====================

async function reloadChildren(wrapper, node, depth, projectName) {
  if (!expandedDirs.has(node.path)) {
    expandedDirs.add(node.path);
  }
  const oldTree = wrapper.querySelector('.file-tree');
  if (oldTree) oldTree.remove();
  const tree = createFileTree(node.path, depth + 1, projectName);
  wrapper.appendChild(tree);
  const caret = wrapper.querySelector('.file-tree-item__caret');
  if (caret) {
    caret.innerHTML = '';
    caret.appendChild(icon('M6 9l6 6 6-6', 12));
  }
}
