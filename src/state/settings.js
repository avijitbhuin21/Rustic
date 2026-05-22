import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { loadFontFromUrl, loadFontFromBytes } from '@/lib/font-loader';

const FONTS_KEY = 'rustic_custom_fonts';

function getSavedFonts() {
  try { return JSON.parse(localStorage.getItem(FONTS_KEY) || '[]'); }
  catch { return []; }
}

function persistFonts(fonts) {
  localStorage.setItem(FONTS_KEY, JSON.stringify(fonts));
}

export const useSettings = create((set, get) => ({
  settings: null,
  themes: [],
  activeTheme: null,
  loading: false,
  saving: false,
  error: null,
  loadedFonts: getSavedFonts(),

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

  // --- Theme actions ---

  importTheme: async (path) => {
    const theme = await invoke('import_theme', { path });
    const themes = await invoke('list_themes');
    set({ themes });
    return theme;
  },

  importThemeJson: async (json) => {
    const theme = await invoke('import_theme_json', { json });
    const themes = await invoke('list_themes');
    // If the just-saved theme is the currently active one (editing in place),
    // refresh activeTheme so ThemeBridge re-applies the new colors.
    const cur = get().activeTheme;
    const next = cur?.name === theme.name
      ? { activeTheme: await invoke('get_active_theme') }
      : {};
    set({ themes, ...next });
    return theme;
  },

  getTheme: (name) => invoke('get_theme', { name }),

  deleteTheme: async (name) => {
    await invoke('delete_theme', { name });
    const themes = await invoke('list_themes');
    const cur = get().settings;
    // If deleted theme was active, reload active theme
    const activeTheme = await invoke('get_active_theme');
    const next = cur
      ? { ...cur, theme: { ...cur.theme, active_theme: activeTheme.name } }
      : cur;
    set({ themes, activeTheme, ...(next ? { settings: next } : {}) });
  },

  setActiveTheme: async (name) => {
    const cur = get().settings;
    if (!cur) return;
    const next = { ...cur, theme: { ...cur.theme, active_theme: name } };
    set({ settings: next });
    await invoke('update_settings', { settings: next });
    const activeTheme = await invoke('get_active_theme');
    set({ activeTheme });
  },

  // --- Font actions ---

  addFontFromUrl: async (url) => {
    const loaded = await loadFontFromUrl(url);
    if (loaded.length === 0) throw new Error('No fonts found at that URL.');
    const prev = get().loadedFonts;
    const added = loaded.filter((f) => !prev.some((p) => p.name === f.name));
    const next = [...prev, ...added.map((f) => ({ ...f, type: 'url' }))];
    persistFonts(next);
    set({ loadedFonts: next });
    return added;
  },

  addFontFromFile: async (path, bytes) => {
    const name = path.split(/[/\\]/).pop().replace(/\.[^.]+$/, '').replace(/[-_]/g, ' ');
    const result = await loadFontFromBytes(name, bytes);
    if (!result) throw new Error('Failed to load font from file.');
    const prev = get().loadedFonts;
    if (prev.some((f) => f.name === name)) return name;
    const next = [...prev, { name, path, type: 'file' }];
    persistFonts(next);
    set({ loadedFonts: next });
    return name;
  },

  removeFont: (name) => {
    const next = get().loadedFonts.filter((f) => f.name !== name);
    persistFonts(next);
    set({ loadedFonts: next });
  },

  // Re-inject all saved fonts into the document (call on app start)
  rehydrateFonts: async () => {
    const fonts = getSavedFonts();
    for (const f of fonts) {
      try {
        if (f.type === 'url') await loadFontFromUrl(f.url);
        else if (f.type === 'file') {
          const { readFile } = await import('@tauri-apps/plugin-fs');
          const bytes = await readFile(f.path);
          await loadFontFromBytes(f.name, bytes);
        }
      } catch { /* ignore stale entries */ }
    }
  },

  detectVscodeKeybindings: async () => invoke('detect_vscode_keybindings'),
}));
