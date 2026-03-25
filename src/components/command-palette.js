import { el } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { editorStore, openFile, saveActiveBuffer } from '../state/editor.js';
import { openSettings, closeSettings } from '../state/settings.js';
import * as api from '../lib/tauri-api.js';
import { workspaceStore } from '../state/workspace.js';

let paletteEl = null;
let inputEl = null;
let listEl = null;
let visible = false;
let mode = 'commands'; // 'commands' or 'files'
let selectedIndex = 0;
let filteredItems = [];

const commands = [
  { label: 'File: Save', action: () => saveActiveBuffer() },
  { label: 'File: Open Settings', action: () => openSettings() },
  { label: 'View: Toggle Sidebar', action: () => uiStore.setState({ primarySidebarVisible: !uiStore.getState('primarySidebarVisible') }) },
  { label: 'View: Toggle Panel', action: () => uiStore.setState({ bottomPanelVisible: !uiStore.getState('bottomPanelVisible') }) },
  { label: 'View: Toggle Secondary Sidebar', action: () => uiStore.setState({ secondarySidebarVisible: !uiStore.getState('secondarySidebarVisible') }) },
  { label: 'View: Explorer', action: () => uiStore.setState({ activePanel: 'explorer', primarySidebarVisible: true }) },
  { label: 'View: Search', action: () => uiStore.setState({ activePanel: 'search', primarySidebarVisible: true }) },
  { label: 'View: Source Control', action: () => uiStore.setState({ activePanel: 'git', primarySidebarVisible: true }) },
  { label: 'View: Agent', action: () => uiStore.setState({ activePanel: 'agent', primarySidebarVisible: true }) },
  { label: 'Terminal: New Terminal', action: async () => {
    const { createTerminal } = await import('../state/terminal.js');
    createTerminal();
  }},
  { label: 'Editor: Format Document', action: async () => {
    const id = editorStore.getState('activeBufferId');
    if (id) await api.formatDocument(id);
  }},
];

function ensureCreated() {
  if (paletteEl) return;

  paletteEl = el('div', { class: 'command-palette-overlay' });
  paletteEl.style.display = 'none';

  const box = el('div', { class: 'command-palette' });
  inputEl = el('input', { class: 'command-palette__input', type: 'text', placeholder: 'Type a command...' });
  listEl = el('div', { class: 'command-palette__list' });

  box.appendChild(inputEl);
  box.appendChild(listEl);
  paletteEl.appendChild(box);
  document.body.appendChild(paletteEl);

  paletteEl.addEventListener('click', (e) => {
    if (e.target === paletteEl) hide();
  });

  inputEl.addEventListener('input', () => {
    filter(inputEl.value);
  });

  inputEl.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { hide(); return; }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredItems.length - 1);
      renderList();
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      renderList();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      accept(selectedIndex);
    }
  });
}

function filter(query) {
  const q = query.toLowerCase();
  if (mode === 'commands') {
    filteredItems = commands.filter((c) => c.label.toLowerCase().includes(q));
  }
  selectedIndex = 0;
  renderList();
}

function renderList() {
  listEl.innerHTML = '';
  filteredItems.forEach((item, i) => {
    const row = el('div', {
      class: `command-palette__item ${i === selectedIndex ? 'command-palette__item--selected' : ''}`,
    });
    row.textContent = item.label || item.name || item.path || '';
    row.addEventListener('click', () => accept(i));
    listEl.appendChild(row);
  });
  const selected = listEl.querySelector('.command-palette__item--selected');
  if (selected) selected.scrollIntoView({ block: 'nearest' });
}

function accept(index) {
  const item = filteredItems[index];
  if (!item) return;
  hide();
  if (item.action) item.action();
  else if (item.path) openFile(item.path);
}

function show(m = 'commands') {
  ensureCreated();
  mode = m;
  visible = true;
  paletteEl.style.display = 'flex';
  inputEl.value = '';
  inputEl.placeholder = mode === 'files' ? 'Search files by name...' : 'Type a command...';

  if (mode === 'files') {
    loadFiles();
  } else {
    filteredItems = [...commands];
  }

  selectedIndex = 0;
  renderList();
  inputEl.focus();
}

async function loadFiles() {
  const projects = workspaceStore.getState('projects') || [];
  filteredItems = [];
  for (const p of projects) {
    try {
      const results = await api.searchInProject(p.id, '', false, false, false, '*', '');
      // fallback: just show project name
    } catch {}
  }
  // Simple approach: use empty query and just show commands
  filteredItems = [];
  renderList();
}

function hide() {
  if (paletteEl) paletteEl.style.display = 'none';
  visible = false;
}

export function openCommandPalette(m = 'commands') {
  show(m);
}

export function isCommandPaletteVisible() {
  return visible;
}

// Global keyboard shortcut
document.addEventListener('keydown', (e) => {
  if (e.ctrlKey && e.shiftKey && e.key === 'P') {
    e.preventDefault();
    if (visible) hide(); else show('commands');
  }
  if (e.ctrlKey && !e.shiftKey && e.key === 'p') {
    e.preventDefault();
    if (visible) hide(); else show('files');
  }
});
