// Theme packs in the browser, for the live theme preview. Mirrors the rules in
// crates/rustic-app/src/themes.rs (`is_safe_color`, `validate`, `Pack::resolve`,
// `active`) so a pack looks the same here as it will once installed. Custom CSS
// is never part of a preview.
import BASES from './theme-bases.json';

export const FORMAT = 2;

/** The 31 color slots of a mode, in file order. */
export const COLOR_SLOTS = [
  'bg_hard', 'bg', 'bg_soft', 'bg1', 'bg2', 'bg3', 'bg4',
  'fg', 'fg1', 'fg2', 'fg3', 'fg4',
  'accent', 'primary', 'border',
  'bright_red', 'bright_green', 'bright_yellow', 'bright_blue', 'bright_purple', 'bright_aqua', 'bright_orange',
  'token_keyword', 'token_string', 'token_comment', 'token_function', 'token_type',
  'token_variable', 'token_number', 'token_operator', 'token_punctuation',
];

export const STYLE_KEYS = [
  'font_sans', 'font_mono', 'radius', 'motion', 'border_width',
  'shadow', 'surface', 'density', 'button', 'texture', 'ink',
];

/** Allowed values for the enum-like style keys. */
export const STYLE_ENUMS = {
  motion: ['full', 'reduced'],
  shadow: ['none', 'soft', 'strong', 'glow', 'hard'],
  surface: ['solid', 'glass'],
  density: ['compact', 'normal', 'comfortable'],
  button: ['solid', 'pill', 'raised', 'brutal', 'glow'],
  texture: ['none', 'grain', 'grid', 'dots', 'scanlines', 'paper'],
  ink: ['none', 'sketch'],
};

const COLOR_FUNCS = ['rgb(', 'rgba(', 'hsl(', 'hsla(', 'hwb(', 'lab(', 'lch(', 'oklab(', 'oklch(', 'color(', 'color-mix('];

/** Whether `v` is a single CSS color and nothing that could carry other CSS. */
export function isSafeColor(v) {
  if (typeof v !== 'string') return false;
  const s = v.trim();
  if (!s || s.length > 80) return false;
  const lower = s.toLowerCase();
  if (/[{};<>\\"'@!]/.test(s) || lower.includes('url')) return false;
  if (s.startsWith('#')) return /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(s);
  if (COLOR_FUNCS.some((f) => lower.startsWith(f))) return lower.endsWith(')') && /^[a-z0-9 .,%/()+#-]+$/.test(lower);
  return /^[a-z]+$/.test(lower);
}

/** Whether a style value (font stack, length, keyword) is plain enough to use in CSS. */
export function isSafeStyleValue(v) {
  if (typeof v !== 'string') return false;
  const s = v.trim();
  return !!s && s.length <= 200 && !s.toLowerCase().includes('url') && /^[A-Za-z0-9 .,%_()'"/-]+$/.test(s);
}

const isObject = (v) => !!v && typeof v === 'object' && !Array.isArray(v);
const isBlank = (v) => v == null || (typeof v === 'string' && !v.trim());

/**
 * Clean copy of `input` holding only values Rustic would accept, plus the list
 * of `{ path, message }` problems for everything that was dropped. Blank values
 * are dropped silently (they mean "inherit"). Never throws.
 */
export function sanitizePack(input) {
  const problems = [];
  const note = (path, message) => problems.push({ path, message });
  const src = isObject(input) ? input : {};
  if (!isObject(input)) note('', 'The theme must be a JSON object');
  const pack = { format: FORMAT, name: 'Untitled' };

  if (typeof src.format === 'number' && src.format > FORMAT) {
    note('format', `This theme needs a newer Rustic (format ${src.format}, supported ${FORMAT})`);
  }
  const name = typeof src.name === 'string' ? src.name.trim() : '';
  if (name && name.length <= 80) pack.name = name;
  else note('name', 'The theme needs a name (up to 80 characters)');
  for (const key of ['author', 'version', 'description']) {
    if (typeof src[key] === 'string' && src[key].trim()) pack[key] = src[key].trim();
  }

  for (const mode of ['dark', 'light']) {
    if (src[mode] == null) continue;
    if (!isObject(src[mode])) {
      note(mode, `"${mode}" must be an object of colors`);
      continue;
    }
    const colors = {};
    for (const slot of COLOR_SLOTS) {
      const value = src[mode][slot];
      if (isBlank(value)) continue;
      if (isSafeColor(value)) colors[slot] = value.trim();
      else note(`${mode}.${slot}`, `"${String(value).slice(0, 40)}" isn't a valid color`);
    }
    pack[mode] = colors;
  }
  if (!pack.dark && !pack.light) note('', 'The theme defines no colors');

  const style = {};
  if (src.style != null && !isObject(src.style)) note('style', '"style" must be an object');
  for (const key of STYLE_KEYS) {
    const value = isObject(src.style) ? src.style[key] : undefined;
    if (isBlank(value)) continue;
    const allowed = STYLE_ENUMS[key];
    if (!isSafeStyleValue(value)) note(`style.${key}`, `"${String(value).slice(0, 40)}" isn't allowed`);
    else if (allowed && !allowed.includes(value.trim())) note(`style.${key}`, `Use one of: ${allowed.join(', ')}`);
    else style[key] = value.trim();
  }
  pack.style = style;

  if (typeof src.css === 'string' && src.css.trim()) note('css', 'Custom CSS is not shown in the preview');
  return { pack, problems };
}

/** Modes a pack provides. */
export function packModes(pack) {
  return ['dark', 'light'].filter((m) => isObject(pack?.[m]));
}

/** Full color theme of a sanitized `pack` for `mode`, falling back to its only mode. */
export function resolvePack(pack, mode) {
  const modes = packModes(pack);
  if (!modes.length) return { ...BASES[mode === 'light' ? 'light' : 'dark'], name: pack.name };
  const kind = modes.includes(mode) ? mode : modes[0];
  return { ...BASES[kind], ...pack[kind], name: pack.name, kind };
}

/**
 * What the app's theme bridge paints for a sanitized `pack`: the same shape as
 * the backend's `theme_active`. A pack with one mode is always shown in it.
 */
export function packToActive(pack, mode = 'dark') {
  const modes = packModes(pack);
  const variant = (m) => (modes.includes(m) || !modes.length ? resolvePack(pack, m) : { ...BASES[m] });
  return {
    id: pack.name,
    name: pack.name,
    mode: modes.length === 1 ? modes[0] : mode,
    dark: variant('dark'),
    light: variant('light'),
    style: { ...pack.style },
    css: null,
    fallback: false,
  };
}

/** The Obsidian built-in as a pack (dark only). */
export function obsidianPack() {
  const colors = { ...BASES.dark };
  delete colors.name;
  delete colors.kind;
  return { format: FORMAT, name: 'Obsidian', dark: colors, style: {} };
}
