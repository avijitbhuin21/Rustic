import { el, icon } from '../../utils/dom.js';
import { setActiveBuffer, closeBuffer, editorStore, SETTINGS_BUFFER_ID } from '../../state/editor.js';
import { closeSettings } from '../../state/settings.js';
import { showContextMenu } from '../dropdown-menu.js';
import { setDragType, clearDragType } from '../../utils/drag-state.js';

/**
 * Create a single tab element.
 * @param {{ id: number, fileName: string, projectName: string, isModified: boolean, isActive: boolean }} opts
 */
export function createTab({ id, fileName, projectName, isModified, isActive, groupId }) {
  const tab = el('div', {
    class: `tab ${isActive ? 'tab--active' : ''}`,
    dataset: { bufferId: id },
  });

  const isSettings = id === SETTINGS_BUFFER_ID;

  if (isSettings) {
    // Gear icon for settings tab
    const gearIcon = icon('M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z M15 12a3 3 0 11-6 0 3 3 0 016 0z', 12);
    gearIcon.style.flexShrink = '0';
    tab.appendChild(gearIcon);
  } else {
    // Modified dot indicator
    const modDot = el('span', { class: `tab__modified ${isModified ? 'tab__modified--visible' : ''}` });
    tab.appendChild(modDot);
  }

  // Project name prefix
  const projLabel = (!isSettings && projectName)
    ? el('span', { class: 'tab__project' }, `[${projectName}] `)
    : null;

  // File name
  const nameLabel = el('span', { class: 'tab__name' }, fileName);

  // Close button
  const closeBtn = el('button', { class: 'tab__close', title: 'Close' });
  closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
  closeBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (isSettings) {
      closeSettings();
    } else {
      closeBuffer(id, { groupId });
    }
  });

  if (projLabel) tab.appendChild(projLabel);
  tab.appendChild(nameLabel);
  tab.appendChild(closeBtn);

  // Click to activate (within this group)
  tab.addEventListener('click', () => setActiveBuffer(id, groupId));

  // Middle-click to close
  tab.addEventListener('mousedown', (e) => {
    if (e.button === 1) {
      e.preventDefault();
      closeBuffer(id, { groupId });
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
      { label: 'Close Others', action: async () => {
        for (const i of ids.filter((i) => i !== id)) await closeBuffer(i);
      }},
      { label: 'Close to the Right', action: async () => {
        for (const i of ids.slice(idx + 1)) await closeBuffer(i);
      }},
      { label: 'Close All', action: async () => {
        for (const i of ids) await closeBuffer(i);
      }},
      { separator: true },
      { label: 'Copy Path', action: () => {
        const buf = buffers[id];
        if (buf?.filePath) navigator.clipboard.writeText(buf.filePath);
      }},
    ], e.clientX, e.clientY);
  });

  // Drag and drop — include buffer ID and source group ID
  tab.draggable = true;
  tab.addEventListener('dragstart', (e) => {
    const payload = JSON.stringify({ __rustic: 'tab', bufferId: id, groupId });
    e.dataTransfer.setData('text/plain', payload);
    e.dataTransfer.effectAllowed = 'move';
    setDragType('tab');
    tab.classList.add('tab--dragging');
    console.log('[DnD] tab dragstart', { bufferId: id, groupId, effectAllowed: e.dataTransfer.effectAllowed, types: Array.from(e.dataTransfer.types) });
  });
  tab.addEventListener('dragend', (e) => {
    console.log('[DnD] tab dragend', { dropEffect: e.dataTransfer.dropEffect });
    clearDragType();
    tab.classList.remove('tab--dragging');
  });

  return tab;
}
