// Theme packs (format 2): installed list, active theme, color mode and the
// actions behind the Appearance → Themes manager. Imported / edited themes
// only apply after the user trusts that exact file (see rustic_app::themes).
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

/** Ask the bridge to repaint (other windows / tabs refresh on focus). */
function notifyChanged() {
  window.dispatchEvent(new CustomEvent('rustic:theme-changed'));
}

export const useThemes = create((set, get) => ({
  list: [],
  active: null,
  loaded: false,

  refresh: async () => {
    const [list, active] = await Promise.all([invoke('theme_list'), invoke('theme_active')]);
    set({ list: Array.isArray(list) ? list : [], active, loaded: true });
    return { list, active };
  },

  /** Full detail (file text, hash, scan report) for the trust prompt. */
  detail: (id) => invoke('theme_get', { id }),

  importFile: async (path) => {
    const d = await invoke('theme_import_file', { path });
    await get().refresh();
    return d;
  },

  importText: async (text) => {
    const d = await invoke('theme_import_text', { text });
    await get().refresh();
    return d;
  },

  /** Trust the reviewed file (`hash`) and make it active. */
  trustAndApply: async (id, hash) => {
    await invoke('theme_trust', { id, hash });
    await invoke('theme_apply', { id });
    await get().refresh();
    notifyChanged();
  },

  apply: async (id) => {
    await invoke('theme_apply', { id });
    await get().refresh();
    notifyChanged();
  },

  setMode: async (mode) => {
    await invoke('theme_set_mode', { mode });
    await get().refresh();
    notifyChanged();
  },

  remove: async (id) => {
    await invoke('theme_delete', { id });
    await get().refresh();
    notifyChanged();
  },
}));
