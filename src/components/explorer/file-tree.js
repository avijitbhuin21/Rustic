import { el } from '../../utils/dom.js';
import { getCachedChildren, loadChildren } from '../../state/workspace.js';
import { createFileTreeItem, INDENT_PX } from './file-tree-item.js';
import { debug } from '../../lib/log.js';
import { createSkeletonRows } from '../skeleton.js';

export function createFileTree(path, depth, projectName) {
  const container = el('div', { class: 'file-tree' });

  const children = getCachedChildren(path);

  if (children === null) {
    debug('FileTree', 'createFileTree ASYNC', { path, depth });
    loadChildren(path).then(() => {
      debug('FileTree', 'async load resolved', { path, depth, inDOM: document.body.contains(container) });
      renderItems(container, path, depth, projectName);
    });
    container.appendChild(createSkeletonRows(4, ['72%', '54%', '88%', '60%']));
    return container;
  }

  debug('FileTree', 'createFileTree SYNC', { path, depth, children: children.length });
  renderItems(container, path, depth, projectName);
  return container;
}

function renderItems(container, path, depth, projectName) {
  const children = getCachedChildren(path);
  const inlineInput = container.querySelector('.inline-input-wrapper');
  container.innerHTML = '';
  if (inlineInput) container.appendChild(inlineInput);

  if (!children || children.length === 0) {
    debug('FileTree', 'renderItems EMPTY', { path, depth });
    container.appendChild(
      el('div', {
        class: 'file-tree__empty',
        style: { paddingLeft: (depth + 1) * INDENT_PX + 'px' },
      }, 'Empty folder'),
    );
    return;
  }

  const sorted = [...children].sort((a, b) =>
    (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0)
    || a.name.toLowerCase().localeCompare(b.name.toLowerCase())
  );

  debug('FileTree', 'renderItems', { path, depth, count: sorted.length });

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
