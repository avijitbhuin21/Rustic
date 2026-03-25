import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const settingsStore = createStore({
  settings: null,     // UserSettings object
  themes: [],         // ThemeInfo[]
  activeCategory: 'general',
  isOpen: false,
});

export async function loadSettings() {
  try {
    const settings = await api.getSettings();
    const themes = await api.listThemes();
    settingsStore.setState({ settings: settings || getDefaults(), themes: themes || [] });
  } catch (e) {
    console.error('Failed to load settings:', e);
    settingsStore.setState({ settings: getDefaults(), themes: [] });
  }
}

export async function saveSettings(settings) {
  try {
    await api.updateSettings(settings);
    settingsStore.setState({ settings });
  } catch (e) {
    console.error('Failed to save settings:', e);
  }
}

export async function updateSetting(path, value) {
  const settings = { ...settingsStore.getState('settings') };
  const parts = path.split('.');
  let obj = settings;
  for (let i = 0; i < parts.length - 1; i++) {
    obj[parts[i]] = { ...obj[parts[i]] };
    obj = obj[parts[i]];
  }
  obj[parts[parts.length - 1]] = value;
  await saveSettings(settings);
}

export function openSettings() {
  settingsStore.setState({ isOpen: true });
  loadSettings();
}

export function closeSettings() {
  settingsStore.setState({ isOpen: false });
}

export function setCategory(cat) {
  settingsStore.setState({ activeCategory: cat });
}

function getDefaults() {
  return {
    general: {
      font_family: 'JetBrains Mono, Fira Code, Consolas, monospace',
      font_size: 14,
      ui_scale: 1.0,
      auto_save: false,
      auto_save_delay_ms: 1000,
    },
    editor: {
      tab_size: 4,
      insert_spaces: true,
      word_wrap: false,
      line_numbers: true,
      minimap: false,
      cursor_blink: true,
      cursor_style: 'line',
      render_whitespace: 'none',
    },
    theme: {
      active_theme: 'Gruvbox Dark',
      custom_themes: [],
    },
    keybindings: [],
    ai: {
      default_provider: null,
      max_tokens: 4096,
      temperature: 0.7,
    },
  };
}
