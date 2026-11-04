import React, { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

// Pixel-art agent mascot. Drawn on a 32×24 logical canvas and scaled up with
// nearest-neighbour so it stays crisp. Body follows the logo (foreground
// colour: white shards on dark, ink on light); the tuft is the logo's bolt.
const W = 32;
const H = 24;
const OX = 4;
// Rows of headroom above the 24-row scene so the pet jump and the rising heart
// never leave the drawable area (they used to hard-clip at the canvas top).
const TOP = 6;
const FPS = 12;

const TOKENS = {
  body: 'var(--foreground)',
  eye: 'var(--background)',
  glint: 'var(--foreground)',
  bolt: 'var(--info)',
  boltHi: 'color-mix(in oklab, var(--info) 55%, white)',
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
    hi: 'color-mix(in oklab, var(--foreground) 40%, white)',
    shade: 'color-mix(in oklab, var(--foreground) 72%, var(--background))',
    bubble: 'var(--foreground)',
    ink: 'var(--background)',
    shadow: 'rgb(0 0 0 / 0.35)',
  },
  light: {
    hi: 'color-mix(in oklab, var(--foreground) 78%, var(--background))',
    shade: 'color-mix(in oklab, var(--foreground) 85%, black)',
    bubble: 'var(--background)',
    ink: 'var(--foreground)',
    shadow: 'rgb(0 0 0 / 0.12)',
  },
};
const FALLBACK = {
  dark: { body: '#e9ebf1', hi: '#ffffff', shade: '#aeb3c2', eye: '#15171d', glint: '#ffffff', bolt: '#4f9dff', boltHi: '#a8d0ff', lid: '#3a4152', lidEdge: '#5a627a', bubble: '#e9ebf1', ink: '#15171d', blush: '#ff8fa3', warn: '#f0c45a', ok: '#3ecf8e', bad: '#ff5f6d', drop: '#7cc4ff', shadow: 'rgba(0,0,0,.35)' },
  light: { body: '#17191f', hi: '#3b404e', shade: '#000000', eye: '#f4f5f8', glint: '#17191f', bolt: '#2f7ff0', boltHi: '#79b0ff', lid: '#c9ccd6', lidEdge: '#9aa0b1', bubble: '#ffffff', ink: '#17191f', blush: '#ff7d93', warn: '#e0a92a', ok: '#21b072', bad: '#e5484d', drop: '#3d97f0', shadow: 'rgba(0,0,0,.12)' },
};

/** Resolves theme tokens (scoped to `host`) into canvas-ready colours. */
function resolvePalette(host, ctx) {
  const dark = !!(host.closest('.dark') || document.documentElement.classList.contains('dark'));
  const mode = dark ? 'dark' : 'light';
  const probe = document.createElement('span');
  probe.style.display = 'none';
  host.appendChild(probe);
  const out = {};
  for (const [key, expr] of Object.entries({ ...TOKENS, ...MODE_TOKENS[mode] })) {
    probe.style.color = '';
    probe.style.color = expr;
    const val = getComputedStyle(probe).color;
    ctx.fillStyle = '#010203';
    if (val) ctx.fillStyle = val;
    out[key] = val && ctx.fillStyle !== '#010203' ? val : FALLBACK[mode][key];
  }
  probe.remove();
  return out;
}

const BODY = { 8: [8, 15], 9: [7, 16], 10: [6, 17], 11: [6, 17], 12: [6, 17], 13: [6, 17], 14: [6, 17], 15: [6, 17], 16: [6, 17], 17: [6, 17], 18: [7, 16], 19: [8, 15] };
const BOLT = [[2, [14, 15]], [3, [13, 14]], [4, [12, 13, 14, 15]], [5, [13, 14]], [6, [12, 13]], [7, [11, 12]]];
const BOLT_DROOP = [[4, [17, 18]], [5, [15, 16, 17]], [6, [13, 14]], [7, [11, 12]]];
const HEART = [[1, 0], [3, 0], [0, 1], [1, 1], [2, 1], [3, 1], [4, 1], [1, 2], [2, 2], [3, 2], [2, 3]];
const ZED = [[0, 0], [1, 0], [2, 0], [1, 1], [0, 2], [1, 2], [2, 2]];
const SPARKLES = [[2, 6], [21, 4], [4, 12], [20, 11], [1, 2], [23, 8]];

/** Draws one frame of the mascot into the 32×24 context. */
function drawFrame(g, P, s, t) {
  let ox = OX;
  let oy = TOP;
  const px = (x, y, c) => { g.fillStyle = c; g.fillRect(x + ox, y + oy, 1, 1); };
  const rect = (x0, y0, x1, y1, c) => { g.fillStyle = c; g.fillRect(x0 + ox, y0 + oy, x1 - x0 + 1, y1 - y0 + 1); };
  const nub = (x, y) => { rect(x, y, x + 1, y + 1, P.body); px(x, y + 1, P.shade); };

  const f = Math.floor(t * 8);
  const st = t - s.stateStart;
  const sincePet = t - s.petAt;
  const sinceWake = t - s.wakeAt;
  const state = s.state;

  let lx = 0;
  let ly = 0;
  if (s.pointerFresh) { lx = s.lookX; ly = s.lookY; } else { lx = [0, -1, 0, 1, 0, 0][Math.floor(t / 1.6) % 6] * 0.8; }
  let eyes = t % 4.3 < 0.13 ? 'closed' : 'open';
  let mouth = 'normal';
  let arms = 'down';
  let tap = false;
  let droop = false;
  let jump = 0;
  let sway = [0, 1, 0, -1][Math.floor(t * 2) % 4];
  let bob = Math.floor(t * 2.5) % 2;
  const minis = [];

  if (state === 'thinking') { lx = 0.6; ly = -1; arms = 'think'; mouth = 'flat'; sway = 0; }
  else if (state === 'tool') { lx = 0; ly = 1; arms = 'type'; mouth = f % 16 < 12 ? 'normal' : 'tongue'; eyes = 'open'; }
  else if (state === 'waiting') {
    tap = true;
    const n = Math.max(1, Math.min(4, s.subagents || 1));
    for (let i = 0; i < n; i++) {
      const a = t * 1.1 + (i / n) * Math.PI * 2;
      minis.push({ x: Math.round(OX + 11 + Math.cos(a) * 13), y: 18 + Math.round(Math.sin(a)), front: Math.sin(a) > 0, i });
      if (i === 0) lx = Math.cos(a) > 0 ? 1 : -1;
    }
  } else if (state === 'error') { eyes = st < 2.5 ? 'x' : 'open'; mouth = 'wavy'; droop = true; arms = st < 2.5 ? 'down' : 'shrug'; sway = 0; }
  else if (state === 'done') {
    eyes = 'happy'; mouth = 'smile';
    if (st < 2.4) { const p = (st % 0.8) / 0.8; jump = -Math.round(Math.sin(p * Math.PI) * 4); arms = 'up'; } else arms = 'wave';
  } else if (state === 'sleep') { eyes = 'closed'; mouth = 'flat'; bob = Math.floor(t * 1.2) % 2; sway = 0; }

  if (sinceWake < 0.7) { eyes = 'wide'; mouth = 'o'; }
  if (sincePet < 1.2) { eyes = 'happy'; mouth = 'smile'; if (sincePet < 0.45) jump = -Math.round(Math.sin((sincePet / 0.45) * Math.PI) * 3); }

  const drawMini = (x, y, i) => {
    const sx = ox; const sy = oy; ox = 0; oy = TOP;
    const step = (f + i) % 4 < 2;
    rect(x, y, x + 2, y + 2, P.body); px(x, y + 1, P.eye); px(x + 2, y + 1, P.eye);
    px(x + 1, y - 1, P.bolt); px(x + 2, y - 2, P.bolt);
    px(x, y + 3 - (step ? 1 : 0), P.shade); px(x + 2, y + 3 - (step ? 0 : 1), P.shade);
    ox = sx; oy = sy;
  };

  g.clearRect(0, 0, W, H + TOP);
  const shrink = Math.min(3, -jump);
  rect(7 + shrink, 22, 16 - shrink, 22, P.shadow);
  for (const m of minis) if (!m.front) drawMini(m.x, m.y, m.i);

  oy = TOP + jump;
  rect(8, 20, 9, 21, P.shade);
  if (tap && f % 4 < 2) rect(14, 20, 15, 20, P.shade); else rect(14, 20, 15, 21, P.shade);

  oy = TOP + jump + bob;
  for (const y in BODY) { const [a, b] = BODY[y]; rect(a, +y, b, +y, P.body); }
  rect(8, 9, 10, 9, P.hi); px(7, 10, P.hi); px(7, 11, P.hi);
  rect(7, 18, 16, 18, P.shade); rect(8, 19, 15, 19, P.shade);
  for (let y = 12; y <= 17; y++) px(17, y, P.shade);

  const boltCol = state === 'thinking' && f % 4 < 2 ? P.boltHi : P.bolt;
  for (const [y, xs] of droop ? BOLT_DROOP : BOLT) {
    const sh = !droop && y <= 4 ? sway : 0;
    for (const x of xs) px(x + sh, y, boltCol);
  }
  if (state === 'tool' && f % 3 === 0) px(15 + (f % 2), 1, P.boltHi);

  const dx = Math.max(-1, Math.min(1, Math.round(lx)));
  const dy = Math.max(-1, Math.min(1, Math.round(ly)));
  const L = 9 + dx; const R = 13 + dx; const T = 12 + dy;
  if (eyes === 'open') for (const e of [L, R]) { rect(e, T, e + 1, T + 2, P.eye); px(e + (lx > 0.3 ? 1 : 0), T + (ly > 0.3 ? 1 : 0), P.glint); }
  else if (eyes === 'closed') for (const e of [L, R]) rect(e, T + 2, e + 1, T + 2, P.eye);
  else if (eyes === 'happy') for (const e of [L, R]) { px(e - 1, T + 1, P.eye); rect(e, T, e + 1, T, P.eye); px(e + 2, T + 1, P.eye); }
  else if (eyes === 'x') for (const e of [L - 1, R]) { px(e, T, P.eye); px(e + 2, T, P.eye); px(e + 1, T + 1, P.eye); px(e, T + 2, P.eye); px(e + 2, T + 2, P.eye); }
  else if (eyes === 'wide') for (const e of [L, R]) { rect(e, T - 1, e + 1, T + 2, P.eye); px(e, T - 1, P.glint); }

  const c = P.eye;
  if (mouth === 'smile') { px(10, 16, c); px(13, 16, c); rect(11, 17, 12, 17, c); }
  else if (mouth === 'o') rect(11, 16, 12, 17, c);
  else if (mouth === 'wavy') { px(10, 17, c); px(11, 16, c); px(12, 17, c); px(13, 16, c); }
  else if (mouth === 'flat') rect(11, 17, 12, 17, c);
  else if (mouth === 'tongue') { rect(11, 16, 12, 16, c); px(12, 17, P.blush); }
  else rect(11, 16, 12, 16, c);
  if (sincePet < 1.2) { px(7, 16, P.blush); px(16, 16, P.blush); }

  if (state === 'tool') {
    rect(7, 15, 16, 19, P.lid); rect(7, 15, 16, 15, P.lidEdge); rect(5, 20, 18, 20, P.lidEdge);
    px(11, 17, f % 6 < 3 ? P.bolt : P.boltHi); px(12, 17, P.bolt);
  }

  if (arms === 'down') { nub(4, 14); nub(18, 14); }
  else if (arms === 'up') { nub(4, 8); px(5, 10, P.body); px(5, 11, P.body); nub(18, 8); px(18, 10, P.body); px(18, 11, P.body); }
  else if (arms === 'think') { nub(4, 14); nub(18, 12); px(17, 13, P.body); }
  else if (arms === 'wave') { nub(4, 14); if (f % 4 < 2) { nub(18, 8); px(18, 10, P.body); px(18, 11, P.body); } else { nub(19, 9); px(18, 11, P.body); } }
  else if (arms === 'type') { const a = f % 2; nub(8, 15 + a); nub(14, 16 - a); }
  else if (arms === 'shrug') { nub(3, 11); px(5, 12, P.body); nub(19, 11); px(18, 12, P.body); }

  if (state === 'thinking') {
    px(18, 8, P.bubble); px(19, 6, P.bubble); px(20, 6, P.bubble);
    rect(21, 0, 26, 0, P.ink); rect(21, 5, 26, 5, P.ink); rect(20, 1, 20, 4, P.ink); rect(27, 1, 27, 4, P.ink);
    rect(21, 1, 26, 4, P.bubble);
    const n = Math.floor(f / 2) % 4;
    for (let i = 0; i < n; i++) px(22 + i * 2, 3, P.ink);
  }
  if (state === 'error' && st < 2.5) {
    for (let k = 0; k < 3; k++) { const a = t * 5 + k * 2.09; px(11 + Math.round(Math.cos(a) * 5), 5 + Math.round(Math.sin(a) * 1.5), P.warn); }
    if (f % 4 < 3) { px(18, 9, P.drop); px(18, 10, P.drop); }
  }
  if (state === 'done') {
    SPARKLES.forEach(([sx, sy], i) => {
      if ((f + i * 2) % 6 >= 3) return;
      const col = i % 2 ? P.ok : P.warn;
      px(sx, sy, col); px(sx - 1, sy, col); px(sx + 1, sy, col); px(sx, sy - 1, col); px(sx, sy + 1, col);
    });
  }
  if (sincePet < 1.2) { const hy = 3 - Math.floor(sincePet * 5); for (const [hx, hyy] of HEART) px(9 + hx, hy + hyy, P.bad); }
  if (state === 'sleep') {
    for (let k = 0; k < 2; k++) {
      const ph = (t * 0.6 + k * 0.5) % 1;
      const zx = 18 + k * 3 + Math.round(ph * 2);
      const zy = 7 - Math.round(ph * 7);
      if (zy >= 0) for (const [a, b] of ZED) px(zx + a, zy + b, P.body);
    }
  }

  oy = TOP;
  for (const m of minis) if (m.front) drawMini(m.x, m.y, m.i);
}

// Crops into the padded (H + TOP) scene. hero: full scene + 4 rows of headroom.
// mini: crop around the character for inline rows. ring: the character sitting
// inside the context ring, with headroom so the heart can float out above it.
const SIZES = {
  hero: { sx: 0, sy: TOP - 4, sw: W, sh: H + 4, scale: 6 },
  mini: { sx: OX + 2, sy: TOP - 2, sw: 20, sh: 24, scale: 1.4 },
  ring: { sx: OX + 1, sy: TOP - 5, sw: 22, sh: 29, scale: 1.15 },
};
const SLEEP_AFTER_MS = 15000;
const STATE_LABELS = {
  idle: 'Ready', thinking: 'Thinking', tool: 'Running a tool', waiting: 'Waiting on sub-agents',
  error: 'Something went wrong', done: 'Done', sleep: 'Napping',
};

/**
 * Animated pixel-art agent mascot; `state` drives the pose, idle naps on its own.
 * Changing `petKey` pets it (hop + heart) from outside; `interactive={false}`
 * lets a wrapping button own the click.
 */
export function AgentMascot({ state = 'idle', size = 'hero', subagents = 0, petKey = 0, interactive = true, className, style }) {
  const canvasRef = useRef(null);
  const simRef = useRef({ state, stateStart: 0, petAt: -99, wakeAt: -99, subagents, lookX: 0, lookY: 0, pointerFresh: false });
  const lastPointerRef = useRef(performance.now());
  const [shown, setShown] = useState(state);
  const geom = SIZES[size] || SIZES.hero;
  const cssW = Math.round(geom.sw * geom.scale);
  const cssH = Math.round(geom.sh * geom.scale);

  useEffect(() => {
    const sim = simRef.current;
    sim.subagents = subagents;
    if (sim.state !== state && !(sim.state === 'sleep' && state === 'idle')) {
      if (sim.state === 'sleep') sim.wakeAt = performance.now() / 1000;
      sim.state = state;
      sim.stateStart = performance.now() / 1000;
      lastPointerRef.current = performance.now();
      setShown(state);
    }
  }, [state, subagents]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const sim = simRef.current;
    const off = document.createElement('canvas');
    off.width = W;
    off.height = H + TOP;
    const g = off.getContext('2d');
    const ctx = canvas.getContext('2d');
    let palette = resolvePalette(canvas.parentElement || document.body, g);
    const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

    const sizeBacking = () => {
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
      ctx.imageSmoothingEnabled = false;
    };
    sizeBacking();

    const paint = (t) => {
      sim.pointerFresh = performance.now() - lastPointerRef.current < 3000 && sim.hasPointer;
      if (!reduced && sim.state === 'idle' && performance.now() - lastPointerRef.current > SLEEP_AFTER_MS) {
        sim.state = 'sleep';
        sim.stateStart = t;
        setShown('sleep');
      }
      drawFrame(g, palette, sim, reduced ? sim.stateStart + 3 : t);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(off, geom.sx, geom.sy, geom.sw, geom.sh, 0, 0, canvas.width, canvas.height);
    };

    let raf = 0;
    let last = 0;
    let visible = true;
    const loop = (ms) => {
      raf = requestAnimationFrame(loop);
      if (!visible || ms - last < 1000 / FPS) return;
      last = ms;
      paint(ms / 1000);
    };
    if (reduced) paint(performance.now() / 1000);
    else {
      paint(performance.now() / 1000);
      raf = requestAnimationFrame(loop);
    }

    const io = new IntersectionObserver(([e]) => { visible = e.isIntersecting; });
    io.observe(canvas);

    const onPointer = (e) => {
      const r = canvas.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height * (size === 'hero' ? 0.55 : 0.5);
      sim.lookX = Math.max(-1, Math.min(1, (e.clientX - cx) / 110));
      sim.lookY = Math.max(-1, Math.min(1, (e.clientY - cy) / 90));
      sim.hasPointer = true;
      lastPointerRef.current = performance.now();
      if (sim.state === 'sleep') {
        sim.state = 'idle';
        sim.wakeAt = performance.now() / 1000;
        sim.stateStart = sim.wakeAt;
        setShown('idle');
      }
    };
    window.addEventListener('pointermove', onPointer, { passive: true });

    // Theme switches rewrite root classes / inline token vars; re-resolve the palette.
    const mo = new MutationObserver(() => {
      palette = resolvePalette(canvas.parentElement || document.body, g);
      if (reduced) paint(performance.now() / 1000);
    });
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] });
    const onDpr = () => sizeBacking();
    window.addEventListener('resize', onDpr);

    return () => {
      cancelAnimationFrame(raf);
      io.disconnect();
      mo.disconnect();
      window.removeEventListener('pointermove', onPointer);
      window.removeEventListener('resize', onDpr);
    };
  }, [cssW, cssH, geom.sx, geom.sy, geom.sw, geom.sh, size]);

  const pet = () => {
    const sim = simRef.current;
    sim.petAt = performance.now() / 1000;
    lastPointerRef.current = performance.now();
    if (sim.state === 'sleep') {
      sim.state = 'idle';
      sim.wakeAt = sim.petAt;
      sim.stateStart = sim.petAt;
      setShown('idle');
    }
  };

  useEffect(() => {
    if (petKey) pet();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [petKey]);

  return (
    <canvas
      ref={canvasRef}
      onClick={interactive ? pet : undefined}
      role="img"
      aria-label={`Agent: ${STATE_LABELS[shown] || shown}`}
      title={interactive ? STATE_LABELS[shown] || shown : undefined}
      className={cn(
        'shrink-0 select-none [image-rendering:pixelated]',
        interactive ? 'cursor-pointer' : 'pointer-events-none',
        className,
      )}
      style={{ width: cssW, height: cssH, ...style }}
    />
  );
}

const TERMINAL_FAIL = new Set(['failed', 'error']);
const TERMINAL_STOP = new Set(['cancelled', 'canceled', 'aborted']);

/** Maps live agent run signals to a mascot pose, with a brief done/error beat after each run. */
export function useMascotRunState({ taskId, isStreaming, runningTool, waitingOnSubagents, status }) {
  const base = waitingOnSubagents > 0 ? 'waiting' : isStreaming ? (runningTool ? 'tool' : 'thinking') : 'idle';
  const busy = base !== 'idle';
  const [beat, setBeat] = useState(null);
  const prevRef = useRef({ taskId, busy });
  const norm = String(status || '').toLowerCase();

  useEffect(() => {
    const prev = prevRef.current;
    prevRef.current = { taskId, busy };
    if (prev.taskId !== taskId) { setBeat(null); return; }
    if (busy) { setBeat(null); return; }
    if (prev.busy && !TERMINAL_STOP.has(norm)) {
      setBeat({ kind: TERMINAL_FAIL.has(norm) ? 'error' : 'done', at: Date.now() });
    }
  }, [taskId, busy, norm]);

  // The terminal status can land a moment after streaming stops; upgrade the beat if so.
  useEffect(() => {
    if (!beat || Date.now() - beat.at > 1500) return;
    if (TERMINAL_STOP.has(norm)) setBeat(null);
    else if (TERMINAL_FAIL.has(norm) && beat.kind !== 'error') setBeat({ kind: 'error', at: beat.at });
  }, [norm, beat]);

  useEffect(() => {
    if (!beat) return undefined;
    const id = setTimeout(() => setBeat(null), beat.kind === 'error' ? 5000 : 3200);
    return () => clearTimeout(id);
  }, [beat]);

  return beat ? beat.kind : base;
}

export default AgentMascot;
