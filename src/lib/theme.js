/**
 * Apply a theme by setting CSS custom properties on the document root.
 * Also provides xterm.js theme object.
 */

const CSS_VAR_MAP = {
  bg_hard: '--bg-hard',
  bg: '--bg',
  bg_soft: '--bg-soft',
  bg1: '--bg1',
  bg2: '--bg2',
  bg3: '--bg3',
  bg4: '--bg4',
  fg: '--fg',
  fg1: '--fg1',
  fg2: '--fg2',
  fg3: '--fg3',
  fg4: '--fg4',
  accent: '--accent',
  border: '--border',
  bright_red: '--bright-red',
  bright_green: '--bright-green',
  bright_yellow: '--bright-yellow',
  bright_blue: '--bright-blue',
  bright_purple: '--bright-purple',
  bright_aqua: '--bright-aqua',
  bright_orange: '--bright-orange',
  token_keyword: '--token-keyword',
  token_string: '--token-string',
  token_comment: '--token-comment',
  token_function: '--token-function',
  token_type: '--token-type',
  token_variable: '--token-variable',
  token_number: '--token-number',
  token_operator: '--token-operator',
  token_punctuation: '--token-punctuation',
};

let currentTheme = null;

export function applyTheme(theme) {
  currentTheme = theme;
  const root = document.documentElement;

  // Set data-theme attribute for light/dark mode
  root.setAttribute('data-theme', theme.kind || 'dark');

  // Apply all CSS custom properties
  for (const [key, cssVar] of Object.entries(CSS_VAR_MAP)) {
    if (theme[key]) {
      root.style.setProperty(cssVar, theme[key]);
    }
  }

  // Dispatch event for xterm.js and other consumers
  window.dispatchEvent(new CustomEvent('rustic:theme-changed', { detail: theme }));
}

export function getXtermTheme() {
  if (!currentTheme) return null;
  return {
    background: currentTheme.bg,
    foreground: currentTheme.fg,
    cursor: currentTheme.accent,
    cursorAccent: currentTheme.bg,
    selectionBackground: currentTheme.bg3,
    selectionForeground: currentTheme.fg,
    black: currentTheme.bg_hard,
    red: currentTheme.bright_red,
    green: currentTheme.bright_green,
    yellow: currentTheme.bright_yellow,
    blue: currentTheme.bright_blue,
    magenta: currentTheme.bright_purple,
    cyan: currentTheme.bright_aqua,
    white: currentTheme.fg,
    brightBlack: currentTheme.bg4,
    brightRed: currentTheme.bright_red,
    brightGreen: currentTheme.bright_green,
    brightYellow: currentTheme.bright_yellow,
    brightBlue: currentTheme.bright_blue,
    brightMagenta: currentTheme.bright_purple,
    brightCyan: currentTheme.bright_aqua,
    brightWhite: currentTheme.fg1,
  };
}

export function getCurrentTheme() {
  return currentTheme;
}
