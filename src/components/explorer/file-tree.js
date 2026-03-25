import { el } from '../../utils/dom.js';
import { getCachedChildren, loadChildren } from '../../state/workspace.js';
import { createFileTreeItem } from './file-tree-item.js';

export function createFileTree(path, depth, projectName) {
  const container = el('div', { class: 'file-tree' });

  const children = getCachedChildren(path);

  if (children === null) {
    // Not loaded yet — trigger load
    loadChildren(path).then(() => {
      renderItems(container, path, depth, projectName);
    });
    container.appendChild(el('div', { class: 'file-tree__loading' }, 'Loading...'));
    return container;
  }

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
    container.appendChild(
      el('div', {
        class: 'file-tree__empty',
        style: { paddingLeft: (depth + 1) * 16 + 'px' },
      }, 'Empty folder'),
    );
    return;
  }

  for (const node of children) {
    container.appendChild(createFileTreeItem(node, depth, projectName));
  }
}
