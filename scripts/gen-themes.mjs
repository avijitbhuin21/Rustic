// Palette generator for Rustic built-in themes. Each theme is a handful of
// OKLCH decisions (neutral hue/chroma, base lightness, accent, status hues);
// every slot is derived from fixed per-mode lightness ladders and then nudged
// until it meets its contrast target. Run: node scripts/gen-themes.mjs
import { writeFileSync, readFileSync } from 'node:fs';

const OUT = 'crates/rustic-app/themes';

// ── OKLCH → sRGB ──────────────────────────────────────────────────────────
function oklchToLinear(L, C, h) {
  const a = C * Math.cos((h * Math.PI) / 180), b = C * Math.sin((h * Math.PI) / 180);
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.291485548 * b;
  const l = l_ ** 3, m = m_ ** 3, s = s_ ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}
const inGamut = (rgb) => rgb.every((v) => v >= -1e-4 && v <= 1 + 1e-4);
const gamma = (x) => (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055);
function hex({ L, C, h }) {
  let c = C, rgb = oklchToLinear(L, c, h);
  while (!inGamut(rgb) && c > 0) { c = Math.max(0, c - 0.002); rgb = oklchToLinear(L, c, h); }
  return '#' + rgb.map((v) => Math.round(Math.min(1, Math.max(0, gamma(v))) * 255).toString(16).padStart(2, '0')).join('');
}
function lum(hx) {
  const v = [1, 3, 5].map((i) => parseInt(hx.slice(i, i + 2), 16) / 255).map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
}
const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };

/** Move L (away from `against`) until contrast ≥ min. */
function fit(col, against, min) {
  const c = { ...col };
  const dir = lum(hex(c)) >= lum(against) ? 1 : -1;
  for (let i = 0; i < 120 && contrast(hex(c), against) < min; i++) c.L = Math.min(1, Math.max(0, c.L + 0.004 * dir));
  return hex(c);
}

// ── Ladders ───────────────────────────────────────────────────────────────
const STATUS_HUES = { red: 25, green: 145, yellow: 88, blue: 252, purple: 302, aqua: 192, orange: 55 };

function build(mode, t) {
  const m = t[mode];
  const dark = mode === 'dark';
  const n = (L, C = m.neutralC) => ({ L, C, h: m.neutralH });
  const f = (L) => ({ L, C: m.fgC ?? m.neutralC * 0.6, h: m.fgH ?? m.neutralH });
  const b = m.base, F = m.fg;
  const s = dark
    ? { bg_hard: b - 0.03, bg: b, bg_soft: b + 0.012, bg1: b + 0.024, bg2: b + 0.058, bg3: b + 0.1, bg4: b + 0.15, border: b + 0.07 }
    : { bg_hard: b - 0.035, bg: b, bg_soft: b - 0.016, bg1: Math.min(1, b + 0.013), bg2: b - 0.045, bg3: b - 0.085, bg4: b - 0.14, border: b - 0.1 };
  const out = {};
  for (const [k, L] of Object.entries(s)) out[k] = hex(n(L, k === 'bg1' && !dark ? m.neutralC * 0.5 : m.neutralC));
  if (m.borderL != null) out.border = hex(n(m.borderL));
  const bg = out.bg, bg2 = out.bg2;
  out.fg = fit(f(F), bg, m.fgMin ?? 12);
  out.fg1 = out.fg;
  out.fg2 = fit(f(dark ? F - 0.12 : F + 0.12), bg, 7);
  out.fg3 = fit(f(dark ? F - 0.27 : F + 0.26), bg2, 4.6);
  out.fg4 = fit(f(dark ? F - 0.42 : F + 0.42), bg, 2.6);
  const hues = { ...STATUS_HUES, ...(t.hues || {}), ...(m.hues || {}) };
  for (const [k, h] of Object.entries(hues)) out[`bright_${k}`] = fit({ L: m.statusL, C: m.statusC * (k === 'yellow' ? 1.1 : 1), h }, bg, 4.6);
  const acc = { L: m.accent[0], C: m.accent[1], h: m.accent[2] };
  out.accent = fit(acc, bg, 3.2);
  out.primary = m.primary ? hex({ L: m.primary[0], C: m.primary[1], h: m.primary[2] }) : out.accent;
  if (Math.max(contrast(out.primary, bg), contrast(out.primary, out.fg)) < 4.5) throw new Error(`${t.name} ${mode}: primary has no readable text color`);
  out.token_keyword = out.bright_purple;
  out.token_string = out.bright_green;
  out.token_comment = fit(f(dark ? F - 0.36 : F + 0.34), bg, 3.4);
  out.token_function = out.bright_blue;
  out.token_type = out.bright_aqua;
  out.token_variable = out.fg;
  out.token_number = out.bright_orange;
  out.token_operator = out.fg2;
  out.token_punctuation = out.fg3;
  return out;
}

// ── Themes ────────────────────────────────────────────────────────────────
const THEMES = [
  {
    file: 'basalt', name: 'Basalt',
    description: 'Quiet, premium neutrals with a restrained indigo accent — calm enough to disappear behind your code.',
    style: { radius: '0.5rem', shadow: 'soft' },
    dark: { neutralH: 272, neutralC: 0.006, base: 0.175, fg: 0.945, statusL: 0.78, statusC: 0.11, accent: [0.74, 0.12, 278] },
    light: { neutralH: 272, neutralC: 0.005, base: 0.988, fg: 0.22, statusL: 0.5, statusC: 0.14, accent: [0.5, 0.17, 276] },
  },
  {
    file: 'hearth', name: 'Hearth',
    description: 'Warm and low-contrast: toasted browns, cream text and muted earth tones that stay easy on the eyes for long sessions.',
    style: { radius: '0.625rem', shadow: 'soft' },
    hues: { red: 32, green: 125, yellow: 82, blue: 222, purple: 350, aqua: 172, orange: 58 },
    dark: { neutralH: 68, neutralC: 0.017, base: 0.215, fg: 0.875, fgH: 85, fgC: 0.032, fgMin: 9.5, statusL: 0.74, statusC: 0.085, accent: [0.76, 0.11, 66] },
    light: { neutralH: 82, neutralC: 0.024, base: 0.968, fg: 0.29, fgH: 60, fgC: 0.03, fgMin: 10, statusL: 0.5, statusC: 0.11, accent: [0.52, 0.13, 50] },
  },
  {
    file: 'petal', name: 'Petal',
    description: 'Soft pastels on a lavender-tinted base — friendly, gentle colors with a mauve accent.',
    style: { radius: '0.75rem', shadow: 'soft' },
    hues: { red: 12, green: 152, yellow: 84, blue: 248, purple: 305, aqua: 196, orange: 48 },
    dark: { neutralH: 287, neutralC: 0.03, base: 0.2, fg: 0.92, fgH: 282, fgC: 0.035, statusL: 0.82, statusC: 0.085, accent: [0.8, 0.1, 305] },
    light: { neutralH: 300, neutralC: 0.014, base: 0.985, fg: 0.3, fgH: 290, fgC: 0.04, statusL: 0.52, statusC: 0.13, accent: [0.52, 0.15, 302] },
  },
  {
    file: 'midnight', name: 'Midnight',
    description: 'Deep blue night: ink-navy surfaces, cool moonlit text and a clear sky-blue accent.',
    style: { radius: '0.5rem', shadow: 'strong' },
    hues: { red: 15, green: 142, yellow: 84, blue: 256, purple: 296, aqua: 200, orange: 52 },
    dark: { neutralH: 264, neutralC: 0.034, base: 0.19, fg: 0.91, fgH: 268, fgC: 0.04, statusL: 0.78, statusC: 0.11, accent: [0.76, 0.13, 258] },
    light: { neutralH: 255, neutralC: 0.01, base: 0.985, fg: 0.25, fgH: 262, fgC: 0.04, statusL: 0.5, statusC: 0.15, accent: [0.52, 0.17, 258] },
  },
  {
    file: 'onyx', name: 'Onyx',
    description: 'High-contrast and punchy: true black and white with a warm peach accent and mint highlights.',
    style: { radius: '0.375rem', shadow: 'none' },
    hues: { red: 22, green: 165, yellow: 92, blue: 240, purple: 318, aqua: 175, orange: 62 },
    dark: { neutralH: 0, neutralC: 0, base: 0.145, fg: 0.985, statusL: 0.85, statusC: 0.13, accent: [0.86, 0.11, 66] },
    light: { neutralH: 0, neutralC: 0, base: 0.995, fg: 0.16, fgMin: 15, statusL: 0.48, statusC: 0.17, accent: [0.56, 0.17, 45] },
  },
  {
    file: 'neo-brutal', name: 'Neo Brutal', keepStyle: true,
    description: 'Bold: square corners, thick outlines, hard offset shadows and chunky buttons in electric yellow.',
    hues: { red: 25, green: 150, yellow: 98, blue: 262, purple: 305, aqua: 195, orange: 50 },
    dark: { neutralH: 95, neutralC: 0.005, base: 0.165, fg: 0.97, statusL: 0.8, statusC: 0.17, accent: [0.91, 0.18, 102], borderL: 0.5 },
    light: { neutralH: 95, neutralC: 0.028, base: 0.975, fg: 0.17, fgMin: 15, statusL: 0.5, statusC: 0.19, accent: [0.48, 0.22, 264], primary: [0.89, 0.17, 98], borderL: 0.32 },
  },
  {
    file: 'grove', name: 'Grove', keepStyle: true,
    description: 'Organic: forest shade and oat paper, sage and terracotta accents, soft round shapes and a paper grain.',
    hues: { red: 35, green: 138, yellow: 85, blue: 225, purple: 340, aqua: 180, orange: 52 },
    dark: { neutralH: 155, neutralC: 0.016, base: 0.205, fg: 0.9, fgH: 92, fgC: 0.028, fgMin: 10.5, statusL: 0.77, statusC: 0.09, accent: [0.8, 0.09, 135] },
    light: { neutralH: 85, neutralC: 0.022, base: 0.975, fg: 0.27, fgH: 145, fgC: 0.025, statusL: 0.49, statusC: 0.11, accent: [0.47, 0.09, 152] },
  },
  {
    file: 'neon-grid', name: 'Neon Grid', keepStyle: true,
    description: 'Futuristic: deep space ink, cyan neon and magenta highlights, frosted-glass panels and a faint grid.',
    hues: { red: 8, green: 160, yellow: 95, blue: 250, purple: 325, aqua: 205, orange: 55 },
    dark: { neutralH: 276, neutralC: 0.042, base: 0.155, fg: 0.95, fgH: 230, fgC: 0.025, statusL: 0.82, statusC: 0.16, accent: [0.86, 0.14, 205] },
    light: { neutralH: 255, neutralC: 0.012, base: 0.985, fg: 0.21, fgH: 262, fgC: 0.04, statusL: 0.5, statusC: 0.19, accent: [0.5, 0.21, 262] },
  },
];

const report = [];
for (const t of THEMES) {
  const path = `${OUT}/${t.file}.rustic-theme.json`;
  let style = t.style;
  if (t.keepStyle) style = JSON.parse(readFileSync(path, 'utf8')).style;
  const dark = build('dark', t), light = build('light', t);
  const pack = { format: 2, name: t.name, author: 'Rustic', version: '2.0.0', description: t.description, dark, light, style };
  writeFileSync(path, JSON.stringify(pack, null, 2) + '\n');
  for (const [mode, c] of [['dark', dark], ['light', light]]) {
    const pfg = contrast(c.primary, c.bg) >= contrast(c.primary, c.fg) ? 'bg' : 'fg';
    report.push(`${t.name.padEnd(10)} ${mode.padEnd(5)} bg ${c.bg} fg ${c.fg} ${contrast(c.fg, c.bg).toFixed(1)}:1 | muted ${contrast(c.fg3, c.bg2).toFixed(1)} | accent ${c.accent} | primary ${c.primary} text=${pfg} ${Math.max(contrast(c.primary, c.bg), contrast(c.primary, c.fg)).toFixed(1)} | min status ${Math.min(...['red', 'green', 'yellow', 'blue', 'purple', 'aqua', 'orange'].map((k) => contrast(c[`bright_${k}`], c.bg))).toFixed(1)}`);
  }
}
console.log(report.join('\n'));
