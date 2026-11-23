import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { useSettings } from '@/state/settings';
import { useThemes } from '@/state/themes';
import { applyActiveTheme, applyTheme } from '@/lib/theme-apply';

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
    applyActiveTheme(active, systemDark);
  }, [active, systemDark]);

  useEffect(() => {
    if (legacy && activeTheme) applyTheme(activeTheme);
  }, [legacy, activeTheme]);

  return null;
}
