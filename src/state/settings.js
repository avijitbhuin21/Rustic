import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { editorStore, SETTINGS_BUFFER_ID, setActiveBuffer } from './editor.js';

export const settingsStore = createStore({
  settings: null,     // UserSettings object
  themes: [],         // ThemeInfo[]
  activeCategory: 'general',
  isOpen: false,
  // Color palette management
  savedPalettes: [],       // { name, data }[] — active state is derived from settings.theme.active_theme
  previousPalette: null,   // for revert
  // Font management
  fontConfig: null,        // per-element font config
  fontLibrary: [],         // { name, source, url? }[] - loaded fonts
  fontConfigLibrary: [],   // { name, config }[] - saved font configs
});

export async function loadSettings() {
  try {
    const saved = await api.getSettings();
    const themes = await api.listThemes();
    // Deep-merge saved settings with defaults so new settings always have values
    const settings = saved ? deepMerge(getDefaults(), saved) : getDefaults();
    settingsStore.setState({ settings, themes: themes || [] });
  } catch (e) {
    console.error('Failed to load settings:', e);
    settingsStore.setState({ settings: getDefaults(), themes: [] });
  }

  // Load saved palettes from localStorage
  try {
    const palettes = JSON.parse(localStorage.getItem('rustic_palettes') || '[]');
    const fontConfig = JSON.parse(localStorage.getItem('rustic_font_config') || 'null');
    const fontLibrary = JSON.parse(localStorage.getItem('rustic_font_library') || '[]');
    const fontConfigLibrary = JSON.parse(localStorage.getItem('rustic_font_config_library') || '[]');
    settingsStore.setState({ savedPalettes: palettes, fontConfig, fontLibrary, fontConfigLibrary });
  } catch {}
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

  // Add settings as a tab in the editor
  const buffers = editorStore.getState('openBuffers');
  if (!buffers[SETTINGS_BUFFER_ID]) {
    const newBuffers = {
      ...buffers,
      [SETTINGS_BUFFER_ID]: {
        id: SETTINGS_BUFFER_ID,
        filePath: '',
        fileName: 'Settings',
        projectName: '',
        lineCount: 0,
        language: null,
        isModified: false,
        fileType: 'settings',
        isPreview: true,
        isDualMode: false,
        viewMode: 'preview',
      },
    };
    editorStore.setState({ openBuffers: newBuffers });
  }
  setActiveBuffer(SETTINGS_BUFFER_ID);
}

export function closeSettings() {
  settingsStore.setState({ isOpen: false });

  const buffers = { ...editorStore.getState('openBuffers') };
  const wasActive = editorStore.getState('activeBufferId') === SETTINGS_BUFFER_ID;
  delete buffers[SETTINGS_BUFFER_ID];

  // Strip the settings id from every group's bufferIds. Without this the
  // editor column stays "populated" from the layout's point of view even
  // though the tab is gone — see no-open-files check in main.js.
  const groups = editorStore.getState('groups').map(g => {
    if (!g.bufferIds.includes(SETTINGS_BUFFER_ID)) return g;
    const bufferIds = g.bufferIds.filter(id => id !== SETTINGS_BUFFER_ID);
    const activeBufferId = g.activeBufferId === SETTINGS_BUFFER_ID
      ? (bufferIds.length > 0 ? bufferIds[bufferIds.length - 1] : null)
      : g.activeBufferId;
    return { ...g, bufferIds, activeBufferId };
  });

  if (wasActive) {
    const ids = Object.keys(buffers).map(Number);
    const newActiveId = ids.length > 0 ? ids[ids.length - 1] : null;
    editorStore.setState({ openBuffers: buffers, groups, activeBufferId: newActiveId });
  } else {
    editorStore.setState({ openBuffers: buffers, groups });
  }
}

export function setCategory(cat) {
  settingsStore.setState({ activeCategory: cat });
}

export function savePalettes(palettes) {
  localStorage.setItem('rustic_palettes', JSON.stringify(palettes));
  settingsStore.setState({ savedPalettes: palettes });
}

export function saveFontConfig(config) {
  localStorage.setItem('rustic_font_config', JSON.stringify(config));
  settingsStore.setState({ fontConfig: config });
}

export function saveFontLibrary(fonts) {
  localStorage.setItem('rustic_font_library', JSON.stringify(fonts));
  settingsStore.setState({ fontLibrary: fonts });
}

export function saveFontConfigLibrary(configs) {
  localStorage.setItem('rustic_font_config_library', JSON.stringify(configs));
  settingsStore.setState({ fontConfigLibrary: configs });
}

/** Deep-merge defaults with saved settings. Saved values win, but missing keys get defaults. */
function deepMerge(defaults, saved) {
  const result = { ...defaults };
  for (const key of Object.keys(saved)) {
    if (saved[key] && typeof saved[key] === 'object' && !Array.isArray(saved[key])
        && defaults[key] && typeof defaults[key] === 'object' && !Array.isArray(defaults[key])) {
      result[key] = deepMerge(defaults[key], saved[key]);
    } else {
      result[key] = saved[key];
    }
  }
  return result;
}

function getDefaults() {
  return {
    general: {
      auto_save: false,
      auto_save_delay_ms: 1000,
      ui_scale: 1.0,
    },
    editor: {
      tab_size: 4,
      insert_spaces: true,
      word_wrap: false,
      line_numbers: true,
      minimap: false,
      cursor_blink: true,
      cursor_style: 'line',
      cursor_custom_svg: '',
      render_whitespace: 'none',
      show_zero_width: false,
      bracket_pair_colorization: false,
      format_on_save: true,
    },
    theme: {
      active_theme: 'Luxide Dark',
      custom_themes: [],
    },
    appearance: {
      font_family: 'JetBrains Mono, Fira Code, Consolas, monospace',
      font_size: 14,
      font_url: '',
    },
    keybindings: [],
    ai: {
      default_provider: null,
      max_tokens: 16384,
      temperature: 0.7,
    },
  };
}
