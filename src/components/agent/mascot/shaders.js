// GLSL + material factories for the 3D mascot. Colours are sRGB triplets shaded
// directly in display space, so theme tokens come out exactly as authored.

const FUR_VERT = /* glsl */ `
attribute vec4 aGroom;
uniform float uShells;
uniform float uFurLen;
uniform vec3 uForce;
varying float vLayer;
varying vec2 vUv;
varying float vAo;
varying vec3 vN;
varying vec3 vV;
varying vec3 vPos;
void main() {
  vLayer = float(gl_InstanceID) / max(1.0, uShells - 1.0);
  float len = uFurLen * aGroom.r;
  vec3 n = normalize(normal);
  vec3 p = position + n * (len * vLayer) + uForce * (len * vLayer * vLayer);
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  vUv = uv;
  vAo = aGroom.g;
  vPos = position;
  vN = normalize(normalMatrix * n);
  vV = -mv.xyz;
  gl_Position = projectionMatrix * mv;
}`;

const FUR_FRAG = /* glsl */ `
uniform float uDensity;
uniform vec3 uRoot;
uniform vec3 uTip;
uniform vec3 uRim;
uniform vec3 uLight;
uniform vec3 uBlushCol;
uniform float uBlush;
uniform float uDetail;
varying float vLayer;
varying vec2 vUv;
varying float vAo;
varying vec3 vN;
varying vec3 vV;
varying vec3 vPos;
float hash(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}
void main() {
  // Each cell owns one strand with a random root, length and curl; the nearest
  // of the 3×3 neighbours wins, so no grid rows show through.
  vec2 g = vUv * uDensity;
  vec2 cell = floor(g);
  vec2 f = fract(g);
  float cover = 0.0;
  float r = 0.5;
  for (int j = -1; j <= 1; j++) {
    for (int i = -1; i <= 1; i++) {
      vec2 o = vec2(float(i), float(j));
      vec2 id = cell + o;
      float rr = hash(id);
      float tt = vLayer / (0.55 + 0.45 * rr);
      vec2 curl = (vec2(hash(id + 9.7), hash(id + 4.3)) - 0.5) * 1.2 * vLayer * vLayer;
      vec2 c = o + 0.15 + 0.7 * vec2(hash(id + 7.1), hash(id + 3.7)) + curl;
      float rad = 0.66 * (1.0 - 0.72 * tt * tt);
      float a = (1.0 - smoothstep(rad * 0.5, rad, length(f - c))) * step(tt, 1.0);
      if (a > cover) { cover = a; r = rr; }
    }
  }
  // Surface-anchored dither turns the soft strand edge into coverage; the
  // supersampled blit averages it back to a smooth falloff. Low-detail views
  // lean on a fixed threshold instead, which gives clean tufts without noise.
  if (vLayer > 0.0 && cover <= mix(0.35, hash(floor(g * 5.0) + vLayer * 31.7), uDetail)) discard;
  vec3 n = normalize(vN);
  vec3 v = normalize(vV);
  float ndl = dot(n, uLight) * 0.5 + 0.5;
  float shade = mix(0.64, 1.0, ndl);
  // uDetail fades the strand-to-strand contrast so small views shade smoothly.
  float depth = mix(1.0, mix(0.62, 1.0, smoothstep(0.0, 0.75, vLayer)), uDetail);
  vec3 base = mix(uRoot, uTip, mix(1.0, vLayer, uDetail));
  // Cheeks sit at (±0.66, -0.2) on the front of the body, in body space.
  vec2 cheek = (vec2(abs(vPos.x), vPos.y) - vec2(0.66, -0.2)) * vec2(1.0, 1.5);
  float blush = uBlush * (1.0 - smoothstep(0.06, 0.24, length(cheek))) * step(0.0, vPos.z);
  base = mix(base, uBlushCol, blush * 0.75);
  vec3 col = base * shade * depth * vAo * mix(1.0, 0.94 + 0.12 * r, uDetail);
  float rim = pow(1.0 - max(dot(n, v), 0.0), 2.5);
  col += uRim * rim * (0.3 + 0.7 * vLayer);
  gl_FragColor = vec4(col, 1.0);
}`;

const SOLID_VERT = /* glsl */ `
varying vec3 vN;
varying vec3 vV;
void main() {
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  vN = normalize(normalMatrix * normal);
  vV = -mv.xyz;
  gl_Position = projectionMatrix * mv;
}`;

const SOLID_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform vec3 uRim;
uniform vec3 uLight;
uniform float uUnlit;
uniform float uGloss;
uniform float uOpacity;
varying vec3 vN;
varying vec3 vV;
void main() {
  vec3 n = normalize(vN);
  vec3 v = normalize(vV);
  float ndl = dot(n, uLight) * 0.5 + 0.5;
  float shade = mix(mix(0.6, 1.0, ndl), 1.0, uUnlit);
  float spec = pow(max(dot(n, normalize(uLight + v)), 0.0), 48.0) * uGloss;
  float fres = pow(1.0 - max(dot(n, v), 0.0), 3.0);
  gl_FragColor = vec4(uColor * shade + vec3(spec) + uRim * fres, uOpacity);
}`;

const BLOB_VERT = /* glsl */ `
varying vec2 vUv;
void main() {
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}`;

const BLOB_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform float uOpacity;
varying vec2 vUv;
void main() {
  float a = 1.0 - smoothstep(0.0, 1.0, length(vUv - 0.5) * 2.0);
  gl_FragColor = vec4(uColor, a * a * uOpacity);
}`;

/** Copies an [r, g, b] triplet (optionally scaled) into a vec3 uniform. */
export function setRgb(uniform, rgb, mul = 1) {
  uniform.value.set(rgb[0] * mul, rgb[1] * mul, rgb[2] * mul);
}

/** Builds the mascot's material factories around one shared key-light direction. */
export function createMaterials(THREE) {
  const light = new THREE.Vector3(-0.42, 0.72, 0.55).normalize();
  const vec = () => new THREE.Vector3(1, 1, 1);
  return {
    /** Shell fur: drawn as `shells` instances, each pushed further along the normal. */
    fur({ shells, length, density }) {
      return new THREE.ShaderMaterial({
        uniforms: {
          uShells: { value: shells },
          uFurLen: { value: length },
          uDensity: { value: density },
          uForce: { value: new THREE.Vector3(0, -0.5, 0) },
          uRoot: { value: vec() },
          uTip: { value: vec() },
          uRim: { value: new THREE.Vector3(0, 0, 0) },
          uLight: { value: light },
          uBlushCol: { value: vec() },
          uBlush: { value: 0 },
          uDetail: { value: 1 },
        },
        vertexShader: FUR_VERT,
        fragmentShader: FUR_FRAG,
      });
    },
    /** Smooth lit surface for eyes, laptop and props. */
    solid({ gloss = 0, unlit = 0, transparent = false, side = THREE.FrontSide } = {}) {
      return new THREE.ShaderMaterial({
        uniforms: {
          uColor: { value: vec() },
          uRim: { value: new THREE.Vector3(0, 0, 0) },
          uLight: { value: light },
          uUnlit: { value: unlit },
          uGloss: { value: gloss },
          uOpacity: { value: 1 },
        },
        vertexShader: SOLID_VERT,
        fragmentShader: SOLID_FRAG,
        transparent,
        side,
      });
    },
    /** Soft radial disc for the ground shadow. */
    blob() {
      return new THREE.ShaderMaterial({
        uniforms: { uColor: { value: new THREE.Vector3(0, 0, 0) }, uOpacity: { value: 0 } },
        vertexShader: BLOB_VERT,
        fragmentShader: BLOB_FRAG,
        transparent: true,
        depthWrite: false,
      });
    },
  };
}
