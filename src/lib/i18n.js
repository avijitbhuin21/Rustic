// Minimal i18n scaffold.
//
// Usage:
//   import { t, setLocale, registerStrings } from '../lib/i18n.js';
//   t('settings.title')                         // → "Settings"
//   t('agent.tokens', { count: 42 })            // → "42 tokens"
//
// Locale is resolved once at startup from `localStorage.getItem('rustic:locale')`,
// falling back to `navigator.language`, falling back to 'en'. Strings live in a
// nested object keyed by locale; missing keys fall back to the English value, then
// to the key itself so the UI never renders an empty span.
//
// This file deliberately ships only English strings — adding another locale is a
// matter of `registerStrings('fr', { ... })` from the language pack module. Real
// migration of every hardcoded string is a separate, larger refactor; this
// scaffold just makes the next contributor's job a one-line change.

const tables = {
  en: {
    'app.name': 'Rustic',
    'common.cancel': 'Cancel',
    'common.save': 'Save',
    'common.delete': 'Delete',
    'common.confirm': 'Confirm',
    'common.retry': 'Retry',
    'common.dismiss': 'Dismiss',
    'common.loading': 'Loading…',
    'common.error': 'Error',

    'editor.openFile': 'Open File',
    'editor.commandPalette': 'Command Palette',
    'editor.quickOpen': 'Quick Open',
    'editor.welcome.explore': 'Explore',

    'panel.agent': 'Agent',
    'panel.explorer': 'Explorer',
    'panel.search': 'Search',
    'panel.git': 'Source Control',
    'panel.settings': 'Settings',

    'dialog.unsaved.title': 'Unsaved Changes',
    'dialog.unsaved.dontSave': "Don't Save",

    'toast.saveFailed': 'Save failed',
    'toast.fileChanged': 'File changed on disk',
    'toast.unexpectedError': 'Unexpected error',
  },
};

let currentLocale = 'en';

function detectLocale() {
  try {
    const stored = localStorage.getItem('rustic:locale');
    if (stored && tables[stored]) return stored;
  } catch {}
  if (typeof navigator !== 'undefined' && navigator.language) {
    const short = navigator.language.split('-')[0];
    if (tables[short]) return short;
  }
  return 'en';
}

currentLocale = detectLocale();

/**
 * Register a string table for a locale. Merges with any existing table for
 * the same locale (later registrations override earlier ones).
 */
export function registerStrings(locale, strings) {
  if (!tables[locale]) tables[locale] = {};
  Object.assign(tables[locale], strings);
}

export function setLocale(locale) {
  if (!tables[locale]) {
    console.warn(`[i18n] no string table for locale "${locale}", staying on "${currentLocale}"`);
    return;
  }
  currentLocale = locale;
  try { localStorage.setItem('rustic:locale', locale); } catch {}
  // Notify listeners (UI may want to re-render).
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent('rustic:locale-change', { detail: { locale } }));
  }
}

export function getLocale() { return currentLocale; }

/**
 * Resolve a translation key. Returns the English fallback (or the key itself
 * if even English is missing) so the UI is never empty. `vars` are interpolated
 * using `{name}` placeholders.
 */
export function t(key, vars) {
  const localized = (tables[currentLocale] && tables[currentLocale][key]) || tables.en[key] || key;
  if (!vars) return localized;
  return localized.replace(/\{(\w+)\}/g, (_, name) =>
    vars[name] != null ? String(vars[name]) : ''
  );
}
