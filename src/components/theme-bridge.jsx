import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '@/state/settings';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

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
  const dest   = theme.bright_red || '#f87171';

  return {
    '--background': bg,
    '--foreground': fg,
    '--card': bg1,
    '--card-foreground': fg,
    '--popover': bg1,
    '--popover-foreground': fg,
    '--primary': fg2,
    '--primary-foreground': bg,
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
    '--sidebar-primary-foreground': bg,
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

export function ThemeBridge() {
  const activeTheme = useSettings((s) => s.activeTheme);
  const settings = useSettings((s) => s.settings);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke('get_active_theme')
      .then((t) => { if (!cancelled) applyTheme(t); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [settings?.appearance?.theme]);

  useEffect(() => {
    if (activeTheme) applyTheme(activeTheme);
  }, [activeTheme]);

  return null;
}
