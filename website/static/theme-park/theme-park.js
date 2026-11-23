// Theme Park page: browse shared themes, preview one on the live mock window,
// and build a new one. The draft lives in localStorage; nothing is sent until
// Share. All text from the API is rendered with textContent, and colors are
// only ever assigned through element.style after passing isSafeColor.
import { mountThemePreview } from '/theme-park/preview-host.js';

const PREVIEW_SRC = '/theme-park/preview/index.html';
const DRAFT_KEY = 'rustic.themepark.draft';
const PER_PAGE = 24;
const MODES = ['dark', 'light'];

const SLOT_GROUPS = [
  ['Surfaces', ['bg_hard', 'bg', 'bg_soft', 'bg1', 'bg2', 'bg3', 'bg4', 'border']],
  ['Text', ['fg', 'fg1', 'fg2', 'fg3', 'fg4']],
  ['Accents', ['accent', 'primary', 'bright_red', 'bright_green', 'bright_yellow', 'bright_blue', 'bright_purple', 'bright_aqua', 'bright_orange']],
  ['Code', ['token_keyword', 'token_string', 'token_comment', 'token_function', 'token_type', 'token_variable', 'token_number', 'token_operator', 'token_punctuation']],
];
const STYLE_FIELDS = [
  ['font_sans', 'UI font', null, '"Inter", sans-serif'],
  ['font_mono', 'Code font', null, '"JetBrains Mono", monospace'],
  ['radius', 'Corner radius', null, '0.5rem'],
  ['border_width', 'Border width', null, '1px'],
  ['shadow', 'Shadow', ['none', 'soft', 'strong', 'glow', 'hard']],
  ['surface', 'Surface', ['solid', 'glass']],
  ['density', 'Density', ['compact', 'normal', 'comfortable']],
  ['button', 'Buttons', ['solid', 'pill', 'raised', 'brutal', 'glow']],
  ['texture', 'Texture', ['none', 'grain', 'grid', 'dots', 'scanlines', 'paper']],
  ['ink', 'Outlines', ['none', 'sketch']],
  ['motion', 'Motion', ['full', 'reduced']],
];
const COLOR_FUNCS = ['rgb(', 'rgba(', 'hsl(', 'hsla(', 'hwb(', 'lab(', 'lch(', 'oklab(', 'oklch(', 'color(', 'color-mix('];

const $ = (id) => document.getElementById(id);
const grid = $('grid');
const empty = $('empty');
const more = $('more');
const detail = $('detail');
const creator = $('creator');
const form = $('creator-form');

const state = { mode: 'all', q: '', page: 1, total: 0, loading: false, seq: 0, bases: { dark: {}, light: {} } };
let detailView = null;
let creatorView = null;
let draft = null;
let editMode = 'dark';

/** Element helper: tag, class, text. */
function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
}

/** Same rule as the app and the server: one CSS color, nothing that could carry other CSS. */
function isSafeColor(v) {
  if (typeof v !== 'string') return false;
  const s = v.trim();
  if (!s || s.length > 80) return false;
  const lower = s.toLowerCase();
  if (/[{};<>\\"'@!]/.test(s) || lower.includes('url')) return false;
  if (s.startsWith('#')) return /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(s);
  if (COLOR_FUNCS.some((f) => lower.startsWith(f))) return lower.endsWith(')') && /^[a-z0-9 .,%/()+#-]+$/.test(lower);
  return /^[a-z]+$/.test(lower);
}

const probe = document.createElement('canvas').getContext('2d', { willReadFrequently: true });
/** `#rrggbb` for any safe CSS color (alpha dropped), for the color picker; null when the browser can't paint it. */
function toHex(color) {
  if (!isSafeColor(color) || !probe) return null;
  probe.clearRect(0, 0, 1, 1);
  probe.fillStyle = '#000';
  probe.fillStyle = color.trim();
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = probe.getImageData(0, 0, 1, 1).data;
  if (a === 0 && color.trim().toLowerCase() !== 'transparent') return null;
  return `#${[r, g, b].map((n) => n.toString(16).padStart(2, '0')).join('')}`;
}

/** The colors a theme shows in `mode`: its own values over the app's base palette. */
function resolved(pack, mode) {
  const out = { ...state.bases[mode] };
  for (const [slot, value] of Object.entries(pack?.[mode] || {})) {
    if (isSafeColor(value)) out[slot] = value;
  }
  return out;
}

/** A small static picture of a theme in one mode, drawn from its colors. */
function shot(pack, mode) {
  const c = resolved(pack, mode);
  const paint = (node, prop, slot) => { if (isSafeColor(c[slot])) node.style[prop] = c[slot]; };
  const box = el('div', 'park-shot');
  paint(box, 'background', 'bg');
  const bar = el('div', 'park-shot-bar');
  for (const slot of ['bg3', 'bg2', 'bg2']) { const i = el('i'); paint(i, 'background', slot); bar.append(i); }
  const lines = el('div', 'park-shot-lines');
  for (const [slot, width] of [['token_keyword', '38%'], ['token_function', '64%'], ['token_string', '52%'], ['token_comment', '44%'], ['fg', '58%']]) {
    const i = el('i');
    paint(i, 'background', slot);
    i.style.width = width;
    lines.append(i);
  }
  const pill = el('span', 'park-shot-pill', mode);
  paint(pill, 'background', 'primary');
  paint(pill, 'color', 'bg');
  box.append(bar, lines, pill);
  return box;
}

function renderCard(theme) {
  const li = el('li', 'park-card');
  li.tabIndex = 0;
  li.setAttribute('role', 'button');
  const modes = theme.modes.filter((m) => MODES.includes(m));
  const shots = el('div', `park-shots${modes.length === 2 ? ' two' : ''}`);
  for (const m of modes) shots.append(shot(theme.pack, m));
  const body = el('div', 'park-card-body');
  body.append(el('h3', 'park-card-title', theme.name));
  if (theme.description) body.append(el('p', 'park-card-desc', theme.description));
  const tags = el('div', 'iss-tags');
  if (theme.author) tags.append(el('span', null, `by ${theme.author}`));
  for (const m of modes) tags.append(el('span', 'iss-tag', m));
  body.append(tags);
  li.append(shots, body);
  const open = () => openDetail(theme);
  li.addEventListener('click', open);
  li.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } });
  return li;
}

/** Fetch a page of themes; `reset` starts over (tab or search change). */
async function load(reset) {
  const seq = ++state.seq;
  if (reset) {
    state.page = 1;
    grid.replaceChildren(...Array.from({ length: 6 }, () => el('li', 'park-skel')));
    empty.hidden = true;
  }
  state.loading = true;
  more.disabled = true;
  const params = new URLSearchParams({ mode: state.mode, page: String(state.page), per_page: String(PER_PAGE) });
  if (state.q) params.set('q', state.q);
  try {
    const res = await fetch(`/api/themes?${params}`, { headers: { Accept: 'application/json' } });
    const data = await res.json().catch(() => ({}));
    if (seq !== state.seq) return;
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    state.total = data.total || 0;
    const cards = (data.themes || []).map(renderCard);
    if (reset) grid.replaceChildren(...cards); else grid.append(...cards);
    const shown = grid.querySelectorAll('.park-card').length;
    empty.hidden = shown > 0;
    empty.textContent = state.q ? 'No themes match your search.' : 'No themes here yet. Create the first one.';
    more.hidden = shown >= state.total;
  } catch (e) {
    if (seq !== state.seq) return;
    grid.replaceChildren();
    empty.hidden = false;
    empty.textContent = `Couldn't load themes: ${e.message}`;
    more.hidden = true;
  } finally {
    if (seq === state.seq) { state.loading = false; more.disabled = false; }
  }
}

/** Mark which button of a tab group is selected. */
function selectTab(group, attr, value) {
  group.querySelectorAll('.iss-tab').forEach((t) => t.setAttribute('aria-selected', String(t.dataset[attr] === value)));
}

function openDetail(theme) {
  const url = new URL(location.href);
  url.searchParams.set('id', theme.id);
  history.replaceState(null, '', url);
  $('detail-title').textContent = theme.name;
  const meta = $('detail-meta');
  meta.replaceChildren();
  if (theme.author) meta.append(el('span', null, `by ${theme.author}`));
  for (const m of theme.modes) meta.append(el('span', 'iss-tag', m));
  if (theme.seed) meta.append(el('span', 'iss-tag feature', 'from Rustic'));
  $('detail-desc').textContent = theme.description || '';
  $('detail-download').href = theme.file;
  const modes = theme.modes.filter((m) => MODES.includes(m));
  let mode = modes[0] || 'dark';
  const modeTabs = $('detail-modes');
  modeTabs.hidden = modes.length < 2;
  selectTab(modeTabs, 'pick', mode);
  selectTab($('detail-scenes'), 'scene', 'workbench');
  detailView?.destroy();
  if (!detail.open) detail.showModal();
  detailView = mountThemePreview($('detail-preview'), { src: PREVIEW_SRC, pack: theme.pack, mode });
  modeTabs.onclick = (e) => {
    const pick = e.target.closest('[data-pick]')?.dataset.pick;
    if (!pick) return;
    mode = pick;
    selectTab(modeTabs, 'pick', mode);
    detailView.setTheme(theme.pack, mode);
  };
  $('detail-scenes').onclick = (e) => {
    const scene = e.target.closest('[data-scene]')?.dataset.scene;
    if (!scene) return;
    selectTab($('detail-scenes'), 'scene', scene);
    detailView.setScene(scene);
  };
  $('detail-copy').onclick = async () => {
    const label = $('detail-copy').querySelector('span');
    try {
      await navigator.clipboard.writeText(`${location.origin}${theme.url}`);
      label.textContent = 'Link copied';
    } catch {
      label.textContent = 'Copy failed';
    }
    setTimeout(() => { label.textContent = 'Copy link'; }, 1600);
  };
  $('detail-remix').onclick = () => {
    detail.close();
    openCreator(draftFromPack(theme.pack, `${theme.name} remix`));
  };
}

/** A fresh draft: both palettes filled from the app's defaults, dark only. */
function blankDraft() {
  return { name: '', author: '', description: '', modes: 'dark', dark: { ...state.bases.dark }, light: { ...state.bases.light }, style: {} };
}

/** Draft for editing an existing theme file object. */
function draftFromPack(pack, name) {
  const d = blankDraft();
  const src = pack && typeof pack === 'object' ? pack : {};
  d.name = String(name ?? src.name ?? '').slice(0, 80);
  d.author = typeof src.author === 'string' ? src.author : '';
  d.description = typeof src.description === 'string' ? src.description : '';
  const has = MODES.filter((m) => src[m] && typeof src[m] === 'object');
  d.modes = has.length ? has.join(',') : 'dark';
  for (const m of has) {
    for (const [slot, value] of Object.entries(src[m])) {
      if (slot in d[m] && typeof value === 'string' && value.trim()) d[m][slot] = value.trim();
    }
  }
  if (src.style && typeof src.style === 'object') {
    for (const [key] of STYLE_FIELDS) {
      if (typeof src.style[key] === 'string' && src.style[key].trim()) d.style[key] = src.style[key].trim();
    }
  }
  return d;
}

/** The theme file object for the current draft: only the chosen modes, blanks left out. */
function draftPack() {
  const pack = { format: 2, name: draft.name.trim() || 'Untitled' };
  for (const key of ['author', 'description']) {
    if (draft[key].trim()) pack[key] = draft[key].trim();
  }
  for (const m of draft.modes.split(',')) {
    pack[m] = Object.fromEntries(Object.entries(draft[m]).filter(([, v]) => typeof v === 'string' && v.trim()));
  }
  pack.style = Object.fromEntries(Object.entries(draft.style).filter(([, v]) => typeof v === 'string' && v.trim()));
  return pack;
}

function saveDraft() {
  try {
    localStorage.setItem(DRAFT_KEY, JSON.stringify(draft));
  } catch {
    /* private mode or full storage: the draft just isn't kept */
  }
}

function loadDraft() {
  try {
    const d = JSON.parse(localStorage.getItem(DRAFT_KEY) || 'null');
    if (!d || typeof d !== 'object') return null;
    const base = blankDraft();
    for (const key of ['name', 'author', 'description']) if (typeof d[key] === 'string') base[key] = d[key];
    if (['dark', 'light', 'dark,light'].includes(d.modes)) base.modes = d.modes;
    for (const m of MODES) {
      for (const slot of Object.keys(base[m])) if (typeof d[m]?.[slot] === 'string') base[m][slot] = d[m][slot];
    }
    for (const [key] of STYLE_FIELDS) if (typeof d.style?.[key] === 'string') base.style[key] = d.style[key];
    return base;
  } catch {
    return null;
  }
}

function showCreatorError(msg) {
  const box = $('creator-error');
  box.textContent = msg || '';
  box.hidden = !msg;
}

/** Repaint the creator's preview and save the draft. */
function draftChanged() {
  saveDraft();
  creatorView?.setTheme(draftPack(), editMode);
}

function renderColors() {
  const host = $('creator-colors');
  host.replaceChildren();
  for (const [title, slots] of SLOT_GROUPS) {
    host.append(el('h3', 'park-group', title));
    const wrap = el('div', 'park-slots');
    for (const slot of slots) {
      const row = el('label', 'park-slot');
      const picker = el('input');
      picker.type = 'color';
      const text = el('input');
      text.type = 'text';
      text.spellcheck = false;
      text.maxLength = 80;
      const sync = () => {
        const value = draft[editMode][slot] || '';
        text.value = value;
        const hex = toHex(value);
        if (hex) picker.value = hex;
        row.classList.toggle('bad', !!value.trim() && !isSafeColor(value));
      };
      picker.addEventListener('input', () => { draft[editMode][slot] = picker.value; sync(); draftChanged(); });
      text.addEventListener('input', () => {
        draft[editMode][slot] = text.value;
        row.classList.toggle('bad', !!text.value.trim() && !isSafeColor(text.value));
        const hex = toHex(text.value);
        if (hex) picker.value = hex;
        draftChanged();
      });
      row.append(picker, el('span', null, slot), text);
      sync();
      wrap.append(row);
    }
    host.append(wrap);
  }
}

function renderStyles() {
  const host = $('creator-styles');
  host.replaceChildren();
  for (const [key, label, options, placeholder] of STYLE_FIELDS) {
    const field = el('label', 'iss-field', label);
    let input;
    if (options) {
      input = el('select');
      input.append(new Option('Default', ''));
      for (const o of options) input.append(new Option(o, o));
    } else {
      input = el('input');
      input.maxLength = 200;
      input.placeholder = placeholder;
    }
    input.value = draft.style[key] || '';
    input.addEventListener('input', () => { draft.style[key] = input.value; draftChanged(); });
    field.append(input);
    host.append(field);
  }
}

/** Fill the creator's controls from `draft`. */
function renderCreator() {
  form.elements.name.value = draft.name;
  form.elements.author.value = draft.author;
  form.elements.description.value = draft.description;
  form.elements.modes.value = draft.modes;
  const modes = draft.modes.split(',');
  if (!modes.includes(editMode)) editMode = modes[0];
  const tabs = $('creator-modes');
  tabs.hidden = modes.length < 2;
  selectTab(tabs, 'pick', editMode);
  renderColors();
  renderStyles();
}

function openCreator(next) {
  draft = next || loadDraft() || blankDraft();
  editMode = draft.modes.split(',')[0];
  showCreatorError('');
  renderCreator();
  if (!creator.open) creator.showModal();
  creatorView?.destroy();
  creatorView = mountThemePreview($('creator-preview'), {
    src: PREVIEW_SRC,
    pack: draftPack(),
    mode: editMode,
    onProblems: (problems) => {
      const list = $('creator-problems');
      list.replaceChildren(...problems.map((p) => el('li', null, p.path ? `${p.path}: ${p.message}` : p.message)));
      list.hidden = problems.length === 0;
    },
  });
  saveDraft();
}

/** Save `text` as a file download named `name`. */
function downloadText(name, text) {
  const a = el('a');
  a.href = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

function fileName(name) {
  const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60);
  return `${slug || 'theme'}.rustic-theme.json`;
}

async function share(e) {
  e.preventDefault();
  if (!draft.name.trim()) {
    showCreatorError('Give the theme a name before sharing.');
    form.elements.name.focus();
    return;
  }
  showCreatorError('');
  const btn = $('creator-share');
  const label = btn.querySelector('span');
  btn.disabled = true;
  label.textContent = 'Sharing…';
  try {
    const res = await fetch('/api/themes', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pack: draftPack(), website: form.elements.website.value }),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    localStorage.removeItem(DRAFT_KEY);
    creator.close();
    state.mode = 'all';
    state.q = '';
    $('search').value = '';
    selectTab(document.querySelector('.iss-toolbar .iss-tabs'), 'mode', 'all');
    await load(true);
    if (data.id) await openById(data.id);
  } catch (err) {
    showCreatorError(err.message);
  } finally {
    btn.disabled = false;
    label.textContent = 'Share publicly';
  }
}

async function openById(id) {
  try {
    const res = await fetch(`/api/themes/${encodeURIComponent(id)}`);
    const theme = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(theme.error || `HTTP ${res.status}`);
    openDetail(theme);
  } catch (e) {
    empty.hidden = false;
    empty.textContent = `Couldn't open that theme: ${e.message}`;
  }
}

document.querySelectorAll('.iss-toolbar .iss-tab').forEach((tab) => {
  tab.addEventListener('click', () => {
    if (state.mode === tab.dataset.mode) return;
    state.mode = tab.dataset.mode;
    selectTab(tab.parentElement, 'mode', state.mode);
    load(true);
  });
});

let timer = null;
$('search').addEventListener('input', () => {
  clearTimeout(timer);
  timer = setTimeout(() => { state.q = $('search').value.trim(); load(true); }, 250);
});
more.addEventListener('click', () => { if (!state.loading) { state.page += 1; load(false); } });

for (const dlg of [detail, creator]) {
  dlg.addEventListener('click', (e) => {
    if (e.target === dlg || e.target.closest('[data-close]')) dlg.close();
  });
}
detail.addEventListener('close', () => {
  detailView?.destroy();
  detailView = null;
  const url = new URL(location.href);
  url.searchParams.delete('id');
  history.replaceState(null, '', url);
});
creator.addEventListener('close', () => {
  creatorView?.destroy();
  creatorView = null;
});

$('create-btn').addEventListener('click', () => openCreator());
form.addEventListener('submit', share);
for (const key of ['name', 'author', 'description']) {
  form.elements[key].addEventListener('input', () => { draft[key] = form.elements[key].value; draftChanged(); });
}
form.elements.modes.addEventListener('change', () => {
  draft.modes = form.elements.modes.value;
  renderCreator();
  draftChanged();
});
$('creator-modes').addEventListener('click', (e) => {
  const pick = e.target.closest('[data-pick]')?.dataset.pick;
  if (!pick || pick === editMode) return;
  editMode = pick;
  selectTab($('creator-modes'), 'pick', editMode);
  renderColors();
  draftChanged();
});
$('creator-reset').addEventListener('click', () => {
  localStorage.removeItem(DRAFT_KEY);
  openCreator(blankDraft());
});
$('creator-download').addEventListener('click', () => {
  const pack = draftPack();
  downloadText(fileName(pack.name), `${JSON.stringify(pack, null, 2)}\n`);
});
$('creator-file').addEventListener('change', async (e) => {
  const file = e.target.files?.[0];
  e.target.value = '';
  if (!file) return;
  try {
    if (file.size > 512 * 1024) throw new Error('That file is larger than 512 KB.');
    const pack = JSON.parse(await file.text());
    if (!pack || typeof pack !== 'object' || Array.isArray(pack)) throw new Error('That file is not a theme.');
    openCreator(draftFromPack(pack));
    showCreatorError(typeof pack.css === 'string' && pack.css.trim() ? 'This file has custom CSS. Shared themes cannot include it, so it was left out.' : '');
  } catch (err) {
    showCreatorError(`Couldn't open that file: ${err.message}`);
  }
});

/** Load the app's base palettes (defaults for every color slot), then the first page. */
async function start() {
  try {
    const bases = await (await fetch('/theme-park/bases.json')).json();
    for (const m of MODES) {
      const { name, kind, ...colors } = bases[m] || {};
      state.bases[m] = colors;
    }
  } catch {
    /* cards and the creator still work, with empty defaults */
  }
  await load(true);
  const deep = new URLSearchParams(location.search).get('id');
  if (deep && /^\d+$/.test(deep)) openById(deep);
  else if (new URLSearchParams(location.search).has('create')) openCreator();
}
start();
