// Terminal colours for the active theme. Used by the real terminals
// (components/terminal/terminal-instance.js) and by the theme preview's mock
// terminal, so both show the same thing.

// VS Code's terminal palette. Without these xterm.js falls back to its own
// defaults, which are saturated near-pure ANSI colors (#ff0000 red, etc.).
const TERMINAL_PALETTE = {
  black:         '#000000',
  red:           '#cd3131',
  green:         '#0dbc79',
  yellow:        '#e5e510',
  blue:          '#2472c8',
  magenta:       '#bc3fbc',
  cyan:          '#11a8cd',
  white:         '#e5e5e5',
  brightBlack:   '#666666',
  brightRed:     '#f14c4c',
  brightGreen:   '#23d18b',
  brightYellow:  '#f5f543',
  brightBlue:    '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan:    '#29b8db',
  brightWhite:   '#e5e5e5',
};

const TERMINAL_BASE = {
  background:          '#0a0a0a',
  foreground:          '#e5e5e5',
  cursor:              '#e5e5e5',
  selectionBackground: '#264f78',
};

// Active theme's terminal colors (theme-bridge `--theme-*` vars; xterm only
// parses hex/rgb, so anything else falls back to the defaults above).
const TERMINAL_VARS = {
  background: '--theme-bg-hard',
  foreground: '--theme-fg',
  cursor: '--theme-fg',
  selectionBackground: '--theme-bg3',
  black: '--theme-bg2',
  red: '--theme-bright-red',
  green: '--theme-bright-green',
  yellow: '--theme-bright-yellow',
  blue: '--theme-bright-blue',
  magenta: '--theme-bright-purple',
  cyan: '--theme-bright-aqua',
  white: '--theme-fg2',
  brightBlack: '--theme-fg4',
  brightRed: '--theme-bright-red',
  brightGreen: '--theme-bright-green',
  brightYellow: '--theme-bright-yellow',
  brightBlue: '--theme-bright-blue',
  brightMagenta: '--theme-bright-purple',
  brightCyan: '--theme-bright-aqua',
  brightWhite: '--theme-fg',
};

export function terminalTheme() {
  const out = { ...TERMINAL_BASE, ...TERMINAL_PALETTE };
  if (typeof document === 'undefined') return out;
  const cs = getComputedStyle(document.documentElement);
  for (const [key, cssVar] of Object.entries(TERMINAL_VARS)) {
    const v = cs.getPropertyValue(cssVar).trim();
    if (/^(#[0-9a-f]{3,8}|rgba?\()/i.test(v)) out[key] = v;
  }
  return out;
}
