import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export const useSettings = create((set, get) => ({
  settings: null,
  themes: [],
  activeTheme: null,
  loading: false,
  saving: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null });
    try {
      const [settings, themes, activeTheme] = await Promise.all([
        invoke('get_settings'),
        invoke('list_themes'),
        invoke('get_active_theme'),
      ]);
      set({ settings, themes, activeTheme, loading: false });
    } catch (err) {
      set({ error: String(err), loading: false });
    }
  },

  update: async (partial) => {
    const cur = get().settings;
    if (!cur) return;
    const next = { ...cur, ...partial };
    set({ saving: true, settings: next });
    try {
      await invoke('update_settings', { settings: next });
    } finally {
      set({ saving: false });
    }
  },

  importTheme: async (path) => {
    await invoke('import_theme', { path });
    const themes = await invoke('list_themes');
    set({ themes });
  },

  detectVscodeKeybindings: async () => invoke('detect_vscode_keybindings'),
}));
