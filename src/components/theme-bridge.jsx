import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { useSettings } from '@/state/settings';
import { useThemes } from '@/state/themes';


// Legacy vars defined in globals.css :root — used by hand-rolled chrome
// (body bg, scrollbars, status colors). Kept in sync with the theme.
const LEGACY_FIELD_TO_VARS = {
  bg_hard: ['--bg-primary'],
  bg: ['--bg-primary'],
  bg_soft: ['--bg-secondary'],
  bg1: ['--bg-tertiary'],
  bg2: ['--bg-elevated'],
  bg3: ['--bg-elevated'],
  fg: ['--text-primary'],
  fg1: ['--text-primary'],
  fg2: ['--text-secondary'],
  fg3: ['--text-muted'],
  fg4: ['--text-disabled'],
  accent: ['--accent-primary'],
  border: ['--border-default'],
  bright_red: ['--status-error'],
  bright_green: ['--status-success'],
  bright_yellow: ['--status-warning'],
  bright_blue: ['--status-info'],
  bright_aqua: ['--syntax-type'],
  bright_purple: ['--syntax-keyword'],
  bright_orange: ['--syntax-string'],
};

/** Relative luminance of a `#rrggbb` color (null when not hex). */
function luminance(hex) {
  const m = /^#?([0-9a-f]{6})/i.exec(hex || '');
  if (!m) return null;
  const v = [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
}

/** Whichever of `a` / `b` reads better as text on `fill`. */
function readableOn(fill, a, b) {
  const f = luminance(fill), la = luminance(a), lb = luminance(b);
  if (f == null || la == null || lb == null) return a;
  const ratio = (x) => (Math.max(f, x) + 0.05) / (Math.min(f, x) + 0.05);
  return ratio(la) >= ratio(lb) ? a : b;
}

// shadcn tokens live in the `.dark` class (oklch). Inline styles on <html>
// win over class rules, so setting them here repaints every shadcn surface
// (buttons, dialogs, cards, sidebar, inputs, …). Without this block, switching
// themes only updates a handful of hand-rolled chrome variables and the bulk
// of the UI stays on the original dark palette.
function deriveShadcnTokens(theme) {
  const bg     = theme.bg     || '#1a1a1a';
  const bgSoft = theme.bg_soft|| theme.bg1 || bg;
  const bg1    = theme.bg1    || bgSoft;
  const bg2    = theme.bg2    || bg1;
  const fg     = theme.fg     || '#fafafa';
  const fg2    = theme.fg2    || fg;
  const fg3    = theme.fg3    || fg2;
  const border = theme.border || bg2;
  const accent = theme.accent || fg2;
  const primary = theme.primary || fg2;
  const dest   = theme.bright_red || '#f87171';
  const isLight = (theme.kind ?? '').toLowerCase() === 'light';

  return {
    '--ink': isLight ? fg : '#ffffff',
    '--success': theme.bright_green || (isLight ? '#1f7a45' : '#86efac'),
    '--warning': theme.bright_yellow || (isLight ? '#9a6b00' : '#fcd34d'),
    '--danger': dest,
    '--info': theme.bright_blue || (isLight ? '#2f5fd0' : '#7dd3fc'),
    '--special': theme.bright_purple || (isLight ? '#6b4fc4' : '#c4b5fd'),
    '--highlight': theme.bright_orange || (isLight ? '#b45a1c' : '#fdba74'),
    '--on-status': isLight ? '#ffffff' : bg,
    '--background': bg,
    '--foreground': fg,
    '--card': bg1,
    '--card-foreground': fg,
    '--popover': bg1,
    '--popover-foreground': fg,
    '--primary': primary,
    '--primary-foreground': readableOn(primary, bg, fg),
    '--secondary': bg2,
    '--secondary-foreground': fg,
    '--muted': bg2,
    '--muted-foreground': fg3,
    '--accent': bg2,
    '--accent-foreground': fg,
    '--destructive': dest,
    '--border': border,
    '--input': border,
    '--ring': accent,
    '--sidebar': bg1,
    '--sidebar-foreground': fg,
    '--sidebar-primary': accent,
    '--sidebar-primary-foreground': readableOn(accent, bg, fg),
    '--sidebar-accent': bg2,
    '--sidebar-accent-foreground': fg,
    '--sidebar-border': border,
    '--sidebar-ring': accent,
  };
}

function applyTheme(theme) {
  if (!theme || typeof theme !== 'object') return;
  const root = document.documentElement;

  const kind = (theme.kind ?? '').toLowerCase();
  const isDark = kind ? kind === 'dark' : true;
  root.classList.toggle('dark', isDark);
  root.setAttribute('data-theme', theme.name ?? 'default');

  for (const [field, vars] of Object.entries(LEGACY_FIELD_TO_VARS)) {
    const value = theme[field];
    if (typeof value !== 'string' || !value) continue;
    for (const cssVar of vars) {
      root.style.setProperty(cssVar, value);
    }
    root.style.setProperty(`--theme-${field.replace(/_/g, '-')}`, value);
  }

  const shadcn = deriveShadcnTokens(theme);
  for (const [cssVar, value] of Object.entries(shadcn)) {
    root.style.setProperty(cssVar, value);
  }
}

/** Write `text` into `<style id=…>` at the end of <head> (created on demand). */
function setStyleTag(id, text) {
  let el = document.getElementById(id);
  if (!text) {
    el?.remove();
    return;
  }
  if (!el) {
    el = document.createElement('style');
    el.id = id;
  }
  el.textContent = text;
  document.head.appendChild(el);
}

/**
 * Style settings → CSS. Values are validated server-side (safe characters
 * only), so they can be interpolated directly.
 */
function styleCss(style) {
  if (!style) return '';
  const rules = [];
  if (style.radius) rules.push(`:root{--radius:${style.radius};}`);
  if (style.font_sans) rules.push(`body,.font-sans{font-family:${style.font_sans} !important;}`);
  if (style.font_mono) rules.push(`.font-mono,code,kbd,pre,samp{font-family:${style.font_mono} !important;}`);
  if (style.motion === 'reduced') {
    rules.push('*,*::before,*::after{animation-duration:.01ms !important;animation-iteration-count:1 !important;transition-duration:.01ms !important;scroll-behavior:auto !important;}');
  }
  return rules.join('\n');
}

/** Recipe keys mirrored onto <html> as data-rt-* (see styles/recipes.css). */
const RECIPE_KEYS = ['shadow', 'surface', 'density', 'button', 'texture'];

/** Set or clear the data-rt-* recipe attributes and --rt-border-w for `style`. */
function applyRecipes(style) {
  const root = document.documentElement;
  for (const key of RECIPE_KEYS) {
    const v = style?.[key];
    if (v) root.setAttribute(`data-rt-${key}`, v);
    else root.removeAttribute(`data-rt-${key}`);
  }
  if (style?.border_width) {
    root.setAttribute('data-rt-border', '');
    root.style.setProperty('--rt-border-w', style.border_width);
  } else {
    root.removeAttribute('data-rt-border');
    root.style.removeProperty('--rt-border-w');
  }
}

/** Pick the variant for `mode`, falling back to whichever the theme has. */
function pickVariant(active, mode) {
  if (mode === 'light') return active.light || active.dark;
  return active.dark || active.light;
}

export function ThemeBridge() {
  const activeTheme = useSettings((s) => s.activeTheme);
  const settings = useSettings((s) => s.settings);
  const active = useThemes((s) => s.active);
  const refresh = useThemes((s) => s.refresh);
  const [systemDark, setSystemDark] = useState(
    () => typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches,
  );
  const [legacy, setLegacy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    refresh().catch(() => {
      // Older backend without theme packs: keep the palette-only path.
      if (cancelled) return;
      setLegacy(true);
      if (isTauri()) invoke('get_active_theme').then((t) => !cancelled && applyTheme(t)).catch(() => {});
    });
    return () => { cancelled = true; };
  }, [settings?.theme?.active_theme, refresh]);

  useEffect(() => {
    const onChange = () => { refresh().catch(() => {}); };
    window.addEventListener('rustic:theme-changed', onChange);
    const mq = window.matchMedia?.('(prefers-color-scheme: dark)');
    const onScheme = (e) => setSystemDark(e.matches);
    mq?.addEventListener?.('change', onScheme);
    return () => {
      window.removeEventListener('rustic:theme-changed', onChange);
      mq?.removeEventListener?.('change', onScheme);
    };
  }, [refresh]);

  useEffect(() => {
    if (!active) return;
    const mode = active.mode === 'system' ? (systemDark ? 'dark' : 'light') : (active.mode || 'dark');
    const variant = pickVariant(active, mode);
    if (variant) applyTheme(variant);
    document.documentElement.setAttribute('data-theme-mode', variant?.kind || mode);
    setStyleTag('rustic-theme-style', styleCss(active.style));
    applyRecipes(active.style);
    setStyleTag('rustic-theme-css', active.css || '');
  }, [active, systemDark]);

  useEffect(() => {
    if (legacy && activeTheme) applyTheme(activeTheme);
  }, [legacy, activeTheme]);

  return null;
}
