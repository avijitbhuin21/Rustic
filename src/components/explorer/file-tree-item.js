import { el, icon } from '../../utils/dom.js';
import { loadChildren, getCachedChildren, clearChildrenCache } from '../../state/workspace.js';
import { createFileTree } from './file-tree.js';
import { showContextMenu } from '../dropdown-menu.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';

// Track expanded state per path
const expandedDirs = new Set();

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

async function promptAndCreateFile(dirPath, projectName) {
  const name = prompt('New file name:');
  if (!name) return;
  try {
    const fullPath = await api.createFile(dirPath, name);
    if (fullPath) {
      clearChildrenCache(dirPath);
      await loadChildren(dirPath);
      // Open the new file
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: fullPath, name, projectName },
      }));
    }
  } catch (e) {
    console.error('Failed to create file:', e);
  }
}

async function promptAndCreateFolder(dirPath) {
  const name = prompt('New folder name:');
  if (!name) return;
  try {
    await api.createFolder(dirPath, name);
    clearChildrenCache(dirPath);
    await loadChildren(dirPath);
  } catch (e) {
    console.error('Failed to create folder:', e);
  }
}

export function createFileTreeItem(node, depth, projectName) {
  const wrapper = el('div', { class: 'file-tree-item-wrapper' });

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
    newFileBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      promptAndCreateFile(node.path, projectName).then(() => reloadChildren(wrapper, node, depth, projectName));
    });

    const newFolderBtn = el('button', { title: 'New Folder' });
    newFolderBtn.appendChild(icon('M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM12 11v6M9 14h6', 12));
    newFolderBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      promptAndCreateFolder(node.path).then(() => reloadChildren(wrapper, node, depth, projectName));
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
        // Remove children container
        const childContainer = wrapper.querySelector('.file-tree');
        if (childContainer) childContainer.remove();
        // Update caret
        caret.innerHTML = '';
        caret.appendChild(icon('M9 18l6-6-6-6', 12));
      } else {
        expandedDirs.add(node.path);
        // Load and render children
        await loadChildren(node.path);
        const tree = createFileTree(node.path, depth + 1, projectName);
        wrapper.appendChild(tree);
        // Update caret
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
        // Need to load then render
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

    item.appendChild(spacer);
    item.appendChild(fileIcon);
    item.appendChild(name);

    item.addEventListener('click', () => {
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: node.path, name: node.name, projectName },
      }));
    });
  }

  // Context menu on right-click
  item.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const menuItems = [
      { label: 'Copy Path', action: () => navigator.clipboard.writeText(node.path) },
      { label: 'Copy Name', action: () => navigator.clipboard.writeText(node.name) },
      { separator: true },
      { label: 'Reveal in File Manager', action: async () => {
        try {
          const { invoke } = await import('@tauri-apps/api/core');
          const dir = node.is_dir ? node.path : node.path.replace(/[\\/][^\\/]+$/, '');
          invoke('plugin:shell|open', { path: dir }).catch(() => {});
        } catch {}
      }},
    ];
    if (node.is_dir) {
      menuItems.unshift(
        { label: 'New File...', action: () => promptAndCreateFile(node.path, projectName).then(() => reloadChildren(wrapper, node, depth, projectName)) },
        { label: 'New Folder...', action: () => promptAndCreateFolder(node.path).then(() => reloadChildren(wrapper, node, depth, projectName)) },
        { separator: true },
      );
    } else {
      menuItems.unshift(
        { label: 'Open File', action: () => {
          window.dispatchEvent(new CustomEvent('rustic:open-file', {
            detail: { path: node.path, name: node.name, projectName },
          }));
        }},
        { separator: true },
      );
    }
    showContextMenu(menuItems, e.clientX, e.clientY);
  });

  wrapper.appendChild(item);
  return wrapper;
}

// Reload children after creating a file/folder
async function reloadChildren(wrapper, node, depth, projectName) {
  if (!expandedDirs.has(node.path)) {
    // Auto-expand the folder
    expandedDirs.add(node.path);
  }
  // Remove old tree
  const oldTree = wrapper.querySelector('.file-tree');
  if (oldTree) oldTree.remove();
  // Re-render
  const tree = createFileTree(node.path, depth + 1, projectName);
  wrapper.appendChild(tree);
  // Update caret
  const caret = wrapper.querySelector('.file-tree-item__caret');
  if (caret) {
    caret.innerHTML = '';
    caret.appendChild(icon('M6 9l6 6 6-6', 12));
  }
}
