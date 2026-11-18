// Framing per mascot size. `w`/`h` are CSS pixels; `worldW` is how many scene
// units span the width (the character is ~2 units wide), `cy` the scene height
// at the vertical centre. Shared by the React component and the WebGL scene.
export const VIEWS = {
  hero: {
    w: 240, h: 176, worldW: 4.8, cy: 1.42, shells: 30, density: 1, detail: 1, ss: 2,
    face: 1, mouth: 1, mouthDrop: 0, eyeMin: 0.07, armRest: 0.733,
    headYaw: 0.16, headPitch: 0.07, laptop: 1, compact: false, orbit: 1.9,
  },
  // The send button is ~28 px wide: fur shades smoothly (`detail`), the face is
  // enlarged (`face`), the head turns further with the gaze, and small props are
  // dropped (`compact`) so each state reads from the body alone. The canvas is
  // wider than the 32 px ring so sways and raised arms overflow instead of clipping.
  ring: {
    w: 36, h: 32, worldW: 3.857, cy: 1.43, shells: 14, density: 0.42, detail: 0.3, ss: 3,
    armRest: 0.42,
    face: 1.35, mouth: 1.6, mouthDrop: 0.07, eyeMin: 0.24,
    headYaw: 0.42, headPitch: 0.2, laptop: 0.85, compact: true,
  },
};
VIEWS.mini = VIEWS.ring;
