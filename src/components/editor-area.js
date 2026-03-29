import { el, iconMulti } from '../utils/dom.js';
import { editorStore, closeGroup, openFile } from '../state/editor.js';
import { createEditorGroup } from './editor/editor-group.js';
import * as api from '../lib/tauri-api.js';
import { getDragType, setDragType, clearDragType } from '../utils/drag-state.js';

export function createEditorArea() {
  const area = el('div', { class: 'editor-area' });

  const placeholder = el('div', { class: 'editor-placeholder' }, [
    iconMulti([
      'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z',
      'M13 2v7h7',
    ], 48),
    el('span', {}, 'Open a file to start editing'),
  ]);

  // Split container for editor groups
  const splitContainer = el('div', { class: 'editor-split-container' });
  splitContainer.style.display = 'none';

  area.appendChild(placeholder);
  area.appendChild(splitContainer);

  // Track created group elements
  const groupElements = new Map(); // groupId -> { element, groupId }

  function render() {
    const groups = editorStore.getState('groups');
    const buffers = editorStore.getState('openBuffers');

    // Check if any group has buffers
    const hasAnyBuffer = groups.some(g => g.bufferIds.length > 0);

    if (!hasAnyBuffer) {
      placeholder.style.display = 'flex';
      splitContainer.style.display = 'none';
      return;
    }

    placeholder.style.display = 'none';
    splitContainer.style.display = 'flex';

    // Reconcile groups: add new, remove stale
    const currentGroupIds = new Set(groups.map(g => g.id));

    // Remove groups that no longer exist
    for (const [gId, gEl] of groupElements) {
      if (!currentGroupIds.has(gId)) {
        gEl.element.remove();
        // Also remove resize handle before this group if it exists
        const handle = splitContainer.querySelector(`[data-resize-before="${gId}"]`);
        if (handle) handle.remove();
        groupElements.delete(gId);
      }
    }

    // Add new groups and resize handles
    splitContainer.innerHTML = '';
    for (let i = 0; i < groups.length; i++) {
      const g = groups[i];

      // Add resize handle between groups (not before the first)
      if (i > 0) {
        const handle = createSplitResizeHandle();
        handle.dataset.resizeBefore = g.id;
        splitContainer.appendChild(handle);
      }

      if (!groupElements.has(g.id)) {
        const group = createEditorGroup(g.id);
        groupElements.set(g.id, group);
      }
      splitContainer.appendChild(groupElements.get(g.id).element);
    }
  }

  editorStore.subscribe('groups', render);
  editorStore.subscribe('openBuffers', render);
  render();

  // ── Tauri native file drop handling ──
  // Tauri v2 intercepts OS file drops at the native level and emits events
  // instead of letting them reach the HTML5 drop handler.
  // We listen for tauri://drag-over to show drop highlights and
  // tauri://drag-drop to open the files in the correct editor group.

  function findGroupAtPosition(x, y) {
    for (const [gId, gObj] of groupElements) {
      const rect = gObj.element.getBoundingClientRect();
      if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
        return gId;
      }
    }
    return null;
  }

  let externalHighlightedGroup = null;

  api.onFileDragOver((payload) => {
    const { position } = payload;
    if (!position) return;

    // IMPORTANT: tauri://drag-over fires even for internal webview drags.
    // Only treat as external if no internal drag (tab/file) is in progress.
    const currentType = getDragType();
    if (currentType === 'tab' || currentType === 'file') return;

    setDragType('external');

    const gId = findGroupAtPosition(position.x, position.y);

    // Remove highlight from previous group
    if (externalHighlightedGroup && externalHighlightedGroup !== gId) {
      const prev = groupElements.get(externalHighlightedGroup);
      if (prev) prev.element.classList.remove('editor-group--drop-target');
    }

    // Add highlight to current group
    if (gId) {
      const gObj = groupElements.get(gId);
      if (gObj) gObj.element.classList.add('editor-group--drop-target');
      externalHighlightedGroup = gId;
    } else {
      externalHighlightedGroup = null;
    }
  });

  api.onFileDragLeave(() => {
    // Only clear if we were tracking an external drag
    if (getDragType() === 'external') {
      clearDragType();
    }
    // Remove all drop highlights
    if (externalHighlightedGroup) {
      const prev = groupElements.get(externalHighlightedGroup);
      if (prev) prev.element.classList.remove('editor-group--drop-target');
      externalHighlightedGroup = null;
    }
  });

  api.onFileDrop((payload) => {
    const { paths, position } = payload;

    // Ignore if this is an internal drag (tab/file from within the app)
    const currentType = getDragType();
    if (currentType === 'tab' || currentType === 'file') return;

    clearDragType();

    // Remove highlight
    if (externalHighlightedGroup) {
      const prev = groupElements.get(externalHighlightedGroup);
      if (prev) prev.element.classList.remove('editor-group--drop-target');
      externalHighlightedGroup = null;
    }

    if (!paths || paths.length === 0) return;

    // Find which editor group the files were dropped on
    let targetGroupId = null;
    if (position) {
      targetGroupId = findGroupAtPosition(position.x, position.y);
    }
    // Fall back to the active group
    if (!targetGroupId) {
      targetGroupId = editorStore.getState('activeGroupId');
    }

    console.log('[DnD] Tauri external file drop', { paths, position, targetGroupId });

    for (const filePath of paths) {
      openFile(filePath, '', targetGroupId);
    }
  });

  return area;
}

/** Resize handle between editor groups */
function createSplitResizeHandle() {
  const handle = el('div', { class: 'editor-split-handle' });

  let startX = 0;
  let leftEl = null;
  let rightEl = null;
  let totalWidth = 0;
  let leftStart = 0;

  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    startX = e.clientX;
    leftEl = handle.previousElementSibling;
    rightEl = handle.nextElementSibling;
    if (!leftEl || !rightEl) return;

    totalWidth = leftEl.offsetWidth + rightEl.offsetWidth;
    leftStart = leftEl.offsetWidth;
    handle.classList.add('active');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    function onMove(e) {
      const delta = e.clientX - startX;
      const newLeft = Math.max(100, Math.min(totalWidth - 100, leftStart + delta));
      const newRight = totalWidth - newLeft;
      leftEl.style.flex = `0 0 ${newLeft}px`;
      rightEl.style.flex = `0 0 ${newRight}px`;
    }

    function onUp() {
      handle.classList.remove('active');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      // Convert back to flex ratios
      const leftW = leftEl.offsetWidth;
      const rightW = rightEl.offsetWidth;
      const total = leftW + rightW;
      leftEl.style.flex = `${leftW / total}`;
      rightEl.style.flex = `${rightW / total}`;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    }

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  });

  return handle;
}
