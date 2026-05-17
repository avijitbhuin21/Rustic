import { el } from '../utils/dom.js';
import { uiStore } from '../state/ui.js';
import { openFile } from '../state/editor.js';
import * as api from '../lib/tauri-api.js';
import { workspaceStore } from '../state/workspace.js';
import { getAllCommands, executeCommand } from '../lib/commands.js';

let paletteEl = null;
let inputEl = null;
let listEl = null;
let hintEl = null;
let visible = false;
let mode = 'commands'; // 'commands' or 'files'
let selectedIndex = 0;
let filteredItems = [];

let fileIndex = [];
let fileIndexLoaded = false;
let fileIndexLoading = null;

function ensureCreated() {
  if (paletteEl) return;

  paletteEl = el('div', { class: 'command-palette-overlay' });
  paletteEl.style.display = 'none';

  const box = el('div', { class: 'command-palette' });
  inputEl = el('input', { class: 'command-palette__input', type: 'text', placeholder: 'Type a command...' });
  listEl = el('div', { class: 'command-palette__list' });
  hintEl = el('div', { class: 'command-palette__hint' });

  box.appendChild(inputEl);
  box.appendChild(listEl);
  box.appendChild(hintEl);
  paletteEl.appendChild(box);
  document.body.appendChild(paletteEl);

  paletteEl.addEventListener('click', (e) => {
    if (e.target === paletteEl) hide();
  });

  inputEl.addEventListener('input', () => {
    const v = inputEl.value;
    // ">" prefix forces commands mode (VS Code-style).
    if (v.startsWith('>') && mode !== 'commands') {
      mode = 'commands';
      inputEl.value = v.slice(1);
      filter(inputEl.value);
      return;
    }
    filter(v);
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

// Subsequence fuzzy match. Returns null if no match, lower score = better match.
function fuzzyScore(query, target) {
  if (!query) return 0;
  const q = query.toLowerCase();
  const t = target.toLowerCase();

  const idx = t.indexOf(q);
  if (idx >= 0) return idx;

  let score = 0;
  let qi = 0;
  let prevMatch = -2;
  for (let i = 0; i < t.length && qi < q.length; i++) {
    if (t[i] === q[qi]) {
      const gap = i - prevMatch - 1;
      score += gap * 4;
      if (i === 0 || /[ /\\:._-]/.test(t[i - 1])) score -= 2;
      prevMatch = i;
      qi++;
    }
  }
  if (qi < q.length) return null;
  return score + (prevMatch - q.length) * 0.5;
}

function filter(query) {
  const q = (query || '').trim();
  if (mode === 'commands') {
    const cmds = getAllCommands().map((c) => ({
      kind: 'command',
      id: c.id,
      title: c.title,
      category: c.category,
      label: `${c.category}: ${c.title}`,
      hint: c.id,
    }));
    if (!q) {
      filteredItems = cmds;
    } else {
      filteredItems = cmds
        .map((c) => ({ c, score: fuzzyScore(q, c.label) }))
        .filter((x) => x.score !== null)
        .sort((a, b) => a.score - b.score)
        .map((x) => x.c);
    }
  } else if (mode === 'files') {
    if (!q) {
      filteredItems = fileIndex.slice(0, 200);
    } else {
      // Score against basename first (high signal), then full path as fallback.
      const matches = [];
      for (const f of fileIndex) {
        const base = f.relPath.split(/[/\\]/).pop() || f.relPath;
        const baseScore = fuzzyScore(q, base);
        const pathScore = fuzzyScore(q, f.relPath);
        let score;
        if (baseScore === null && pathScore === null) continue;
        if (baseScore === null) score = pathScore + 50;
        else if (pathScore === null) score = baseScore;
        else score = Math.min(baseScore, pathScore + 25);
        matches.push({ f, score });
      }
      matches.sort((a, b) => a.score - b.score);
      filteredItems = matches.slice(0, 200).map((m) => m.f);
    }
  }
  selectedIndex = 0;
  renderList();
}

function renderList() {
  listEl.innerHTML = '';

  if (filteredItems.length === 0) {
    const empty = el('div', { class: 'command-palette__empty' });
    if (mode === 'files' && !fileIndexLoaded) {
      empty.textContent = 'Indexing files…';
    } else if (mode === 'files' && fileIndex.length === 0) {
      empty.textContent = 'No project open. Add one from the Explorer.';
    } else {
      empty.textContent = 'No matches';
    }
    listEl.appendChild(empty);
    updateHint();
    return;
  }

  filteredItems.forEach((item, i) => {
    const row = el('div', {
      class: `command-palette__item ${i === selectedIndex ? 'command-palette__item--selected' : ''}`,
    });

    const main = el('div', { class: 'command-palette__main' });
    const meta = el('div', { class: 'command-palette__meta' });

    if (item.kind === 'command') {
      main.textContent = item.label;
      meta.textContent = item.hint;
    } else {
      const base = item.relPath.split(/[/\\]/).pop() || item.relPath;
      const dir = item.relPath.slice(0, item.relPath.length - base.length).replace(/[/\\]$/, '');
      main.textContent = base;
      const projectPrefix = item.multiProject ? `${item.projectName} • ` : '';
      meta.textContent = `${projectPrefix}${dir || '/'}`;
    }

    row.appendChild(main);
    row.appendChild(meta);
    row.addEventListener('click', () => accept(i));
    listEl.appendChild(row);
  });

  const selected = listEl.querySelector('.command-palette__item--selected');
  if (selected) selected.scrollIntoView({ block: 'nearest' });
  updateHint();
}

function updateHint() {
  if (!hintEl) return;
  if (mode === 'files') {
    hintEl.textContent = `Files (${fileIndex.length}) — ↑↓ navigate, ↵ open, esc close`;
  } else {
    hintEl.textContent = `Commands (${getAllCommands().length}) — ↑↓ navigate, ↵ run, esc close`;
  }
}

function accept(index) {
  const item = filteredItems[index];
  if (!item) return;
  hide();
  if (item.kind === 'command') {
    executeCommand(item.id);
  } else if (item.kind === 'file') {
    openFile(item.absPath);
    // Surface the editor area in case the user is on a different panel.
    uiStore.setState({ activePanel: 'explorer' });
  }
}

function show(m = 'commands') {
  ensureCreated();
  mode = m;
  visible = true;
  paletteEl.style.display = 'flex';
  inputEl.value = '';
  inputEl.placeholder = mode === 'files'
    ? 'Search files by name…'
    : 'Type a command (or > to search commands)…';

  if (mode === 'files') {
    // Kick off (or reuse) the index, then render whatever's available so the
    // user can start typing immediately.
    ensureFileIndex().then(() => {
      if (visible && mode === 'files') filter(inputEl.value);
    });
  }

  filter(inputEl.value);
  selectedIndex = 0;
  renderList();
  inputEl.focus();
}

async function ensureFileIndex() {
  if (fileIndexLoaded) return;
  if (fileIndexLoading) return fileIndexLoading;

  fileIndexLoading = (async () => {
    const projects = (workspaceStore.getState('projects') || [])
      .filter((p) => p.id !== '__global__' && p.root_path);
    const multiProject = projects.length > 1;
    const next = [];
    for (const p of projects) {
      try {
        const files = await api.listProjectFiles(p.root_path, 5000);
        for (const rel of files) {
          // listProjectFiles returns forward-slash relative paths.
          const sep = p.root_path.includes('\\') ? '\\' : '/';
          const absPath = p.root_path.replace(/[/\\]+$/, '') + sep + rel.replace(/\//g, sep);
          next.push({
            kind: 'file',
            absPath,
            relPath: rel,
            projectName: p.name || p.root_path,
            multiProject,
          });
        }
      } catch (e) {
        // Non-fatal — one project failing shouldn't blank the picker.
        console.warn('Quick Open: failed to list files for project', p.id, e);
      }
    }
    fileIndex = next;
    fileIndexLoaded = true;
  })();

  try {
    await fileIndexLoading;
  } finally {
    fileIndexLoading = null;
  }
}

function hide() {
  if (paletteEl) paletteEl.style.display = 'none';
  visible = false;
}

/// Drop the cached file index so the next Quick Open re-walks the project.
/// Called by workspace state when projects are added/removed and on file-tree
/// refresh events so the picker doesn't lie about what files exist.
export function invalidateFileIndex() {
  fileIndex = [];
  fileIndexLoaded = false;
  fileIndexLoading = null;
}

export function openCommandPalette(m = 'commands') {
  // Toggle: pressing the shortcut while the palette is already open in the
  // same mode closes it (matches the old inline handler's behavior).
  if (visible && mode === m) {
    hide();
    return;
  }
  show(m);
}

export function isCommandPaletteVisible() {
  return visible;
}

// Keyboard shortcuts (Ctrl+P, Ctrl+Shift+P) are dispatched via the central
// keybinding registry — see src/lib/builtin-commands.js. Users can rebind
// them from Settings → Shortcuts.
