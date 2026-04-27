import { el } from '../utils/dom.js';
import { editorStore, closeGroup, openFile } from '../state/editor.js';
import { createEditorGroup } from './editor/editor-group.js';
import { openCommandPalette } from './command-palette.js';
import { uiStore } from '../state/ui.js';
import * as api from '../lib/tauri-api.js';
import { getDragType, setDragType, clearDragType } from '../utils/drag-state.js';

function createWelcomeShortcut(label, shortcut, action) {
  const row = el('div', { class: 'welcome-shortcut' });
  const link = el('a', { class: 'welcome-shortcut__label', href: '#' }, label);
  link.addEventListener('click', (e) => { e.preventDefault(); action(); });
  const keys = el('span', { class: 'welcome-shortcut__keys' });
  shortcut.split('+').forEach((key, i) => {
    if (i > 0) keys.appendChild(el('span', { class: 'welcome-key-sep' }, '+'));
    keys.appendChild(el('kbd', { class: 'welcome-kbd' }, key));
  });
  row.appendChild(link);
  row.appendChild(keys);
  return row;
}

/// A discoverable feature link with a description. Used in the welcome
/// screen's "Explore" section so first-run users find Agent / MCP / Skills
/// / Workflows / Git rather than having to guess what the activity-bar icons
/// do.
function createWelcomeFeature(label, description, action) {
  const row = el('div', { class: 'welcome-feature' });
  const link = el('a', {
    class: 'welcome-feature__label',
    href: '#',
    'aria-label': `${label}: ${description}`,
  }, label);
  link.addEventListener('click', (e) => { e.preventDefault(); action(); });
  const desc = el('span', { class: 'welcome-feature__desc' }, description);
  row.appendChild(link);
  row.appendChild(desc);
  return row;
}

function openSidebarPanel(panelId) {
  uiStore.setState({ activePanel: panelId, primarySidebarVisible: true });
}

function openAgent() {
  uiStore.setState({ activePanel: 'agent', secondarySidebarVisible: true });
}

export function createEditorArea() {
  const area = el('main', { class: 'editor-area', id: 'main-content', 'aria-label': 'Editor' });

  // Welcome screen logo. `new URL(..., import.meta.url)` is the Vite pattern
  // that survives both dev (served via vite) and prod (bundled + fingerprinted);
  // a plain relative src like 'rsutic_icon.svg' quietly fails under Tauri's
  // custom protocol and leaves the welcome screen image-less.
  const logoImg = el('img', {
    class: 'welcome-logo',
    src: new URL('../rsutic_icon.svg', import.meta.url).href,
    alt: 'Rustic',
    draggable: 'false',
  });

  const placeholder = el('div', { class: 'editor-placeholder' }, [
    logoImg,
    el('div', { class: 'welcome-shortcuts' }, [
      createWelcomeShortcut('Open File', 'Ctrl+O', async () => {
        try {
          const { open } = await import('@tauri-apps/plugin-dialog');
          const path = await open();
          if (path) openFile(path);
        } catch {}
      }),
      createWelcomeShortcut('Command Palette', 'Ctrl+Shift+P', () => openCommandPalette()),
      createWelcomeShortcut('Quick Open', 'Ctrl+P', () => openCommandPalette('files')),
    ]),
    el('div', { class: 'welcome-section-title' }, 'Explore'),
    el('div', { class: 'welcome-features' }, [
      createWelcomeFeature('AI Agent', 'Chat with Claude / OpenAI / Gemini, with file edits and tool use', () => openAgent()),
      createWelcomeFeature('Source Control', 'Stage, commit, diff, and push from the Git panel', () => openSidebarPanel('git')),
      createWelcomeFeature('Project Search', 'Find across files (Ctrl+Shift+F)', () => openSidebarPanel('search')),
      createWelcomeFeature('Skills', 'Reusable agent workflows installed from disk', () => openSidebarPanel('agent')),
      createWelcomeFeature('MCP servers', 'Connect external tool servers to the agent', () => openSidebarPanel('agent')),
      createWelcomeFeature('Settings', 'Configure providers, themes, and keybindings', async () => {
        const { openSettings } = await import('../state/settings.js');
        openSettings();
      }),
    ]),
  ]);

  // Split container for editor groups
  const splitContainer = el('div', { class: 'editor-split-container' });
  splitContainer.style.display = 'none';

  area.appendChild(placeholder);
  area.appendChild(splitContainer);

  // Track created group elements
  const groupElements = new Map(); // groupId -> { element, groupId }
  let lastGroupOrder = [];  // track group order to detect structural changes

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
    const currentOrder = groups.map(g => g.id);

    // Check if group structure actually changed (added/removed/reordered)
    const structureChanged =
      currentOrder.length !== lastGroupOrder.length ||
      currentOrder.some((id, i) => lastGroupOrder[i] !== id);

    // Remove groups that no longer exist
    for (const [gId, gEl] of groupElements) {
      if (!currentGroupIds.has(gId)) {
        gEl.element.remove();
        const handle = splitContainer.querySelector(`[data-resize-before="${gId}"]`);
        if (handle) handle.remove();
        groupElements.delete(gId);
      }
    }

    // Only rebuild DOM when group structure actually changed — avoid
    // innerHTML='' which detaches the textarea and kills keyboard focus
    if (structureChanged) {
      lastGroupOrder = currentOrder;
      splitContainer.innerHTML = '';
      for (let i = 0; i < groups.length; i++) {
        const g = groups[i];

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
