// Procedural animation for the 3D mascot. Each run state is a pose function of
// time; poses are cross-faded by smoothed weights so state changes never snap.
// Pure numbers in, numbers out: the scene maps channels onto nodes.
//
// The send button shows the mascot about 28 px wide with no props, and it is the
// only place the working states are seen. There one scene unit is ~9 px, so a
// state has to read from whole-body motion: `c` (compact) switches a pose to
// that larger, simpler version.

export const STATES = [
  'idle', 'thinking', 'tool', 'waiting', 'error', 'done', 'sleep',
  'asking', 'listening', 'condensing', 'retry',
];

const CHANNELS = [
  'x', 'y', 'sy', 'bob', 'yaw', 'pitch', 'roll',
  'armLUp', 'armLFwd', 'armRUp', 'armRFwd', 'footL', 'footR',
  'eyeOpen', 'eyeWide', 'eyeHappy', 'eyeCross', 'lookX', 'lookY', 'lookFree',
  'smile', 'flat', 'wavy', 'o', 'tongue', 'blush',
  'laptop', 'bubble', 'zzz', 'sparkle', 'dizzy', 'drop', 'minis', 'question', 'waves',
  'heart', 'heartT',
];
const DEFAULTS = { sy: 1, eyeOpen: 1 };

const { sin, cos, abs, max, min, PI } = Math;

/** Hermite step from 0 at `a` to 1 at `b`. */
export function smoothstep(a, b, x) {
  const t = max(0, min(1, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
}

/** Frame-rate independent exponential approach of `a` toward `b`. */
export function damp(a, b, lambda, dt) {
  return b + (a - b) * Math.exp(-lambda * dt);
}

const mix = (a, b, k) => a + (b - a) * k;
const clamp1 = (v) => max(-1, min(1, v));

/** Resets every channel of a pose to its neutral value. */
function resetPose(p) {
  for (const ch of CHANNELS) p[ch] = DEFAULTS[ch] || 0;
  return p;
}

/** Adds one hop cycle (`u` in 0..1) to a pose: anticipation squash, arc, stretch. */
function hop(p, u, height) {
  const ground = 0.2;
  if (u < ground) {
    p.sy -= 0.14 * sin((u / ground) * PI);
    return;
  }
  const a = (u - ground) / (1 - ground);
  p.y += 4 * height * a * (1 - a);
  p.sy += 0.09 * sin(a * PI);
}

const breathe = (p, t, amount) => { p.sy += amount * sin(t * 2.1); };

// Compact motion per state: idle is still, thinking rocks, tool pulses behind
// the laptop, waiting turns its head, asking hops, listening tilts, condensing
// squeezes, retry ticks like a metronome, error shakes, done hops happily.
const POSES = {
  idle(p, t, st, c) {
    breathe(p, t, c ? 0.03 : 0.015);
    p.roll = 0.035 * sin(t * 0.7);
    p.armLUp = p.armRUp = 0.05 * sin(t * 2.1 + 0.6);
    p.lookFree = 1;
  },
  thinking(p, t, st, c) {
    p.lookY = -0.85;
    p.flat = 1;
    if (c) {
      const rock = sin(t * 2.4);
      p.x = 0.2 * rock;
      p.roll = -0.2 * rock;
      p.lookX = 0.6 * rock;
      return;
    }
    breathe(p, t, 0.012);
    p.lookX = 0.55;
    p.pitch = -0.07;
    p.roll = -0.06 + 0.02 * sin(t * 1.3);
    p.armRUp = 1.25 + 0.05 * sin(t * 2.4);
    p.armRFwd = 0.75;
    p.bubble = 1;
  },
  tool(p, t, st, c) {
    const k = t * 21;
    const x = t % 2.6;
    p.lookY = 0.8;
    p.pitch = 0.07;
    p.armLUp = p.armRUp = -0.35;
    p.armLFwd = 1.05 + 0.2 * sin(k);
    p.armRFwd = 1.05 + 0.2 * sin(k + PI);
    p.laptop = 1;
    if (c) {
      p.sy -= 0.12 * abs(sin(t * 9));
      return;
    }
    p.bob = 0.012 * sin(k);
    p.tongue = smoothstep(1.85, 2.0, x) - smoothstep(2.45, 2.6, x);
  },
  waiting(p, t, st, c) {
    breathe(p, t, c ? 0.03 : 0.015);
    p.footR = max(0, sin(t * 7.5));
    p.armLUp = p.armRUp = 0.12;
    if (c) {
      const turn = clamp1(1.7 * sin(t * 1.5));
      p.lookX = turn;
      p.x = 0.08 * turn;
      return;
    }
    p.lookX = 0.9 * cos(t * 1.1);
    p.lookY = 0.45;
    p.minis = 1;
  },
  error(p, t, st, c) {
    const early = 1 - smoothstep(2.3, 2.7, st);
    p.sy -= c ? 0.1 : 0.05;
    p.eyeCross = early;
    p.wavy = 1;
    p.dizzy = early;
    p.drop = early;
    p.roll = 0.11 * sin(t * 5.5) * early;
    if (c) p.x = 0.14 * sin(t * 15) * early;
    p.armLUp = p.armRUp = 0.75 * (1 - early);
    p.lookY = 0.3 * (1 - early);
  },
  done(p, t, st, c) {
    const early = 1 - smoothstep(2.2, 2.5, st);
    p.eyeHappy = 1;
    p.smile = 1;
    p.blush = 0.5;
    if (st < 2.4) hop(p, (st % 0.6) / 0.6, c ? 0.55 : 0.5);
    p.armLUp = 1.9 * early;
    p.armRUp = mix(1.85 + 0.35 * sin(t * 10), 1.9, early);
    p.sparkle = 0.35 + 0.65 * early;
  },
  sleep(p, t, st, c) {
    p.sy += -0.06 + (c ? 0.07 : 0.035) * sin(t * 1.15);
    p.eyeOpen = 0;
    p.flat = 0.7;
    p.pitch = c ? 0.16 : 0.09;
    p.roll = c ? 0.12 : 0.04;
    p.zzz = 1;
  },
  asking(p, t, st, c) {
    p.armRUp = 1.9 + 0.3 * sin(t * 9);
    p.eyeWide = 0.5;
    p.o = 0.6;
    p.question = 1;
    p.lookFree = 1;
    if (c) {
      hop(p, (t * 1.6) % 1, 0.3);
      return;
    }
    breathe(p, t, 0.015);
    p.y = 0.05 * abs(sin(t * 2.6));
    p.roll = -0.05;
  },
  listening(p, t, st, c) {
    breathe(p, t, 0.012);
    p.pitch = 0.06;
    p.roll = c ? 0.22 : 0.09;
    if (c) p.sy += 0.05 * sin(t * 7);
    p.armLUp = 0.2;
    p.eyeWide = 0.35;
    p.smile = 0.35;
    p.waves = 1;
    p.lookFree = 1;
  },
  condensing(p, t, st, c) {
    const k = 0.5 + 0.5 * sin(t * (c ? 4 : 5.5));
    p.sy -= c ? 0.08 + 0.2 * k : 0.07 + 0.07 * k;
    if (!c) p.yaw = 0.025 * sin(t * 31) * k;
    p.eyeOpen = 0.12;
    p.flat = 1;
    p.armLUp = p.armRUp = -0.3;
    p.armLFwd = p.armRFwd = 0.5 + 0.2 * k;
  },
  retry(p, t, st, c) {
    breathe(p, t, 0.015);
    if (c) p.roll = 0.2 * clamp1(3 * sin(t * 3.2));
    p.footL = max(0, sin(t * 6));
    p.flat = 1;
    p.armLUp = p.armRUp = 0.1;
    p.drop = 1;
    p.lookFree = 1;
  },
};

/** Creates per-instance rig state (blend weights, blink and gaze timers). */
export function createRig() {
  const weights = {};
  const started = {};
  for (const k of STATES) { weights[k] = 0; started[k] = 0; }
  return {
    weights,
    started,
    current: null,
    blinkAt: -9,
    blinkNext: 1.5 + Math.random() * 3,
    wanderX: 0,
    wanderY: 0,
    wanderNext: 0,
    lookX: 0,
    lookY: 0,
    pose: resetPose({}),
    scratch: resetPose({}),
  };
}

/**
 * Advances the rig to time `t` and returns the blended pose. `sim` carries the
 * run state (`state`, `stateStart`), pet/wake timestamps and the pointer gaze;
 * `compact` selects the send-button poses.
 */
export function stepRig(rig, sim, t, dt, compact = false) {
  const state = POSES[sim.state] ? sim.state : 'idle';
  if (rig.current !== state) {
    if (rig.current === null) rig.weights[state] = 1;
    rig.current = state;
    rig.started[state] = sim.stateStart;
  }

  const out = rig.pose;
  for (const ch of CHANNELS) out[ch] = 0;
  let total = 0;
  for (const k of STATES) {
    const w = damp(rig.weights[k], k === state ? 1 : 0, 9, dt);
    rig.weights[k] = w;
    if (w < 0.002) continue;
    const p = resetPose(rig.scratch);
    POSES[k](p, t, t - rig.started[k], compact);
    for (const ch of CHANNELS) out[ch] += w * p[ch];
    total += w;
  }
  if (total > 0) for (const ch of CHANNELS) out[ch] /= total;

  const sinceWake = t - sim.wakeAt;
  if (sinceWake >= 0 && sinceWake < 0.9) {
    const k = 1 - smoothstep(0.55, 0.9, sinceWake);
    out.eyeWide = max(out.eyeWide, k);
    out.eyeOpen = mix(out.eyeOpen, 1, k);
    out.o = max(out.o, k);
    out.smile *= 1 - k;
    out.flat *= 1 - k;
    out.y += (compact ? 0.25 : 0.1) * sin(min(1, sinceWake / 0.35) * PI);
  }

  const sincePet = t - sim.petAt;
  if (sincePet >= 0 && sincePet < 1.3) {
    const k = smoothstep(0, 0.08, sincePet) * (1 - smoothstep(0.95, 1.3, sincePet));
    out.eyeHappy = max(out.eyeHappy, k);
    out.eyeCross *= 1 - k;
    out.smile = max(out.smile, k);
    out.o *= 1 - k;
    out.flat *= 1 - k;
    out.wavy *= 1 - k;
    out.blush = max(out.blush, k);
    if (sincePet < 0.5) hop(out, sincePet / 0.5, 0.42);
    out.heart = k;
    out.heartT = sincePet / 1.3;
  }

  if (t > rig.blinkNext) {
    rig.blinkAt = t;
    rig.blinkNext = t + 2.2 + Math.random() * 3.8;
  }
  const blink = t - rig.blinkAt;
  if (blink < 0.16) out.eyeOpen *= 1 - sin((blink / 0.16) * PI);

  if (t > rig.wanderNext) {
    rig.wanderNext = t + 1.2 + Math.random() * 2.2;
    const glance = Math.random() < 0.65;
    rig.wanderX = glance ? (Math.random() * 2 - 1) * 0.7 : 0;
    rig.wanderY = glance ? (Math.random() * 2 - 1) * 0.25 : 0;
  }
  const freeX = sim.pointerFresh ? sim.lookX : rig.wanderX;
  const freeY = sim.pointerFresh ? sim.lookY : rig.wanderY;
  rig.lookX = damp(rig.lookX, mix(out.lookX, freeX, out.lookFree), 10, dt);
  rig.lookY = damp(rig.lookY, mix(out.lookY, freeY, out.lookFree), 10, dt);
  out.lookX = rig.lookX;
  out.lookY = rig.lookY;

  return out;
}
