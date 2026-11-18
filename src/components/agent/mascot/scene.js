// WebGL engine for the 3D mascot. One shared renderer and one copy of the model
// serve every mascot on screen: each view poses the model, renders it into the
// shared GL canvas and blits the result into its own 2D canvas, so the app never
// holds more than one WebGL context for mascots.
import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import glbUrl from '@/assets/mascot/mascot.glb?url';
import { createMaterials, setRgb } from './shaders';
import { buildProps } from './props';
import { createRig, stepRig, damp } from './rig';
import { resolvePalette } from './palette';
import { VIEWS } from './views';

const FUR = {
  Body: { length: 0.16, density: 42 },
  ArmL: { length: 0.11, density: 12 },
  ArmR: { length: 0.11, density: 12 },
  FootL: { length: 0.08, density: 10 },
  FootR: { length: 0.08, density: 10 },
};
const BASE_Y = 0.08;
const BODY_HALF = 0.8;
const FOV = 22;
const ELEVATION = 0.12;
const AXIS_Y = new THREE.Vector3(0, 1, 0);
const AXIS_Z = new THREE.Vector3(0, 0, 1);
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

/** Loads the model and builds the shared scene; rejects when WebGL is unavailable. */
async function createEngine() {
  const glCanvas = document.createElement('canvas');
  const renderer = new THREE.WebGLRenderer({
    canvas: glCanvas, antialias: true, alpha: true, powerPreference: 'low-power',
  });
  renderer.setPixelRatio(1);
  renderer.autoClear = false;
  renderer.setClearColor(0x000000, 0);
  renderer.setScissorTest(true);
  glCanvas.addEventListener('webglcontextlost', (e) => e.preventDefault());

  const gltf = await new GLTFLoader().loadAsync(glbUrl);
  const M = createMaterials(THREE);
  const scene = new THREE.Scene();
  scene.add(gltf.scene);
  const node = (name) => {
    const o = gltf.scene.getObjectByName(name);
    if (!o) throw new Error(`mascot.glb is missing node ${name}`);
    return o;
  };

  const furs = Object.entries(FUR).map(([name, cfg]) => {
    const mesh = node(name);
    const src = mesh.geometry;
    const geom = new THREE.InstancedBufferGeometry();
    geom.setIndex(src.index);
    geom.setAttribute('position', src.attributes.position);
    geom.setAttribute('normal', src.attributes.normal);
    geom.setAttribute('uv', src.attributes.uv);
    geom.setAttribute('aGroom', src.attributes.color);
    mesh.geometry = geom;
    mesh.material = M.fur({ shells: 1, length: cfg.length, density: cfg.density });
    mesh.userData.furDensity = cfg.density;
    mesh.frustumCulled = false;
    return mesh;
  });
  const bodyFur = furs[0].material.uniforms;

  const tints = [];
  const solid = (name, key, opts, rim) => {
    const mesh = node(name);
    mesh.material = M.solid(opts);
    mesh.frustumCulled = false;
    tints.push({ m: mesh.material, key, rim });
    return mesh;
  };
  const side = (s) => ({
    eye: solid(`Eye${s}`, 'eye', { gloss: 0.35 }),
    pupil: solid(`Pupil${s}`, 'pupil', { gloss: 0.9 }),
    glint: solid(`Glint${s}`, 'glint', { unlit: 1 }),
    glintSmall: solid(`GlintSmall${s}`, 'glint', { unlit: 1 }),
    happy: solid(`EyeHappy${s}`, 'eye', { unlit: 0.6 }),
    cross: solid(`EyeCross${s}`, 'eye', { unlit: 0.6 }),
    arm: node(`Arm${s}`),
    foot: node(`Foot${s}`),
  });
  const L = side('L');
  const R = side('R');
  for (const e of [L, R]) {
    e.happy.translateZ(0.035);
    e.cross.translateZ(0.035);
    for (const k of ['pupil', 'glint', 'glintSmall']) e[`${k}Rest`] = e[k].position.clone();
  }

  const body = node('Body');
  const tongue = solid('Tongue', 'blush', { unlit: 0.4 });
  const laptop = node('Laptop');
  solid('LaptopBase', 'lidEdge');
  solid('LaptopLid', 'lid', { gloss: 0.15 });
  const logo = solid('LaptopLogo', 'accent', { unlit: 1 });

  // Mouth shapes are blended on the CPU (232 vertices): keeps the shaders free
  // of three's morph-target plumbing.
  const mouth = solid('Mouth', 'eye', { unlit: 0.6 });
  mouth.translateZ(0.012);
  const mouthRestY = mouth.position.y;
  const mouthPos = mouth.geometry.attributes.position;
  const mouthBase = Float32Array.from(mouthPos.array);
  const mouthKeys = mouth.userData.targetNames || Object.keys(mouth.morphTargetDictionary || {});
  const mouthDeltas = {};
  (mouth.geometry.morphAttributes.position || []).forEach((attr, i) => {
    const d = new Float32Array(attr.count * 3);
    for (let v = 0; v < attr.count; v++) {
      d[v * 3] = attr.getX(v);
      d[v * 3 + 1] = attr.getY(v);
      d[v * 3 + 2] = attr.getZ(v);
    }
    mouthDeltas[String(mouthKeys[i]).toLowerCase()] = d;
  });
  mouth.geometry.morphAttributes = {};
  mouth.updateMorphTargets();
  let mouthSig = '';

  const disc = new THREE.PlaneGeometry(1, 1);
  const shadow = new THREE.Mesh(disc, M.blob());
  shadow.rotation.x = -Math.PI / 2;
  shadow.position.set(0, 0.004, 0.08);
  shadow.renderOrder = -1;
  scene.add(shadow);
  const props = buildProps(THREE, M, scene);
  const qy = new THREE.Quaternion();
  const qz = new THREE.Quaternion();

  /** Writes the view's theme colours into every material. */
  function applyPalette(view) {
    const c = view.palette.colors;
    for (const mesh of furs) {
      const u = mesh.material.uniforms;
      setRgb(u.uRoot, c.furRoot);
      setRgb(u.uTip, c.furTip);
      setRgb(u.uRim, c.rim, view.palette.mode === 'dark' ? 0.22 : 0.3);
      setRgb(u.uBlushCol, c.blush);
      u.uShells.value = view.cfg.shells;
      u.uDensity.value = mesh.userData.furDensity * view.cfg.density;
      u.uDetail.value = view.cfg.detail;
      mesh.geometry.instanceCount = view.cfg.shells;
    }
    for (const { m, key, rim } of tints) {
      setRgb(m.uniforms.uColor, c[key]);
      if (rim) setRgb(m.uniforms.uRim, c[rim], 0.3);
    }
    setRgb(shadow.material.uniforms.uColor, c.shadow);
    props.setPalette(c);
  }

  /** Poses one arm: splay about Z, then swing toward the front about Y. */
  function poseArm(arm, sign, up, fwd, rest) {
    qz.setFromAxisAngle(AXIS_Z, sign * (rest + up));
    qy.setFromAxisAngle(AXIS_Y, -sign * fwd);
    arm.quaternion.copy(qy).multiply(qz);
  }

  /** Maps a blended pose onto the model's nodes and materials. */
  function applyPose(p, view, t, dt) {
    const c = view.palette.colors;
    const cfg = view.cfg;
    const fs = cfg.face;
    const sy = clamp(p.sy, 0.6, 1.4);
    const sxz = 1 / Math.sqrt(sy);
    const y = BASE_Y + BODY_HALF * sy + p.y + p.bob;
    body.scale.set(sxz, sy, sxz);
    body.position.set(p.x, y, 0);
    body.rotation.set(p.pitch + p.lookY * cfg.headPitch, p.yaw + p.lookX * cfg.headYaw, p.roll);

    const vy = dt > 0 && view.prevY != null ? (y - view.prevY) / dt : 0;
    view.prevY = y;
    view.furVy = damp(view.furVy, vy, 12, Math.min(dt, 0.05));
    const force = -0.45 - clamp(view.furVy * 0.3, -0.6, 0.6);
    for (const mesh of furs) mesh.material.uniforms.uForce.value.set(0, force, 0);
    bodyFur.uBlush.value = p.blush;

    const air = clamp(p.y / 0.6, 0, 1);
    L.foot.position.y = p.y;
    R.foot.position.y = p.y;
    L.foot.rotation.x = -0.5 * p.footL + 0.5 * air;
    R.foot.rotation.x = -0.5 * p.footR + 0.5 * air;
    shadow.scale.set(2.9 * (1 - 0.35 * air), 1.5 * (1 - 0.35 * air), 1);
    shadow.material.uniforms.uOpacity.value = cfg.compact ? 0 : view.palette.shadowOpacity * (1 - 0.5 * air);

    poseArm(L.arm, -1, p.armLUp, p.armLFwd, cfg.armRest);
    poseArm(R.arm, 1, p.armRUp, p.armRFwd, cfg.armRest);

    const shut = Math.max(p.eyeHappy, p.eyeCross);
    const base = 1 - shut;
    const open = Math.max(cfg.eyeMin, p.eyeOpen);
    const wide = 1 + 0.16 * p.eyeWide;
    const lx = clamp(p.lookX, -1, 1);
    const ly = clamp(p.lookY, -1, 1);
    for (const e of [L, R]) {
      e.eye.visible = base > 0.02;
      e.eye.scale.set(base * wide * fs, base * wide * open * fs, base * fs);
      e.pupil.position.set(e.pupilRest.x + lx * 0.05, e.pupilRest.y - ly * 0.06, e.pupilRest.z);
      e.glint.position.set(e.glintRest.x + lx * 0.03, e.glintRest.y - ly * 0.035, e.glintRest.z);
      e.glintSmall.position.set(e.glintSmallRest.x + lx * 0.03, e.glintSmallRest.y - ly * 0.035, e.glintSmallRest.z);
      e.glint.visible = e.glintSmall.visible = open > 0.35;
      e.happy.visible = p.eyeHappy > 0.02;
      e.happy.scale.setScalar(p.eyeHappy * fs);
      e.cross.visible = p.eyeCross > 0.02;
      e.cross.scale.setScalar(p.eyeCross * fs);
    }

    const sig = `${p.smile.toFixed(3)},${p.flat.toFixed(3)},${p.wavy.toFixed(3)},${p.o.toFixed(3)}`;
    if (sig !== mouthSig) {
      mouthSig = sig;
      const out = mouthPos.array;
      out.set(mouthBase);
      for (const key of ['smile', 'flat', 'wavy', 'o']) {
        const w = p[key];
        const d = mouthDeltas[key];
        if (!d || w < 0.001) continue;
        for (let i = 0; i < out.length; i++) out[i] += w * d[i];
      }
      mouthPos.needsUpdate = true;
    }
    mouth.scale.setScalar(cfg.mouth);
    mouth.position.y = mouthRestY - cfg.mouthDrop;
    tongue.visible = p.tongue > 0.02;
    tongue.scale.setScalar(p.tongue);

    laptop.visible = p.laptop > 0.02;
    laptop.scale.setScalar(p.laptop * cfg.laptop);
    const blink = 0.5 + 0.5 * Math.sin(t * 6);
    logo.material.uniforms.uColor.value.set(
      c.accent[0] + (c.accentHi[0] - c.accent[0]) * blink,
      c.accent[1] + (c.accentHi[1] - c.accent[1]) * blink,
      c.accent[2] + (c.accentHi[2] - c.accent[2]) * blink,
    );
  }

  const views = new Set();
  let raf = 0;

  /** Renders one view at time `t` (seconds) and blits it into the view's canvas. */
  function render(view, t) {
    const first = view.lastT == null;
    const dt = first || view.reduced ? 1 : clamp(t - view.lastT, 0, 0.05);
    view.lastT = t;
    view.onTick?.(t);
    const p = stepRig(view.rig, view.sim, t, dt, view.cfg.compact);
    applyPalette(view);
    applyPose(p, view, t, first ? 0 : dt);
    props.update(p, view, t, dt);

    if (glCanvas.width < view.bw || glCanvas.height < view.bh) {
      renderer.setSize(Math.max(glCanvas.width, view.bw), Math.max(glCanvas.height, view.bh), false);
    }
    renderer.setViewport(0, 0, view.bw, view.bh);
    renderer.setScissor(0, 0, view.bw, view.bh);
    renderer.clear();
    renderer.render(scene, view.camera);
    const { canvas, ctx } = view;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(glCanvas, 0, glCanvas.height - view.bh, view.bw, view.bh, 0, 0, canvas.width, canvas.height);
  }

  const frame = (ms) => {
    raf = 0;
    let live = false;
    for (const view of views) {
      if (view.manual || view.reduced || !view.visible) continue;
      live = true;
      render(view, ms / 1000);
    }
    if (live) raf = requestAnimationFrame(frame);
  };
  const kick = () => { if (!raf) raf = requestAnimationFrame(frame); };

  /** Sizes a view's backing stores for the current device pixel ratio. */
  function sizeView(view) {
    const pr = view.pixelRatio || window.devicePixelRatio || 1;
    view.canvas.width = Math.round(view.cfg.w * pr);
    view.canvas.height = Math.round(view.cfg.h * pr);
    view.bw = Math.round(view.canvas.width * view.cfg.ss);
    view.bh = Math.round(view.canvas.height * view.cfg.ss);
    view.ctx.imageSmoothingEnabled = true;
    view.ctx.imageSmoothingQuality = 'high';
  }

  const repaintStatic = () => {
    for (const view of views) {
      view.palette = resolvePalette(view.host);
      if (view.reduced) render(view, view.sim.stateStart + 3);
    }
  };
  new MutationObserver(repaintStatic).observe(document.documentElement, {
    attributes: true, attributeFilter: ['class', 'style', 'data-theme'],
  });
  window.addEventListener('resize', () => {
    for (const view of views) {
      sizeView(view);
      if (view.reduced) render(view, view.sim.stateStart + 3);
    }
  });

  return {
    /**
     * Attaches a mascot to a 2D `canvas`. `sim` is the caller-owned run state;
     * `manual` views are only drawn through `render(t)` (used for capture).
     */
    createView({ canvas, size, host, sim, onTick, reduced = false, manual = false, pixelRatio = 0 }) {
      const cfg = VIEWS[size] || VIEWS.hero;
      const aspect = cfg.w / cfg.h;
      const dist = cfg.worldW / aspect / (2 * Math.tan((FOV * Math.PI) / 360));
      const camera = new THREE.PerspectiveCamera(FOV, aspect, 0.1, 100);
      camera.position.set(0, cfg.cy + dist * Math.sin(ELEVATION), dist * Math.cos(ELEVATION));
      camera.lookAt(0, cfg.cy, 0);
      const view = {
        canvas, cfg, sim, onTick, reduced, manual, pixelRatio, camera,
        host: host || document.body,
        ctx: canvas.getContext('2d'),
        rig: createRig(),
        palette: resolvePalette(host || document.body),
        miniW: [0, 0, 0, 0],
        furVy: 0,
        prevY: null,
        lastT: null,
        visible: true,
      };
      sizeView(view);
      views.add(view);
      let io = null;
      if (!manual) {
        io = new IntersectionObserver(([entry]) => {
          view.visible = entry.isIntersecting;
          if (view.visible) kick();
        });
        io.observe(canvas);
        render(view, reduced ? sim.stateStart + 3 : performance.now() / 1000);
        if (!reduced) kick();
      }
      return {
        render: (t) => render(view, t),
        /** Redraws a reduced-motion view after its state changed. */
        invalidate: () => { if (view.reduced) render(view, view.sim.stateStart + 3); },
        dispose: () => {
          io?.disconnect();
          views.delete(view);
        },
      };
    },
    info: () => {
      const gl = renderer.getContext();
      const ext = gl.getExtension('WEBGL_debug_renderer_info');
      return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    },
  };
}

let enginePromise = null;

/** Returns the shared mascot engine, creating it on first use. */
export function getMascotEngine() {
  if (!enginePromise) enginePromise = createEngine();
  return enginePromise;
}
