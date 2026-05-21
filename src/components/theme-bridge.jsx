import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '@/state/settings';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

const FIELD_TO_VARS = {
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

function applyTheme(theme) {
  if (!theme || typeof theme !== 'object') return;
  const root = document.documentElement;

  const kind = (theme.kind ?? '').toLowerCase();
  const isDark = kind ? kind === 'dark' : true;
  root.classList.toggle('dark', isDark);
  root.setAttribute('data-theme', theme.name ?? 'default');

  for (const [field, vars] of Object.entries(FIELD_TO_VARS)) {
    const value = theme[field];
    if (typeof value !== 'string' || !value) continue;
    for (const cssVar of vars) {
      root.style.setProperty(cssVar, value);
    }
    root.style.setProperty(`--theme-${field.replace(/_/g, '-')}`, value);
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
