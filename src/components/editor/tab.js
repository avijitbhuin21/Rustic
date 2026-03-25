import { el, icon } from '../../utils/dom.js';
import { setActiveBuffer, closeBuffer, editorStore } from '../../state/editor.js';
import { showContextMenu } from '../dropdown-menu.js';

/**
 * Create a single tab element.
 * @param {{ id: number, fileName: string, projectName: string, isModified: boolean, isActive: boolean }} opts
 */
export function createTab({ id, fileName, projectName, isModified, isActive }) {
  const tab = el('div', {
    class: `tab ${isActive ? 'tab--active' : ''}`,
    dataset: { bufferId: id },
  });

  // Modified dot indicator
  const modDot = el('span', { class: `tab__modified ${isModified ? 'tab__modified--visible' : ''}` });

  // Project name prefix
  const projLabel = projectName
    ? el('span', { class: 'tab__project' }, `[${projectName}] `)
    : null;

  // File name
  const nameLabel = el('span', { class: 'tab__name' }, fileName);

  // Close button
  const closeBtn = el('button', { class: 'tab__close', title: 'Close' });
  closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
  closeBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    closeBuffer(id);
  });

  tab.appendChild(modDot);
  if (projLabel) tab.appendChild(projLabel);
  tab.appendChild(nameLabel);
  tab.appendChild(closeBtn);

  // Click to activate
  tab.addEventListener('click', () => setActiveBuffer(id));

  // Middle-click to close
  tab.addEventListener('mousedown', (e) => {
    if (e.button === 1) {
      e.preventDefault();
      closeBuffer(id);
    }
  });

  // Context menu
  tab.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    const buffers = editorStore.getState('openBuffers') || {};
    const ids = Object.keys(buffers).map(Number);
    const idx = ids.indexOf(id);

    showContextMenu([
      { label: 'Close', action: () => closeBuffer(id) },
      { label: 'Close Others', action: () => {
        ids.filter((i) => i !== id).forEach((i) => closeBuffer(i));
      }},
      { label: 'Close to the Right', action: () => {
        ids.slice(idx + 1).forEach((i) => closeBuffer(i));
      }},
      { label: 'Close All', action: () => {
        ids.forEach((i) => closeBuffer(i));
      }},
      { separator: true },
      { label: 'Copy Path', action: () => {
        const buf = buffers[id];
        if (buf?.filePath) navigator.clipboard.writeText(buf.filePath);
      }},
    ], e.clientX, e.clientY);
  });

  // Drag and drop for tab reordering
  tab.draggable = true;
  tab.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData('text/plain', String(id));
    tab.classList.add('tab--dragging');
  });
  tab.addEventListener('dragend', () => {
    tab.classList.remove('tab--dragging');
  });
  tab.addEventListener('dragover', (e) => {
    e.preventDefault();
    tab.classList.add('tab--drop-target');
  });
  tab.addEventListener('dragleave', () => {
    tab.classList.remove('tab--drop-target');
  });
  tab.addEventListener('drop', (e) => {
    e.preventDefault();
    tab.classList.remove('tab--drop-target');
    // Tab reorder would need state management — basic visual feedback for now
  });

  return tab;
}
