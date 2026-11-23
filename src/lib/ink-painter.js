// Hand-drawn outlines for elements that can't host a child <svg>: text fields
// (void elements), scrolling popups and the app's panels. The outline is drawn
// by the ink engine and set as a CSS background, so it never scrolls with the
// content, adds scrollbars or takes clicks. One observer pair drives it all;
// components only carry `data-slot` / `data-ink-frame` attributes.

const FIELDS = '[data-slot="input"], [data-slot="textarea"], [data-slot="select-trigger"]';
const SURFACES = [
  'dialog-content', 'popover-content', 'dropdown-menu-content', 'dropdown-menu-sub-content',
  'context-menu-content', 'context-menu-sub-content', 'select-content', 'sheet-content',
].map((s) => `[data-slot="${s}"]`).join(', ');
const FRAMES = '[data-ink-frame]';
const SELECTOR = `${FIELDS}, ${SURFACES}, ${FRAMES}`;
const WATCHED_ATTRS = ['disabled', 'aria-invalid', 'data-ink-frame'];
const SETTLE_MS = 90;

let kit = null;
let resizeObserver = null;
let mutationObserver = null;
let rev = 0;
let nextSeed = 0;
const tracked = new Set();
const states = new WeakMap();
const colors = new Map();

/** Computed value of a Looseleaf colour token (an SVG image can't read CSS variables). */
function color(key) {
  let v = colors.get(key);
  if (v === undefined) {
    v = getComputedStyle(document.documentElement).getPropertyValue(`--ll-${key}`).trim() || 'currentColor';
    colors.set(key, v);
  }
  return v;
}

/** Recipe, params and inset (px kept clear inside the element's edge) for `el`. */
function specFor(el) {
  if (el.matches(FIELDS)) {
    const focus = el === document.activeElement || el.contains(document.activeElement);
    return {
      recipe: kit.RECIPES.field,
      inset: 2,
      params: { focus, invalid: el.getAttribute('aria-invalid') === 'true', disabled: el.matches(':disabled, [data-disabled]') },
    };
  }
  const radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
  const corner = Math.round(Math.min(10, Math.max(4, radius * 0.7)));
  if (el.matches(SURFACES)) return { recipe: kit.RECIPES.surface, inset: 3, params: { corner } };
  const tone = el.getAttribute('data-ink-frame') || 'panel';
  if (tone.startsWith('rule')) return { recipe: kit.RECIPES.rule, inset: 0, overlay: true, params: { edge: tone.split('-')[1] || 'bottom' } };
  // Panels are filled edge to edge by children with their own backgrounds, so
  // their outline is drawn over the content instead of behind it.
  return { recipe: kit.RECIPES.frame, inset: tone === 'panel' ? 2 : 3, overlay: tone === 'panel', params: { tone, corner } };
}

/** Draw (or redraw) the outline of `el` for its current size and state. */
function paint(el) {
  const st = states.get(el);
  if (!kit || !st || !el.isConnected) return;
  const w = el.offsetWidth;
  const h = el.offsetHeight;
  if (w < 10 || h < 10) return;
  const { recipe, inset, params, overlay = false } = specFor(el);
  const bw = w - 2 * inset;
  const bh = h - 2 * inset;
  const key = `${recipe.id}|${bw}x${bh}|${JSON.stringify(params)}|${rev}`;
  if (st.key === key) return;
  let frame;
  try {
    frame = kit.inkFrame(recipe, {
      box: { w: bw, h: bh }, params, seed: st.seed, tier: 'lo', density: 'comfortable', dir: 'ltr', knobs: kit.DEFAULT_KNOBS,
    });
  } catch (e) {
    console.warn('ink: outline failed', e);
    return;
  }
  const fg = getComputedStyle(el).color;
  const body = kit.frameSvg(frame, undefined, false)
    .replace(/\smask="[^"]*"/g, '')
    .replace(/mix-blend-mode:[^;"]*;?/g, '')
    .replace(/var\(--ll-([a-z-]+)\)/g, (_, k) => color(k))
    .replace(/currentColor/g, fg);
  const pad = frame.pad;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${-pad} ${-pad} ${frame.w + 2 * pad} ${frame.h + 2 * pad}" preserveAspectRatio="none">${body}</svg>`;
  st.key = key;
  st.pad = pad;
  st.inset = inset;
  st.overlay = overlay;
  const image = `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
  const position = `${inset - pad}px ${inset - pad}px`;
  const size = `${bw + 2 * pad}px ${bh + 2 * pad}px`;
  const s = el.style;
  if (overlay) {
    s.setProperty('--ink-img', image);
    s.setProperty('--ink-pos', position);
    s.setProperty('--ink-size', size);
  } else {
    s.backgroundImage = image;
    s.backgroundRepeat = 'no-repeat';
    s.backgroundOrigin = 'border-box';
    s.backgroundPosition = position;
    s.backgroundSize = size;
  }
  el.setAttribute('data-ink-backdrop', overlay ? 'over' : '');
}

/** Stretch the current drawing to the new size now; redraw once resizing settles. */
function onResize(el) {
  const st = states.get(el);
  if (!st) return;
  if (st.key === null) {
    paint(el);
    return;
  }
  const size = `${el.offsetWidth - 2 * st.inset + 2 * st.pad}px ${el.offsetHeight - 2 * st.inset + 2 * st.pad}px`;
  if (st.overlay) el.style.setProperty('--ink-size', size);
  else el.style.backgroundSize = size;
  clearTimeout(st.timer);
  st.timer = setTimeout(() => paint(el), SETTLE_MS);
}

/** Remove the drawing and every inline style the painter set on `el`. */
function clear(el) {
  const st = states.get(el);
  if (st) clearTimeout(st.timer);
  const s = el.style;
  for (const p of ['background-image', 'background-repeat', 'background-origin', 'background-position', 'background-size', '--ink-img', '--ink-pos', '--ink-size']) s.removeProperty(p);
  el.removeAttribute('data-ink-backdrop');
}

function track(el) {
  if (tracked.has(el)) return;
  tracked.add(el);
  states.set(el, { seed: `rt${nextSeed++}`, key: null, pad: 0, inset: 0, timer: 0 });
  resizeObserver.observe(el);
}

function untrack(el) {
  if (!tracked.delete(el)) return;
  resizeObserver.unobserve(el);
  clear(el);
  states.delete(el);
}

/** Apply `fn` to `node` and its descendants that the painter draws on. */
function eachTarget(node, fn) {
  if (node.nodeType !== 1) return;
  if (node.matches(SELECTOR)) fn(node);
  node.querySelectorAll(SELECTOR).forEach(fn);
}

function onMutations(records) {
  for (const r of records) {
    if (r.type === 'attributes') {
      const el = r.target;
      if (r.attributeName === 'data-ink-frame' && !tracked.has(el) && el.matches(SELECTOR)) track(el);
      else if (tracked.has(el)) paint(el);
      continue;
    }
    r.removedNodes.forEach((n) => eachTarget(n, untrack));
    r.addedNodes.forEach((n) => eachTarget(n, track));
  }
}

function onFocusChange(e) {
  const el = e.target?.closest?.(FIELDS);
  if (el && tracked.has(el)) requestAnimationFrame(() => paint(el));
}

/** Start drawing outlines with the loaded ink `kit` (idempotent). */
export function startInkPainter(loadedKit) {
  kit = loadedKit;
  if (resizeObserver || typeof ResizeObserver === 'undefined') return;
  resizeObserver = new ResizeObserver((entries) => {
    for (const e of entries) onResize(e.target);
  });
  mutationObserver = new MutationObserver(onMutations);
  mutationObserver.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: WATCHED_ATTRS });
  document.addEventListener('focusin', onFocusChange, true);
  document.addEventListener('focusout', onFocusChange, true);
  eachTarget(document.body, track);
}

/** Stop drawing and restore every element the painter touched. */
export function stopInkPainter() {
  if (!resizeObserver) return;
  mutationObserver.disconnect();
  resizeObserver.disconnect();
  document.removeEventListener('focusin', onFocusChange, true);
  document.removeEventListener('focusout', onFocusChange, true);
  for (const el of [...tracked]) {
    clear(el);
    states.delete(el);
  }
  tracked.clear();
  resizeObserver = null;
  mutationObserver = null;
}

/** Redraw everything, e.g. after the theme's colours changed. */
export function repaintInk() {
  if (!resizeObserver) return;
  rev++;
  colors.clear();
  for (const el of tracked) paint(el);
}
