// Lazy chunk: Looseleaf's ink engine (vendored in src/lib/ink) plus the recipes
// Rustic draws. Imported only when the active theme sets `style.ink`, so other
// themes never load it.
import { defineRecipe, DEFAULT_KNOBS } from '@/lib/ink/recipes';
import { Ink, inkFrame, frameSvg, ensureInkDefs } from '@/lib/ink/react';
import { box, dashedBox } from '@/lib/ink/shapes';
import { wash } from '@/lib/ink/fills';
import { roundRectPoly, seg } from '@/lib/ink/geom';
import { clamp } from '@/lib/ink/rng';
import { drawSkeleton } from '@/lib/ink/art-draw';
import { doodles } from '@/lib/ink/art/doodles';
import { buttonRecipe } from '@/lib/ink/recipes/button-ink';
import { badgeRecipe } from '@/lib/ink/recipes/badge-ink';
import { checkboxRecipe } from '@/lib/ink/recipes/checkbox-ink';
import { switchRecipe, switchThumbRecipe } from '@/lib/ink/recipes/switch-ink';
import { tabsRecipe } from '@/lib/ink/recipes/tabs-ink';
import { toggleRecipe } from '@/lib/ink/recipes/toggle-ink';

// Long outlines use the engine's cheaper single-pass stroke.
const WIDE_PX = 360;
const strokeOpts = (w, extra = {}) => (w > WIDE_PX ? { simple: true, ...extra } : extra);

// The single-pass stroke cuts straight across corners tighter than this
// (a filled wedge in ~10% of draws at 2px), so no outline goes below it.
const MIN_CORNER = 4;

/** Square outline as four separate strokes that overshoot a little at the corners. */
function edgeBox(ink, w, h, pen) {
  const rng = ink.next('edges');
  const o = { start: 'soft', end: 'soft', wobble: 0.4, simple: true };
  const n = (len) => Math.max(3, Math.ceil(len / 60));
  const j = () => rng.gauss(0, 0.5);
  const over = () => rng.range(0.5, 2.5);
  ink.stroke(seg(-over(), j(), w + over(), j(), rng.gauss(0, 0.3), n(w)), pen, o);
  ink.stroke(seg(w + j(), -over(), w + j(), h + over(), rng.gauss(0, 0.3), n(h)), pen, o);
  ink.stroke(seg(w + over(), h + j(), -over(), h + j(), rng.gauss(0, 0.3), n(w)), pen, o);
  ink.stroke(seg(j(), h + over(), j(), -over(), rng.gauss(0, 0.3), n(h)), pen, o);
}

/** Text field: a paper slip with a pencil outline, blue while focused. */
const fieldRecipe = defineRecipe({
  id: 'rt-field',
  kind: 'field',
  layers: ['rest'],
  pad: 4,
  draw(ink, { w, h }, { focus, invalid, disabled }) {
    const r = clamp(Math.min(w, h) * 0.14, MIN_CORNER, 5);
    const body = roundRectPoly(1.5, 1.5, w - 3, h - 3, r);
    ink.layer('rest', () => {
      if (disabled) {
        wash(ink, body, 'paper-shade', { alpha: 0.9, material: 'wash' });
        dashedBox(ink, 0, 0, w, h, { r, pen: 'pencil', opts: { color: 'pencil-mark' } });
        return;
      }
      ink.poly('under', body, 'slip', 0.7);
      const pen = invalid ? 'red' : focus ? 'blue' : 'pencil';
      box(ink, 0, 0, w, h, { pen, corner: r, opts: strokeOpts(w, { width: focus || invalid ? 1.25 : 1.15 }) });
    });
  },
});

/** Floating sheet (dialog, popover, menu): one confident pen outline. */
const surfaceRecipe = defineRecipe({
  id: 'rt-surface',
  kind: 'sheet',
  layers: ['rest'],
  pad: 4,
  draw(ink, { w, h }, { corner = 5 }) {
    ink.layer('rest', () => box(ink, 0, 0, w, h, { pen: 'ink', corner: Math.max(MIN_CORNER, corner), opts: strokeOpts(w) }));
  },
});

/** App chrome (panels, cards, the prompt box): a lighter pencil outline. */
const frameRecipe = defineRecipe({
  id: 'rt-frame',
  kind: 'frame',
  layers: ['rest'],
  pad: 4,
  draw(ink, { w, h }, { tone, corner = MIN_CORNER }) {
    const pen = tone === 'prompt' ? 'ink' : tone === 'card' ? 'pencil.fine' : 'pencil';
    ink.layer('rest', () => {
      if (tone === 'panel') edgeBox(ink, w, h, pen);
      else box(ink, 0, 0, w, h, { pen, corner: Math.max(MIN_CORNER, corner), opts: { simple: true } });
    });
  },
});

/** A single pencil rule along one edge of an element. */
const ruleRecipe = defineRecipe({
  id: 'rt-rule',
  kind: 'rule',
  layers: ['rest'],
  pad: 4,
  draw(ink, { w, h }, { edge }) {
    const rng = ink.next('rule');
    const bow = rng.gauss(0, 0.35);
    const pts = edge === 'top' ? seg(0, 1.5, w, 1.5, bow, Math.max(3, Math.ceil(w / 40)))
      : edge === 'left' ? seg(1.5, 0, 1.5, h, bow, Math.max(3, Math.ceil(h / 40)))
      : edge === 'right' ? seg(w - 1.5, 0, w - 1.5, h, bow, Math.max(3, Math.ceil(h / 40)))
      : seg(0, h - 1.5, w, h - 1.5, bow, Math.max(3, Math.ceil(w / 40)));
    ink.layer('rest', () => ink.stroke(pts, 'pencil', { start: 'soft', end: 'soft', wobble: 0.5, simple: true }));
  },
});

export const RECIPES = {
  button: buttonRecipe,
  badge: badgeRecipe,
  checkbox: checkboxRecipe,
  switch: switchRecipe,
  'switch-thumb': switchThumbRecipe,
  tabs: tabsRecipe,
  toggle: toggleRecipe,
  field: fieldRecipe,
  surface: surfaceRecipe,
  frame: frameRecipe,
  rule: ruleRecipe,
};

export const DOODLE_NAMES = Object.keys(doodles);

/** Draw a named doodle at `size` px; returns the engine's result (`svg` markup, `w`, `h`, `pad`). */
export function drawDoodle(name, size, seed) {
  const def = doodles[name];
  return def ? drawSkeleton(def, size, seed, { tone: 'doodle' }) : null;
}

export { Ink, inkFrame, frameSvg, ensureInkDefs, DEFAULT_KNOBS };
