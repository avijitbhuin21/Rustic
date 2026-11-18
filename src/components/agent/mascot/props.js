// Procedural props around the mascot: thought bubble, Zs, sparkles, dizzy
// stars, sweat drop, orbiting minis, question mark, sound waves and the heart.
import { damp, smoothstep } from './rig';
import { setRgb } from './shaders';

const { sin, cos, abs, max, min, PI } = Math;
const SPARKLES = [[-1.7, 2.3], [1.75, 2.45], [-1.5, 0.95], [1.6, 1.1], [-0.9, 2.85], [0.95, 2.9]];

/** Builds every prop into `scene` and returns palette + per-frame update hooks. */
export function buildProps(THREE, M, scene) {
  const root = new THREE.Group();
  scene.add(root);
  const tints = [];
  const mat = (key, opts, rim) => {
    const m = M.solid(opts);
    tints.push({ m, key, rim });
    return m;
  };
  const mesh = (geom, material, parent, x = 0, y = 0, z = 0, s = 1) => {
    const o = new THREE.Mesh(geom, material);
    o.position.set(x, y, z);
    if (typeof s === 'number') o.scale.setScalar(s); else o.scale.set(s[0], s[1], s[2]);
    o.frustumCulled = false;
    parent.add(o);
    return o;
  };
  const group = (x = 0, y = 0, z = 0) => {
    const g = new THREE.Group();
    g.position.set(x, y, z);
    g.visible = false;
    root.add(g);
    return g;
  };
  const sphere = new THREE.SphereGeometry(1, 24, 16);
  const flat = { unlit: 1, side: THREE.DoubleSide };

  const bubble = group(1.56, 2.6, 0.25);
  const bubMat = mat('bubble', { unlit: 0.75 });
  const hullMat = mat('ink', { unlit: 1, side: THREE.BackSide });
  const puff = (x, y, rx, ry) => {
    mesh(sphere, bubMat, bubble, x, y, 0, [rx, ry, min(rx, 0.16)]);
    mesh(sphere, hullMat, bubble, x, y, 0, [rx + 0.04, ry + 0.04, min(rx, 0.16) + 0.01]);
  };
  puff(0, 0, 0.6, 0.4);
  puff(-0.6, -0.46, 0.11, 0.11);
  puff(-0.84, -0.7, 0.065, 0.065);
  const inkMat = mat('ink', { unlit: 1 });
  const dots = [-0.24, 0, 0.24].map((x) => mesh(sphere, inkMat, bubble, x, 0, 0.17, 0.065));

  const zMat = mat('furTip', { unlit: 0.7 });
  const bar = new THREE.BoxGeometry(1, 0.22, 0.2);
  const zs = [0, 1, 2].map(() => {
    const g = group();
    mesh(bar, zMat, g, 0, 0.39);
    mesh(bar, zMat, g, 0, -0.39);
    mesh(bar, zMat, g, 0, 0, 0, [1.25, 1, 1]).rotation.z = 0.66;
    return g;
  });

  const starShape = new THREE.Shape();
  for (let i = 0; i < 8; i++) {
    const r = i % 2 ? 0.36 : 1;
    const a = (i / 8) * PI * 2 + PI / 2;
    if (i) starShape.lineTo(cos(a) * r, sin(a) * r); else starShape.moveTo(cos(a) * r, sin(a) * r);
  }
  const star = new THREE.ShapeGeometry(starShape);
  const okMat = mat('ok', flat);
  const warnMat = mat('warn', flat);
  const sparkles = SPARKLES.map(([x, y], i) => {
    const g = group(x, y, 0.45);
    mesh(star, i % 2 ? okMat : warnMat, g);
    return g;
  });
  const dizzy = [0, 1, 2].map(() => {
    const g = group();
    mesh(star, warnMat, g);
    return g;
  });

  const dropMat = mat('drop', { gloss: 0.6 });
  const drop = group(1.12, 1.95, 0.3);
  mesh(sphere, dropMat, drop, 0, 0, 0, 0.085);
  mesh(new THREE.ConeGeometry(0.074, 0.17, 16), dropMat, drop, 0, 0.115, 0);

  const miniBody = mat('furTip', {}, 'rim');
  const miniEye = mat('eye', { gloss: 0.4 });
  const minis = [0, 1, 2, 3].map(() => {
    const g = group();
    mesh(sphere, miniBody, g, 0, 0, 0, [0.27, 0.25, 0.26]);
    mesh(sphere, miniEye, g, -0.1, 0.03, 0.23, [0.04, 0.055, 0.03]);
    mesh(sphere, miniEye, g, 0.1, 0.03, 0.23, [0.04, 0.055, 0.03]);
    return g;
  });

  const askMat = mat('warn', { unlit: 0.6 });
  const question = group(1.4, 2.6, 0.3);
  mesh(new THREE.TorusGeometry(0.2, 0.07, 12, 28, PI * 1.5), askMat, question, 0, 0.1, 0).rotation.z = -1.22;
  mesh(new THREE.CylinderGeometry(0.07, 0.07, 0.16, 12), askMat, question, 0.05, -0.17, 0);
  mesh(sphere, askMat, question, 0.05, -0.42, 0, 0.085);

  const waves = group(-1.05, 1.4, 0.5);
  const arcGeom = new THREE.TorusGeometry(1, 0.05, 8, 24, PI / 2);
  const arcs = [0, 1, 2].map(() => {
    const m = M.solid({ unlit: 1, transparent: true });
    m.depthWrite = false;
    tints.push({ m, key: 'accent' });
    const o = mesh(arcGeom, m, waves);
    o.rotation.z = PI * 0.75;
    return o;
  });

  const heartShape = new THREE.Shape();
  heartShape.moveTo(0, -0.5);
  heartShape.bezierCurveTo(-0.95, 0.1, -0.5, 0.8, 0, 0.32);
  heartShape.bezierCurveTo(0.5, 0.8, 0.95, 0.1, 0, -0.5);
  const heart = group(0, 2.75, 0.5);
  mesh(new THREE.ShapeGeometry(heartShape, 20), mat('bad', flat), heart, 0, 0, 0, 0.42);

  const show = (g, s) => {
    g.visible = s > 0.012;
    if (g.visible) g.scale.setScalar(s);
  };

  return {
    /** Applies the resolved theme colours to every prop material. */
    setPalette(c) {
      for (const { m, key, rim } of tints) {
        setRgb(m.uniforms.uColor, c[key]);
        if (rim) setRgb(m.uniforms.uRim, c[rim], 0.35);
      }
    },
    /** Positions and scales every prop for pose `p` at time `t`. */
    update(p, view, t, dt) {
      // The send-button view is too small for props: the body carries each state.
      root.visible = !view.cfg.compact;
      if (!root.visible) return;

      show(bubble, p.bubble);
      if (bubble.visible) {
        bubble.position.y = 2.6 + 0.04 * sin(t * 1.7);
        dots.forEach((d, i) => { d.position.y = 0.07 * max(0, sin(t * 4.2 - i * 0.9)); });
      }

      zs.forEach((g, i) => {
        const ph = (t * 0.36 + i / 3) % 1;
        const env = smoothstep(0, 0.18, ph) * (1 - smoothstep(0.7, 1, ph));
        show(g, p.zzz * env * (0.16 + 0.14 * ph));
        g.position.set(0.95 + 0.55 * ph + 0.08 * sin(ph * 9), 2.0 + ph, 0.4);
        g.rotation.z = 0.2 * sin(ph * 6);
      });

      sparkles.forEach((g, i) => {
        const tw = 0.5 + 0.5 * sin(t * 5.2 + i * 1.9);
        show(g, p.sparkle * 0.2 * tw * tw);
        g.rotation.z = t * 1.5 + i;
      });

      dizzy.forEach((g, i) => {
        const a = t * 4.2 + i * 2.094;
        show(g, p.dizzy * 0.14);
        g.position.set(cos(a) * 0.72, 2.28 + 0.05 * sin(a * 2), 0.15 + sin(a) * 0.4);
        g.rotation.z = a;
      });

      const dp = (t * 0.7) % 1;
      show(drop, p.drop * smoothstep(0, 0.15, dp) * (1 - smoothstep(0.75, 1, dp)));
      drop.position.y = 1.95 - 0.32 * dp;

      const count = max(1, min(4, view.sim.subagents || 1));
      minis.forEach((g, i) => {
        view.miniW[i] = damp(view.miniW[i], i < count ? 1 : 0, 8, dt);
        const a = t * 1.1 + (i / count) * PI * 2;
        show(g, p.minis * view.miniW[i]);
        g.position.set(
          cos(a) * view.cfg.orbit,
          0.25 + 0.08 * abs(sin(t * 8 + i * 1.3)),
          sin(a) * view.cfg.orbit * 0.6,
        );
        g.rotation.y = Math.atan2(-sin(a), 0.6 * cos(a));
      });

      show(question, p.question * (1 + 0.06 * sin(t * 6)));
      question.position.y = 2.6 + 0.06 * sin(t * 3);
      question.rotation.z = 0.14 * sin(t * 3);

      waves.visible = p.waves > 0.012;
      arcs.forEach((o, i) => {
        const ph = 1 - ((t * 0.8 + i / 3) % 1);
        o.scale.setScalar(0.3 + 0.6 * ph);
        o.material.uniforms.uOpacity.value = p.waves * sin(PI * ph);
      });

      show(heart, p.heart);
      heart.position.set(0.12 * sin(p.heartT * 9), 2.3 + p.y + 0.55 * p.heartT, 0.5);
      heart.rotation.z = 0.2 * sin(p.heartT * 10);
    },
  };
}
