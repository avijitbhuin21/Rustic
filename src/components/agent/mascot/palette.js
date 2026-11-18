// Resolves theme tokens into sRGB triplets (0..1) for the mascot's shaders.
// Tokens are oklch / color-mix expressions, so they are painted into a 1×1
// canvas and read back rather than parsed.

const TOKENS = {
  accent: 'var(--info)',
  accentHi: 'color-mix(in oklab, var(--info) 55%, white)',
  lid: 'color-mix(in oklab, var(--muted-foreground) 45%, var(--background))',
  lidEdge: 'var(--muted-foreground)',
  blush: 'color-mix(in oklab, var(--danger) 60%, white)',
  warn: 'var(--warning)',
  ok: 'var(--success)',
  bad: 'var(--danger)',
  drop: 'var(--info)',
};

const MODE_TOKENS = {
  dark: {
    furTip: 'color-mix(in oklab, var(--foreground) 92%, white)',
    furRoot: 'color-mix(in oklab, var(--foreground) 80%, var(--background))',
    rim: 'color-mix(in oklab, var(--info) 35%, white)',
    eye: 'color-mix(in oklab, var(--background) 85%, black)',
    pupil: 'color-mix(in oklab, var(--background) 55%, black)',
    glint: 'white',
    bubble: 'var(--foreground)',
    ink: 'var(--background)',
    shadow: 'black',
  },
  light: {
    furTip: 'color-mix(in oklab, var(--foreground) 72%, var(--background))',
    furRoot: 'var(--foreground)',
    rim: 'color-mix(in oklab, var(--info) 45%, var(--background))',
    eye: 'var(--background)',
    pupil: 'color-mix(in oklab, var(--foreground) 92%, black)',
    glint: 'white',
    bubble: 'var(--background)',
    ink: 'var(--foreground)',
    shadow: 'black',
  },
};

const FALLBACK = {
  dark: {
    furTip: '#eef0f5', furRoot: '#9aa0b2', rim: '#b9d6ff', eye: '#111318', pupil: '#08090c', glint: '#ffffff',
    accent: '#4f9dff', accentHi: '#a8d0ff', lid: '#3a4152', lidEdge: '#5a627a', blush: '#ff8fa3',
    warn: '#f0c45a', ok: '#3ecf8e', bad: '#ff5f6d', drop: '#7cc4ff', bubble: '#e9ebf1', ink: '#15171d', shadow: '#000000',
  },
  light: {
    furTip: '#3b404e', furRoot: '#111318', rim: '#9cc3fb', eye: '#f4f5f8', pupil: '#0d0e12', glint: '#ffffff',
    accent: '#2f7ff0', accentHi: '#79b0ff', lid: '#c9ccd6', lidEdge: '#9aa0b1', blush: '#ff7d93',
    warn: '#e0a92a', ok: '#21b072', bad: '#e5484d', drop: '#3d97f0', bubble: '#ffffff', ink: '#17191f', shadow: '#000000',
  },
};

const SHADOW_OPACITY = { dark: 0.42, light: 0.2 };
const SENTINEL = '#010203';
let probeCtx = null;

/** Converts a #rrggbb string to an [r, g, b] triplet in 0..1. */
function hexToRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

/** Paints a CSS colour into a 1×1 canvas and reads it back; null when the canvas can't parse it. */
function cssToRgb(css) {
  if (!probeCtx) {
    const c = document.createElement('canvas');
    c.width = 1;
    c.height = 1;
    probeCtx = c.getContext('2d', { willReadFrequently: true });
  }
  if (!probeCtx || !css) return null;
  probeCtx.fillStyle = SENTINEL;
  probeCtx.fillStyle = css;
  if (probeCtx.fillStyle === SENTINEL) return null;
  probeCtx.clearRect(0, 0, 1, 1);
  probeCtx.fillRect(0, 0, 1, 1);
  const d = probeCtx.getImageData(0, 0, 1, 1).data;
  if (d[3] === 0) return null;
  return [d[0] / 255, d[1] / 255, d[2] / 255];
}

/** Resolves the mascot palette from the theme tokens in scope at `host`. */
export function resolvePalette(host) {
  const dark = !!(host.closest?.('.dark') || document.documentElement.classList.contains('dark'));
  const mode = dark ? 'dark' : 'light';
  const probe = document.createElement('span');
  probe.style.display = 'none';
  host.appendChild(probe);
  const colors = {};
  for (const [key, expr] of Object.entries({ ...TOKENS, ...MODE_TOKENS[mode] })) {
    probe.style.color = '';
    probe.style.color = expr;
    colors[key] = cssToRgb(getComputedStyle(probe).color) || hexToRgb(FALLBACK[mode][key]);
  }
  probe.remove();
  return { mode, colors, shadowOpacity: SHADOW_OPACITY[mode] };
}
