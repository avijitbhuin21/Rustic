import { el } from '../../utils/dom.js';
import { getCachedChildren, loadChildren } from '../../state/workspace.js';
import { createFileTreeItem, INDENT_PX } from './file-tree-item.js';

export function createFileTree(path, depth, projectName) {
  const container = el('div', { class: 'file-tree' });

  const children = getCachedChildren(path);

  if (children === null) {
    // Not loaded yet — trigger load
    console.log('[FileTree] createFileTree ASYNC path=%s depth=%d', path, depth);
    loadChildren(path).then(() => {
      console.log('[FileTree] async load resolved path=%s depth=%d inDOM=%s', path, depth, document.body.contains(container));
      renderItems(container, path, depth, projectName);
    });
    container.appendChild(el('div', { class: 'file-tree__loading' }, 'Loading...'));
    return container;
  }

  console.log('[FileTree] createFileTree SYNC path=%s depth=%d children=%d', path, depth, children.length);
  renderItems(container, path, depth, projectName);
  return container;
}

function renderItems(container, path, depth, projectName) {
  const children = getCachedChildren(path);
  // Preserve any active inline input
  const inlineInput = container.querySelector('.inline-input-wrapper');
  container.innerHTML = '';
  if (inlineInput) container.appendChild(inlineInput);

  if (!children || children.length === 0) {
    console.log('[FileTree] renderItems EMPTY path=%s depth=%d', path, depth);
    container.appendChild(
      el('div', {
        class: 'file-tree__empty',
        style: { paddingLeft: (depth + 1) * INDENT_PX + 'px' },
      }, 'Empty folder'),
    );
    return;
  }

  // Defensive sort: directories first, then alphabetical (case-insensitive)
  const sorted = [...children].sort((a, b) =>
    (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0)
    || a.name.toLowerCase().localeCompare(b.name.toLowerCase())
  );

  console.log('[FileTree] renderItems path=%s depth=%d items=%s', path, depth,
    sorted.map(n => (n.is_dir ? '[D]' : '[F]') + n.name).join(', '));

  for (const node of sorted) {
    container.appendChild(createFileTreeItem(node, depth, projectName));
  }

  // Validate DOM structure after render
  setTimeout(() => {
    if (!document.body.contains(container)) return;
    const wrappers = container.querySelectorAll(':scope > .file-tree-item-wrapper');
    for (const w of wrappers) {
      const item = w.querySelector(':scope > .file-tree-item');
      const subtree = w.querySelector(':scope > .file-tree');
      const itemIdx = Array.from(w.children).indexOf(item);
      const treeIdx = subtree ? Array.from(w.children).indexOf(subtree) : -1;
      if (item && subtree && treeIdx < itemIdx) {
        console.error('[FileTree] BUG: subtree rendered BEFORE item! wrapper=%s', w.dataset.path);
      }
    }
    // Check if any file-tree children leaked to wrong level
    const parentWrapper = container.parentElement;
    if (parentWrapper && parentWrapper.classList.contains('file-tree-item-wrapper')) {
      const parentPath = parentWrapper.dataset.path;
      for (const w of wrappers) {
        const childPath = w.dataset.path;
        if (childPath && !childPath.startsWith(parentPath)) {
          console.error('[FileTree] BUG: child path=%s does not belong under parent=%s', childPath, parentPath);
        }
      }
    }
  }, 50);
}
