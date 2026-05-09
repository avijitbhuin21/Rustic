import { el, icon } from '../../utils/dom.js';
import { settingsStore, updateSetting, loadSettings, savePalettes, saveFontConfig, saveFontLibrary } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';
import { applyTheme, getCurrentTheme } from '../../lib/theme.js';
import { loadFontFromUrl } from '../../lib/font-loader.js';


// SVG path constants
const ICON_COPY = 'M8 4H6a2 2 0 00-2 2v12a2 2 0 002 2h8a2 2 0 002-2v-2 M16 4h2a2 2 0 012 2v6 M12 2h4l4 4v2 M9 2v4h4';
const ICON_TRASH = 'M3 6h18 M19 6l-1 14H6L5 6 M10 11v6 M14 11v6 M8 6V4h8v2';
const ICON_CHECK = 'M20 6L9 17l-5-5';
const ICON_REVERT = 'M3 10h4m-4 0l3-3m-3 3l3 3 M21 12a9 9 0 11-3-6.7';
const ICON_EXPORT = 'M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4 M17 8l-5-5-5 5 M12 3v12';
const ICON_PLUS = 'M12 5v14 M5 12h14';
// ─── Appearance Settings (main export) ───────────────────────────

export function createAppearanceSettings(settings) {
  const container = el('div', { class: 'settings-section' });

  container.appendChild(createFontsCollapsible(settings));
  container.appendChild(createColorPaletteCollapsible(settings));

  return container;
}

// ─── Fonts Section ───────────────────────────────────────────────

function createFontsCollapsible(settings) {
  const wrapper = el('div', { class: 'settings-collapsible settings-collapsible--open' });

  const header = el('div', { class: 'settings-collapsible__header' });
  const chevron = el('span', { class: 'settings-collapsible__chevron' });
  chevron.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="9 18 15 12 9 6"/></svg>';
  header.appendChild(chevron);
  header.appendChild(el('span', { class: 'settings-collapsible__title' }, 'Fonts'));

  const fontCardsContainer = el('div');

  const addBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add font' });
  addBtn.appendChild(icon(ICON_PLUS, 14));
  addBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    showAddFontModal(fontCardsContainer);
  });
  header.appendChild(addBtn);

  header.addEventListener('click', (e) => {
    if (e.target.closest('.settings-collapsible__action-btn')) return;
    wrapper.classList.toggle('settings-collapsible--open');
  });

  const body = el('div', { class: 'settings-collapsible__body' });
  renderFontCards(fontCardsContainer);
  body.appendChild(fontCardsContainer);

  wrapper.appendChild(header);
  wrapper.appendChild(body);
  return wrapper;
}

// ─── Add Font Modal ──────────────────────────────────────────────

function showAddFontModal(fontCardsContainer) {
  document.querySelector('.font-add-modal-overlay')?.remove();

  const overlay = el('div', { class: 'palette-add-modal-overlay' });
  const modal = el('div', { class: 'palette-add-modal' });

  // Title row with info icon
  const titleRow = el('div', { class: 'font-add-modal__title-row' });
  titleRow.appendChild(el('div', { class: 'font-apply-modal__title' }, 'Add Font'));

  const infoWrap = el('div', { class: 'font-add-modal__info-wrap' });
  const infoBtn = el('button', { class: 'settings-card__icon-btn', title: 'Supported URL formats' });
  infoBtn.appendChild(icon('M12 2a10 10 0 100 20 10 10 0 000-20z M12 16v-4 M12 8h.01', 14));
  const tooltip = el('div', { class: 'font-add-modal__bubble' });
  tooltip.innerHTML = '<b>Supported URLs:</b><br>'
    + '- fonts.google.com/specimen/Font+Name<br>'
    + '- fonts.google.com/share?selection.family=...<br>'
    + '- fonts.googleapis.com/css2?family=...<br>'
    + '- Direct .woff2, .ttf, .otf file URLs<br>'
    + '<br><b>Tip:</b> Share URLs can load multiple fonts at once.';
  infoBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    tooltip.classList.toggle('font-add-modal__bubble--visible');
  });
  modal.addEventListener('click', (e) => {
    if (!e.target.closest('.font-add-modal__info-wrap')) tooltip.classList.remove('font-add-modal__bubble--visible');
  });
  infoWrap.appendChild(infoBtn);
  infoWrap.appendChild(tooltip);
  titleRow.appendChild(infoWrap);
  modal.appendChild(titleRow);

  // ── URL row (input + load button, single line) ──
  const urlRow = el('div', { class: 'font-add-modal__url-row' });

  const urlInput = el('input', {
    class: 'settings-input font-add-modal__url-input',
    type: 'text',
    placeholder: 'Paste a Google Fonts or direct font URL...',
  });
  urlRow.appendChild(urlInput);

  const loadBtn = el('button', { class: 'settings-btn settings-btn--accent' }, 'Load');
  loadBtn.addEventListener('click', async () => {
    const url = urlInput.value.trim();
    if (!url) { showStatus('Please enter a URL.', true); return; }
    loadBtn.textContent = 'Loading...';
    loadBtn.disabled = true;
    statusMsg.style.display = 'none';
    try {
      const fonts = await loadFontFromUrl(url);
      if (fonts.length > 0) {
        for (const f of fonts) addToFontLibrary(f.name, 'url', f.url);
        renderFontCards(fontCardsContainer);
        overlay.remove();
      } else {
        showStatus('Failed to load font. Check the URL and try again.', true);
      }
    } catch (e) {
      console.error('Font URL error:', e);
      showStatus('Error loading font. Make sure the URL is valid.', true);
    }
    loadBtn.textContent = 'Load';
    loadBtn.disabled = false;
  });
  urlRow.appendChild(loadBtn);
  modal.appendChild(urlRow);

  // Status / error message
  const statusMsg = el('div', { class: 'palette-add-modal__error', style: 'display:none' });
  modal.appendChild(statusMsg);

  // ── Divider ──
  const divider = el('div', { class: 'font-add-modal__divider' });
  divider.appendChild(el('span', {}, 'or'));
  modal.appendChild(divider);

  // ── Drop zone (bottom, clickable) ──
  const dropZone = el('div', { class: 'palette-add-modal__drop-zone font-add-modal__drop-zone' });
  dropZone.appendChild(icon('M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4 M7 10l5 5 5-5 M12 15V3', 28));
  dropZone.appendChild(el('div', { class: 'palette-add-modal__drop-text' }, 'Drop a .ttf, .otf, or .woff2 file here — or click to browse'));

  function showStatus(msg, isError) {
    statusMsg.textContent = msg;
    statusMsg.style.color = isError ? 'var(--bright-red)' : 'var(--fg3)';
    statusMsg.style.display = '';
  }

  const openFilePicker = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const path = await open({ filters: [{ name: 'Fonts', extensions: ['ttf', 'otf', 'woff2', 'woff'] }] });
      if (path) {
        showStatus('Loading...', false);
        const result = await loadLocalFontFile(path);
        if (result) {
          addToFontLibrary(result.name, 'file', result.dataUrl);
          renderFontCards(fontCardsContainer);
          overlay.remove();
        } else {
          showStatus('Failed to load font file.', true);
        }
      }
    } catch (e) {
      console.error('Font file error:', e);
      showStatus('Error loading file.', true);
    }
  };

  dropZone.addEventListener('click', openFilePicker);
  dropZone.addEventListener('dragover', (e) => { e.preventDefault(); dropZone.classList.add('palette-add-modal__drop-zone--active'); });
  dropZone.addEventListener('dragleave', () => { dropZone.classList.remove('palette-add-modal__drop-zone--active'); });
  dropZone.addEventListener('drop', async (e) => {
    e.preventDefault();
    dropZone.classList.remove('palette-add-modal__drop-zone--active');
    const file = e.dataTransfer?.files?.[0];
    if (!file) return;
    showStatus('Loading...', false);
    try {
      const buf = await file.arrayBuffer();
      const ext = file.name.split('.').pop().toLowerCase();
      const mimeMap = { ttf: 'font/ttf', otf: 'font/otf', woff: 'font/woff', woff2: 'font/woff2' };
      const mime = mimeMap[ext] || 'font/opentype';
      const base64 = btoa(String.fromCharCode(...new Uint8Array(buf)));
      const dataUrl = `data:${mime};base64,${base64}`;
      const name = file.name.replace(/\.[^.]+$/, '');
      const fontFace = new FontFace(name, `url(${dataUrl})`);
      await fontFace.load();
      document.fonts.add(fontFace);
      addToFontLibrary(name, 'file', dataUrl);
      renderFontCards(fontCardsContainer);
      overlay.remove();
    } catch (err) {
      console.error('Font drop error:', err);
      showStatus('Failed to load dropped font.', true);
    }
  });

  modal.appendChild(dropZone);

  overlay.appendChild(modal);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
  document.body.appendChild(overlay);
}

// ─── Font Library ────────────────────────────────────────────────

function addToFontLibrary(name, source, url) {
  const fonts = [...(settingsStore.getState('fontLibrary') || [])];
  const existing = fonts.findIndex((f) => f.name === name);
  if (existing >= 0) {
    fonts[existing] = { name, source, url };
  } else {
    fonts.push({ name, source, url });
  }
  saveFontLibrary(fonts);
}

const FONT_TARGETS = [
  { key: 'editor', label: 'Editor' },
  { key: 'terminal', label: 'Terminal' },
  { key: 'folderNames', label: 'Folder Names' },
  { key: 'fileNames', label: 'File Names' },
  { key: 'agentChat', label: 'Agent Chat' },
];

const TARGET_CSS_MAP = {
  editor:      '--font-family-mono',
  terminal:    '--font-family-terminal',
  folderNames: '--font-family-folders',
  fileNames:   '--font-family-files',
  agentChat:   '--font-family-chat',
};

function ensureChatFontStyleBlock() {
  const STYLE_ID = 'rustic-chat-font-style';
  if (document.getElementById(STYLE_ID)) return;
  const styleEl = document.createElement('style');
  styleEl.id = STYLE_ID;
  styleEl.textContent = [
    '.chat-messages { font-family: var(--font-family-chat, inherit); }',
    '.chat-message__text { font-family: var(--font-family-chat, inherit); }',
  ].join('\n');
  document.head.appendChild(styleEl);
}

function applyFontToTargets(fontName, targets) {
  const root = document.documentElement;
  const mono = `"${fontName}", monospace`;
  const ui = `"${fontName}", -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif`;
  const isAll = targets.length === FONT_TARGETS.length;

  const config = {};

  for (const key of targets) {
    const cssVar = TARGET_CSS_MAP[key];
    const value = (key === 'editor' || key === 'terminal') ? mono : ui;
    root.style.setProperty(cssVar, value);
    config[key] = value;
    if (key === 'agentChat') ensureChatFontStyleBlock();
  }

  if (isAll) {
    ensureChatFontStyleBlock();
    root.style.setProperty('--font-family', ui);
    saveFontConfig(null);
    updateSetting('appearance.font_family', fontName);
  } else {
    saveFontConfig(config);
  }
}

function showFontApplyModal(fontName, onDone) {
  // Remove any existing modal
  document.querySelector('.font-apply-modal-overlay')?.remove();

  const overlay = el('div', { class: 'font-apply-modal-overlay' });
  const modal = el('div', { class: 'font-apply-modal' });

  // Title
  const title = el('div', { class: 'font-apply-modal__title' });
  title.textContent = `Apply "${fontName}"`;
  modal.appendChild(title);

  const desc = el('div', { class: 'font-apply-modal__desc' }, 'Select where to apply this font:');
  modal.appendChild(desc);

  // Checkboxes
  const checkboxes = {};
  const optionsWrap = el('div', { class: 'font-apply-modal__options' });
  for (const t of FONT_TARGETS) {
    const lbl = el('label', { class: 'font-apply-modal__option' });
    const cb = el('input', { type: 'checkbox' });
    cb.checked = true;
    lbl.appendChild(cb);
    lbl.appendChild(document.createTextNode(t.label));
    optionsWrap.appendChild(lbl);
    checkboxes[t.key] = cb;
  }
  modal.appendChild(optionsWrap);

  // Buttons
  const btnRow = el('div', { class: 'font-apply-modal__actions' });

  const applyBtn = el('button', { class: 'settings-btn settings-btn--accent' }, 'Apply');
  applyBtn.addEventListener('click', () => {
    const selected = FONT_TARGETS.filter((t) => checkboxes[t.key].checked).map((t) => t.key);
    if (selected.length === 0) return;
    applyFontToTargets(fontName, selected);
    overlay.remove();
    if (onDone) onDone();
  });
  btnRow.appendChild(applyBtn);

  const cancelBtn = el('button', { class: 'settings-btn' }, 'Cancel');
  cancelBtn.addEventListener('click', () => overlay.remove());
  btnRow.appendChild(cancelBtn);

  modal.appendChild(btnRow);
  overlay.appendChild(modal);

  // Close on overlay click (outside modal)
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) overlay.remove();
  });

  document.body.appendChild(overlay);
}

function renderFontCards(container) {
  container.innerHTML = '';
  const fonts = settingsStore.getState('fontLibrary') || [];

  if (fonts.length === 0) {
    container.appendChild(el('div', { class: 'settings-empty' }, 'No fonts loaded yet. Paste a URL or browse for a file above.'));
    return;
  }

  const grid = el('div', { class: 'settings-card-grid' });
  for (const font of fonts) {
    const card = el('div', { class: 'settings-card' });

    // Header: name + icons
    const header = el('div', { class: 'settings-card__header' });
    const nameEl = el('div', { class: 'settings-card__name' }, font.name);
    nameEl.style.fontFamily = `"${font.name}", monospace`;
    header.appendChild(nameEl);

    const actions = el('div', { class: 'settings-card__icons' });

    const copyBtn = el('button', { class: 'settings-card__icon-btn', title: 'Copy font name' });
    copyBtn.appendChild(icon(ICON_COPY, 14));
    copyBtn.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(font.name);
        copyBtn.innerHTML = '';
        copyBtn.appendChild(icon(ICON_CHECK, 14));
        setTimeout(() => { copyBtn.innerHTML = ''; copyBtn.appendChild(icon(ICON_COPY, 14)); }, 1200);
      } catch { /* ignore */ }
    });
    actions.appendChild(copyBtn);

    const deleteBtn = el('button', { class: 'settings-card__icon-btn settings-card__icon-btn--danger', title: 'Remove font' });
    deleteBtn.appendChild(icon(ICON_TRASH, 14));
    deleteBtn.addEventListener('click', () => {
      const updated = fonts.filter((f) => f.name !== font.name);
      saveFontLibrary(updated);
      renderFontCards(container);
    });
    actions.appendChild(deleteBtn);

    header.appendChild(actions);
    card.appendChild(header);

    // Preview
    const preview = el('div', { class: 'settings-card__preview' });
    preview.style.fontFamily = `"${font.name}", monospace`;
    preview.textContent = 'The quick brown fox jumps over the lazy dog';
    card.appendChild(preview);

    // Apply button — opens overlay modal
    const applyBtn = el('button', { class: 'settings-card__apply-btn' }, 'Apply');
    applyBtn.addEventListener('click', () => {
      showFontApplyModal(font.name, () => {
        applyBtn.textContent = 'Applied!';
        setTimeout(() => { applyBtn.textContent = 'Apply'; }, 1500);
      });
    });
    card.appendChild(applyBtn);

    grid.appendChild(card);
  }
  container.appendChild(grid);
}

async function loadLocalFontFile(path) {
  try {
    const response = await api.readFileBase64(path);
    const base64 = response?.data || response;
    if (!base64) return null;

    const ext = path.split('.').pop().toLowerCase();
    const mimeMap = { ttf: 'font/ttf', otf: 'font/otf', woff: 'font/woff', woff2: 'font/woff2' };
    const mime = mimeMap[ext] || 'font/opentype';

    const dataUrl = `data:${mime};base64,${base64}`;
    const name = path.split(/[/\\]/).pop().replace(/\.[^.]+$/, '');
    const fontFace = new FontFace(name, `url(${dataUrl})`);
    await fontFace.load();
    document.fonts.add(fontFace);
    return { name, dataUrl };
  } catch (e) {
    console.error('Failed to load local font:', e);
    return null;
  }
}

// ─── Color Palette Section ───────────────────────────────────────

function createColorPaletteCollapsible(settings) {
  const wrapper = el('div', { class: 'settings-collapsible settings-collapsible--open' });

  // Custom header with + button
  const header = el('div', { class: 'settings-collapsible__header' });
  const chevron = el('span', { class: 'settings-collapsible__chevron' });
  chevron.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="9 18 15 12 9 6"/></svg>';
  header.appendChild(chevron);
  header.appendChild(el('span', { class: 'settings-collapsible__title' }, 'Color Palette'));

  const addBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add new palette' });
  addBtn.appendChild(icon(ICON_PLUS, 14));
  addBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    showAddPaletteModal(palettesContainer, settings);
  });
  header.appendChild(addBtn);

  header.addEventListener('click', (e) => {
    if (e.target.closest('.settings-collapsible__action-btn')) return;
    wrapper.classList.toggle('settings-collapsible--open');
  });

  const body = el('div', { class: 'settings-collapsible__body' });
  const palettesContainer = el('div');
  renderPaletteCards(palettesContainer, settings);
  body.appendChild(palettesContainer);

  wrapper.appendChild(header);
  wrapper.appendChild(body);
  return wrapper;
}

// ─── Add Palette Modal ───────────────────────────────────────────

function showAddPaletteModal(palettesContainer, settings) {
  document.querySelector('.palette-add-modal-overlay')?.remove();

  const overlay = el('div', { class: 'palette-add-modal-overlay' });
  const modal = el('div', { class: 'palette-add-modal' });

  modal.appendChild(el('div', { class: 'font-apply-modal__title' }, 'Add Palette'));
  modal.appendChild(el('div', { class: 'font-apply-modal__desc' }, 'Paste JSON config or import a file. Include a "name" key to save.'));

  // Side-by-side: textarea (left) + drop zone (right)
  const inputRow = el('div', { class: 'palette-add-modal__input-row' });

  const textarea = el('textarea', {
    class: 'settings-textarea palette-add-modal__textarea',
    placeholder: '{\n  "name": "My Theme",\n  "bg": "#282828",\n  "fg": "#ebdbb2",\n  "accent": "#8ec07c",\n  ...\n}',
    rows: '10',
  });
  inputRow.appendChild(textarea);

  // Drop zone (clickable)
  const dropZone = el('div', { class: 'palette-add-modal__drop-zone' });
  dropZone.appendChild(icon('M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4 M7 10l5 5 5-5 M12 15V3', 28));
  dropZone.appendChild(el('div', { class: 'palette-add-modal__drop-text' }, 'Drop .json / .toml file here or click to browse'));

  const openFilePicker = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const path = await open({ filters: [{ name: 'Theme', extensions: ['toml', 'json'] }] });
      if (path) {
        const content = await api.readFileContent(path);
        if (content) { textarea.value = content; textarea.dispatchEvent(new Event('input')); }
      }
    } catch (e) { console.error('Browse file error:', e); }
  };

  dropZone.addEventListener('click', openFilePicker);
  dropZone.addEventListener('dragover', (e) => { e.preventDefault(); dropZone.classList.add('palette-add-modal__drop-zone--active'); });
  dropZone.addEventListener('dragleave', () => { dropZone.classList.remove('palette-add-modal__drop-zone--active'); });
  dropZone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropZone.classList.remove('palette-add-modal__drop-zone--active');
    const file = e.dataTransfer?.files?.[0];
    if (file) {
      const reader = new FileReader();
      reader.onload = () => { textarea.value = reader.result; textarea.dispatchEvent(new Event('input')); };
      reader.readAsText(file);
    }
  });

  inputRow.appendChild(dropZone);
  modal.appendChild(inputRow);

  // Live preview
  const previewContainer = el('div', { class: 'settings-palette-preview', style: 'display:none' });
  modal.appendChild(previewContainer);

  // Error message area
  const errorMsg = el('div', { class: 'palette-add-modal__error', style: 'display:none' });
  modal.appendChild(errorMsg);

  function showError(msg) {
    errorMsg.textContent = msg;
    errorMsg.style.display = '';
    setTimeout(() => { errorMsg.style.display = 'none'; }, 4000);
  }

  textarea.addEventListener('input', () => {
    errorMsg.style.display = 'none';
    try {
      const data = JSON.parse(textarea.value);
      renderPalettePreview(previewContainer, data);
      previewContainer.style.display = '';
    } catch { previewContainer.style.display = 'none'; }
  });

  // Validate config and extract data
  function parseConfig() {
    const raw = textarea.value.trim();
    if (!raw) { showError('Please paste a JSON config or import a file.'); return null; }
    let data;
    try { data = JSON.parse(raw); }
    catch { showError('Invalid JSON. Please check the syntax.'); return null; }
    if (!data.bg && !data.fg && !data.accent) {
      showError('Config must include at least one color key (bg, fg, accent, etc.).');
      return null;
    }
    return data;
  }

  // Action buttons
  const actionRow = el('div', { class: 'font-apply-modal__actions' });

  // Single primary action: applying a palette auto-saves it to the library
  // and marks it Active. The previous "Apply" + "Save to Library" split made
  // it possible to apply a palette that wasn't in the card grid, which left
  // the old palette stuck as the visible Active row even though the colors
  // had clearly changed. Folding the two actions guarantees the cards always
  // reflect what's painted on screen.
  const applyBtn = el('button', { class: 'settings-btn settings-btn--accent' }, 'Apply & Save');
  applyBtn.addEventListener('click', async () => {
    const data = parseConfig();
    if (!data) return;
    if (!data.name || !data.name.trim()) {
      showError('Missing "name" key in config. Add "name": "My Theme" so the palette can be saved + activated.');
      return;
    }
    const name = data.name.trim();

    // Snapshot for the revert button BEFORE we mutate anything.
    snapshotPreviousPalette();

    // Save to library (upsert by name).
    const palettes = [...(settingsStore.getState('savedPalettes') || [])];
    const existing = palettes.findIndex((p) => p.name === name);
    if (existing >= 0) palettes[existing] = { name, data };
    else palettes.push({ name, data });
    savePalettes(palettes);

    // Apply via `applyTheme` so the currentTheme cache + CSS vars are in sync.
    applyTheme(data);
    cacheThemeColors(name, data);

    // Mark this palette as the active theme so the card renders with the
    // "Active" pill instead of "Set Active".
    await updateSetting('theme.active_theme', name);

    renderPaletteCards(palettesContainer, settingsStore.getState('settings'));
    overlay.remove();
  });
  actionRow.appendChild(applyBtn);

  // Save-only kept as a secondary action: stash the palette in the library
  // without making it the active theme. Useful for batch-importing themes
  // from a friend / dotfile repo.
  const saveOnlyBtn = el('button', { class: 'settings-btn' }, 'Save Only');
  saveOnlyBtn.addEventListener('click', () => {
    const data = parseConfig();
    if (!data) return;
    if (!data.name || !data.name.trim()) {
      showError('Missing "name" key in config. Add "name": "My Theme" to your JSON to save.');
      return;
    }
    const name = data.name.trim();
    const palettes = [...(settingsStore.getState('savedPalettes') || [])];
    const existing = palettes.findIndex((p) => p.name === name);
    if (existing >= 0) palettes[existing] = { name, data };
    else palettes.push({ name, data });
    savePalettes(palettes);
    renderPaletteCards(palettesContainer, settingsStore.getState('settings'));
    overlay.remove();
  });
  actionRow.appendChild(saveOnlyBtn);

  const cancelBtn = el('button', { class: 'settings-btn' }, 'Cancel');
  cancelBtn.addEventListener('click', () => overlay.remove());
  actionRow.appendChild(cancelBtn);

  modal.appendChild(actionRow);
  overlay.appendChild(modal);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
  document.body.appendChild(overlay);
}

// ─── Palette helpers ─────────────────────────────────────────────

function renderPalettePreview(container, data) {
  container.innerHTML = '';
  const colorKeys = ['bg', 'bg_hard', 'fg', 'accent', 'bright_red', 'bright_green', 'bright_yellow',
    'bright_blue', 'bright_purple', 'bright_aqua', 'bright_orange'];
  for (const key of colorKeys) {
    if (data[key]) {
      const swatch = el('div', { class: 'settings-swatch', title: `${key}: ${data[key]}` });
      swatch.style.backgroundColor = data[key];
      container.appendChild(swatch);
    }
  }
}

function applyPaletteFromConfig(data) {
  // Route through `applyTheme` so the in-module `currentTheme` cache stays in
  // sync with the CSS variables. Without this, `getCurrentTheme()` would
  // return the *previous* theme even after a fresh palette was painted onto
  // the document, which broke the revert flow (revert read a stale snapshot)
  // and the cross-component "rustic:theme-changed" listeners.
  applyTheme(data);
}

/// Capture the currently-active theme + its registry name so the user can
/// undo a palette switch with one click. Stashed on `settingsStore` so it
/// survives module re-imports / hot-reloads.
function snapshotPreviousPalette() {
  settingsStore.setState({
    previousPalette: getCurrentTheme(),
    previousActiveThemeName: settingsStore.getState('settings')?.theme?.active_theme || null,
  });
}

// ─── Theme color cache ───────────────────────────────────────────

const SWATCH_KEYS = ['bg', 'fg', 'accent', 'bright_red', 'bright_green', 'bright_blue',
  'bright_yellow', 'bright_purple', 'bright_aqua', 'bright_orange'];

function getThemeCache() {
  try { return JSON.parse(localStorage.getItem('rustic_theme_cache') || '{}'); }
  catch { return {}; }
}

function cacheThemeColors(name, data) {
  const cache = getThemeCache();
  const colors = {};
  for (const key of SWATCH_KEYS) { if (data[key]) colors[key] = data[key]; }
  cache[name] = colors;
  localStorage.setItem('rustic_theme_cache', JSON.stringify(cache));
}

// ─── Palette Cards ───────────────────────────────────────────────

function renderPaletteCard(grid, container, { name, data, isActive, isBuiltin, onActivate, onDelete, onRevert, onExport }) {
  const card = el('div', { class: 'settings-card' + (isActive ? ' settings-card--active' : '') });

  // Header: name + icons
  const header = el('div', { class: 'settings-card__header' });
  const nameEl = el('div', { class: 'settings-card__name' }, name);
  header.appendChild(nameEl);

  if (isBuiltin) {
    const tag = el('span', { class: 'settings-card__tag' }, 'Built-in');
    header.appendChild(tag);
  }

  const actions = el('div', { class: 'settings-card__icons' });

  if (isActive) {
    const revertBtn = el('button', { class: 'settings-card__icon-btn', title: 'Revert to previous' });
    revertBtn.appendChild(icon(ICON_REVERT, 14));
    revertBtn.addEventListener('click', onRevert);
    actions.appendChild(revertBtn);
  }

  if (onDelete) {
    const deleteBtn = el('button', { class: 'settings-card__icon-btn settings-card__icon-btn--danger', title: 'Delete' });
    deleteBtn.appendChild(icon(ICON_TRASH, 14));
    deleteBtn.addEventListener('click', onDelete);
    actions.appendChild(deleteBtn);
  }

  header.appendChild(actions);
  card.appendChild(header);

  // Color swatches
  if (data) {
    const swatches = el('div', { class: 'settings-card__swatches' });
    for (const key of SWATCH_KEYS) {
      if (data[key]) {
        const dot = el('div', { class: 'settings-swatch settings-swatch--small', title: `${key}: ${data[key]}` });
        dot.style.backgroundColor = data[key];
        swatches.appendChild(dot);
      }
    }
    card.appendChild(swatches);
  }

  // Bottom row: Active/Set Active + Export
  const bottomRow = el('div', { class: 'settings-card__bottom-row' });

  const activateBtn = el('button', {
    class: 'settings-card__apply-btn' + (isActive ? ' settings-card__apply-btn--active' : ''),
  }, isActive ? 'Active' : 'Set Active');
  activateBtn.disabled = isActive;
  activateBtn.addEventListener('click', onActivate);
  bottomRow.appendChild(activateBtn);

  if (isActive) {
    const exportBtn = el('button', { class: 'settings-card__icon-btn', title: 'Export theme to clipboard' });
    exportBtn.appendChild(icon(ICON_EXPORT, 14));
    exportBtn.addEventListener('click', onExport);
    bottomRow.appendChild(exportBtn);
  }

  card.appendChild(bottomRow);
  grid.appendChild(card);
}

function renderPaletteCards(container, settings) {
  container.innerHTML = '';

  const themes = settingsStore.getState('themes') || [];
  const savedPalettes = settingsStore.getState('savedPalettes') || [];
  // Single source of truth: `settings.theme.active_theme` (a string name).
  // Built-in themes and saved palettes both compare against this; the old
  // dual tracking (separate `isActive` flag per saved palette) caused the UI
  // to drift out of sync after switching back and forth between palettes.
  const activeThemeName = settings?.theme?.active_theme;
  const themeCache = getThemeCache();

  const currentThemeData = getCurrentTheme();
  if (currentThemeData && activeThemeName) cacheThemeColors(activeThemeName, currentThemeData);

  if (themes.length === 0 && savedPalettes.length === 0) {
    container.appendChild(el('div', { class: 'settings-empty' }, 'No palettes available.'));
    return;
  }

  const grid = el('div', { class: 'settings-card-grid' });

  const revertHandler = async () => {
    const prev = settingsStore.getState('previousPalette');
    const prevName = settingsStore.getState('previousActiveThemeName');
    if (!prev) return;

    // Capture the *current* state as the new "previous" so revert is
    // reversible — the user can revert again to undo the revert.
    const cur = getCurrentTheme();
    const curName = settingsStore.getState('settings')?.theme?.active_theme || null;

    applyTheme(prev);
    if (prevName) {
      try {
        await updateSetting('theme.active_theme', prevName);
        // Cache the reverted theme's colors so the card swatches show
        // immediately even before any backend round-trip lands.
        cacheThemeColors(prevName, prev);
      } catch (e) {
        console.error('[palette] revert failed to update active_theme:', e);
      }
    }

    // Swap snapshot so a second click re-applies what we just left.
    settingsStore.setState({
      previousPalette: cur,
      previousActiveThemeName: curName,
    });

    renderPaletteCards(container, settingsStore.getState('settings'));
  };

  const exportHandler = async () => {
    const theme = getCurrentTheme();
    if (!theme) return;
    try { await navigator.clipboard.writeText(JSON.stringify(theme, null, 2)); } catch { /* ignore */ }
  };

  // Built-in themes
  for (const theme of themes) {
    const isActive = theme.name === activeThemeName;
    const label = theme.kind === 'light' ? `${theme.name} (Light)` : theme.name;

    renderPaletteCard(grid, container, {
      name: label,
      data: themeCache[theme.name] || null,
      isActive,
      isBuiltin: theme.is_builtin,
      onRevert: revertHandler,
      onExport: exportHandler,
      onActivate: async () => {
        snapshotPreviousPalette();
        await updateSetting('theme.active_theme', theme.name);
        try {
          const fullTheme = await api.getActiveTheme();
          if (fullTheme) { applyTheme(fullTheme); cacheThemeColors(theme.name, fullTheme); }
        } catch (e) { console.error('Failed to apply theme:', e); }
        renderPaletteCards(container, settingsStore.getState('settings'));
      },
      onDelete: theme.is_builtin ? null : () => {
        const cache = getThemeCache();
        delete cache[theme.name];
        localStorage.setItem('rustic_theme_cache', JSON.stringify(cache));
        renderPaletteCards(container, settingsStore.getState('settings'));
      },
    });
  }

  // Saved palettes — `isActive` is derived from active_theme, never persisted.
  for (const palette of savedPalettes) {
    renderPaletteCard(grid, container, {
      name: palette.name,
      data: palette.data,
      isActive: palette.name === activeThemeName,
      isBuiltin: false,
      onRevert: revertHandler,
      onExport: exportHandler,
      onActivate: async () => {
        snapshotPreviousPalette();
        applyTheme(palette.data);
        cacheThemeColors(palette.name, palette.data);
        await updateSetting('theme.active_theme', palette.name);
        renderPaletteCards(container, settingsStore.getState('settings'));
      },
      onDelete: () => {
        savePalettes(savedPalettes.filter((p) => p.name !== palette.name));
        renderPaletteCards(container, settingsStore.getState('settings'));
      },
    });
  }

  container.appendChild(grid);
}
