// Keeps Monaco's `rustic-dark` theme in sync with the app theme by reading
// the CSS variables the theme bridge sets. The name is kept so existing
// editors / diff editors pick the change up without passing a new theme.

const ctx = typeof document !== 'undefined'
  ? document.createElement('canvas').getContext('2d', { willReadFrequently: true })
  : null;

/** Resolve any CSS color (hex, rgb, oklch…) to `#rrggbb` / `#rrggbbaa`, or null. */
function toHex(value) {
  if (!ctx || !value) return null;
  ctx.clearRect(0, 0, 1, 1);
  ctx.fillStyle = '#000';
  ctx.fillStyle = value;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
  const h = (n) => n.toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}${a < 255 ? h(a) : ''}`;
}

/** Read `--name` from the root as hex, falling back to `fallback`. */
function cssColor(styles, name, fallback) {
  const v = styles.getPropertyValue(name).trim();
  return (v && toHex(v)) || fallback;
}

/** Define (or redefine) `rustic-dark` from the current app theme and apply it. */
export function applyMonacoTheme(monaco) {
  if (!monaco?.editor || typeof document === 'undefined') return;
  const root = document.documentElement;
  const s = getComputedStyle(root);
  const dark = root.classList.contains('dark');
  const bg = cssColor(s, '--background', dark ? '#1e1e1e' : '#ffffff');
  const fg = cssColor(s, '--foreground', dark ? '#d4d4d4' : '#1f1f1f');
  const card = cssColor(s, '--card', bg);
  const border = cssColor(s, '--border', dark ? '#3c3c3c' : '#d4d4d4');
  const muted = cssColor(s, '--muted-foreground', dark ? '#858585' : '#6e6e6e');
  const accent = cssColor(s, '--ring', dark ? '#0d9488' : '#0f766e');
  const keyword = cssColor(s, '--syntax-keyword', null);
  const string = cssColor(s, '--syntax-string', null);
  const type = cssColor(s, '--syntax-type', null);
  const strip = (c) => c?.replace('#', '').slice(0, 6);
  const comment = strip(muted);
  const rules = [
    { token: 'comment', foreground: comment, fontStyle: 'italic' },
    { token: 'comment.line', foreground: comment, fontStyle: 'italic' },
    { token: 'comment.block', foreground: comment, fontStyle: 'italic' },
    { token: 'comment.doc', foreground: comment, fontStyle: 'italic' },
    { token: 'constant.language', fontStyle: 'italic' },
    { token: 'keyword.constant', fontStyle: 'italic' },
    { token: 'variable.language', fontStyle: 'italic' },
  ];
  if (keyword) rules.push({ token: 'keyword', foreground: strip(keyword) });
  if (string) rules.push({ token: 'string', foreground: strip(string) });
  if (type) rules.push({ token: 'type', foreground: strip(type) }, { token: 'type.identifier', foreground: strip(type) });
  const a = strip(accent);
  monaco.editor.defineTheme('rustic-dark', {
    base: dark ? 'vs-dark' : 'vs',
    inherit: true,
    rules,
    colors: {
      'editor.background': bg,
      'editor.foreground': fg,
      'editorGutter.background': bg,
      'editorLineNumber.foreground': muted,
      'editorLineNumber.activeForeground': fg,
      'editorCursor.foreground': fg,
      'editor.lineHighlightBackground': `${strip(card)}80`,
      'editor.lineHighlightBorder': '#00000000',
      'editorWidget.background': card,
      'editorWidget.border': border,
      'editorSuggestWidget.background': card,
      'editorSuggestWidget.border': border,
      'editorHoverWidget.background': card,
      'editorHoverWidget.border': border,
      'editorIndentGuide.background1': `${strip(border)}80`,
      'minimap.background': bg,
      'scrollbar.shadow': '#00000000',
      'editor.findMatchBackground': `#${a}40`,
      'editor.findMatchBorder': `#${a}`,
      'editor.findMatchHighlightBackground': `#${a}20`,
      'editor.findMatchHighlightBorder': `#${a}60`,
    },
  });
  monaco.editor.setTheme('rustic-dark');
}

let watching = false;

/** Re-apply whenever the theme bridge repaints the root (style / class). */
export function watchMonacoTheme(monaco) {
  applyMonacoTheme(monaco);
  if (watching || typeof MutationObserver === 'undefined') return;
  watching = true;
  let frame = 0;
  const schedule = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => applyMonacoTheme(monaco));
  };
  new MutationObserver(schedule).observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'class'] });
  window.addEventListener('rustic:theme-changed', schedule);
}
