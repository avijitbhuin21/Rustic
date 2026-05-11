---
name: website-planner-workflow
description: Plan and scaffold a scroll-driven website whose background is an animated video, generated from a URL, BRD, or idea. Foreground content layers on top, themed to match. Iterates on design with the user before any media generation.
---

# Website planner workflow

## Purpose
Take a website idea (URL, BRD, or short description), agree a design with the user,
generate a theme-matching animated background (image → video → frame sequence), and
scaffold a scroll-driven site where the foreground content lives on top of the
scrubbing background.

The workflow keeps **all creative and structural decisions with the user** — number
of sections, tech stack, output location, single-vs-per-section video, palette
overrides. Nothing is hard-coded here.

## Invocation
Run with one of:
- A URL → `Run website-planner-workflow on https://example.com/`
- A short idea → `Run website-planner-workflow: "a serene garden-to-meadow scroll for my meditation app"`
- A BRD path → `Run website-planner-workflow on docs/brd.md`

If no input is provided, **ask the user** which of the three they have.

---

## Stage 0 — Preflight: tool availability

Before anything else, verify the agent has the media tools it needs. If any are
missing, **stop and ask the user** — don't try to substitute silently.

### 0.1 Required tools
Check the tool registry for:
- `image_create` — generates the background still(s)
- `animate` (or `video_create` for fully synthetic clips) — turns each still into a short clip
- A way to extract frames from video — typically `ffmpeg` on PATH

### 0.2 Locate ffmpeg (two-step probe)
A direct invocation can fail even when ffmpeg is installed — e.g. it's on PATH for
the user's interactive shell but not for the shell the agent spawns. So probe in
two steps before declaring it missing:

1. Try the direct call:
   ```powershell
   ffmpeg -version
   ```
2. If that fails, look it up on PATH explicitly:
   ```powershell
   where.exe ffmpeg          # Windows
   which ffmpeg              # macOS / Linux
   ```
   If `where`/`which` returns a path, use that absolute path for every ffmpeg
   invocation in Stage 7 instead of the bare `ffmpeg` command, and record the
   resolved path in `PLAN.md` so later stages reuse it.

Only if both probes fail is ffmpeg actually missing — then go to 0.3.

### 0.3 If something is missing
Tell the user which tool is missing and offer:
1. **Configure the provider** in Rustic settings → AI Providers (for `image_create` / `animate` / `video_create`).
2. **Skip generation for that step** and let the user supply the asset locally:
   - For backgrounds: ask for a path to an image file.
   - For the animated clip: ask for a path to a video file.
3. **Install ffmpeg** if frame extraction is missing. Ask the user first — don't
   install silently. If they agree:
   - `winget install Gyan.FFmpeg` (Windows)
   - `brew install ffmpeg` (macOS)
   - `apt install ffmpeg` (Linux)

   After install, **re-run the 0.2 two-step probe** before continuing — the new
   binary may not be on PATH for the current shell.
4. **Skip ffmpeg entirely and write custom extraction code.** Possible but
   significantly more work — e.g. a tiny Node script using `@ffmpeg/ffmpeg`
   (WASM build of ffmpeg, ~30 MB) or a frame-by-frame `<canvas>`-grab from a
   hidden `<video>` element. Only take this branch if the user explicitly asks
   for it; warn them it's slower and adds a dependency.

Do not proceed past Stage 0 with an unresolved gap.

---

## Stage 1 — Gather requirements

The goal of this stage is to understand **what** to build, **where** it lives, and
**how** it should look. Ask, don't assume.

### 1.1 Project brief
Determine what the user gave you:
- **URL** → proceed to Stage 2 for theme extraction.
- **BRD / requirements doc** → read it; summarise the product, audience, tone, must-have sections.
- **Short idea / name** → ask the user to expand on: product, audience, tone, key sections, any reference sites.

### 1.2 Preferences up-front
Ask the user explicitly (one message, batched):
- **Theme / mood preferences** — colors, typography, references, brand assets they want kept.
- **Output location** — which folder under the project should the generated site go in? (Default suggestion: `website/` — but **wait for the user's choice**.)
- **Project structure** — flat? `src/` + `public/` layout? A starter template? Let the user describe it; do not impose one.
- **Tech stack** — vanilla HTML/CSS/JS? React + Vite? Astro? Next? Something else? Wait for the user's pick.
- **Video strategy** — one continuous video that spans the whole scroll, or one short clip per section? Brief tradeoff: continuous = single cinematic arc but harder to tweak; per-section = easy to regenerate one segment, smoother per-segment scrub.

Record all answers in `<output>/PLAN.md` as the running source of truth.

---

## Stage 2 — Theme extraction (only if a URL was given)

If the input was a URL, fetch the site and pull out theme cues. The goal is *not*
to clone structure — only to learn the visual language.

### 2.1 Fetch
Use the agent's web-fetch / browser tooling to retrieve the page HTML + a screenshot.

### 2.2 Extract
From the response, capture:
- **Palette** — pull the 5–8 most-used colors from the screenshot (k-means or a quantizer).
- **Typography** — read `font-family` declarations from the CSS; record headline vs. body fonts.
- **Section rhythm** — note how the live site breaks up content (hero, features, etc.) as a *hint*, not a target.
- **Tone of copy** — formal/playful/technical — short note.

Write these into `<output>/PLAN.md` under a `## Theme cues` heading. If the user
gave a manual palette/typography preference in Stage 1.2, the user's choice wins.
If neither source provides one, the agent picks and explicitly notes its choice in
the plan for user approval.

---

## Stage 3 — Design pass

This is where the agent commits to a concrete design **before** spending any time
or budget on image/video generation. Everything here is text.

### 3.1 Section breakdown
Decide how many sections the site needs, based on the brief — do not default. For
each section, write into `<output>/PLAN.md`:
- **Purpose** (e.g., "hero — establish the calm-garden mood, single CTA")
- **Copy** — headline, sub, body, CTA labels (draft text, not lorem)
- **Visual direction** — what the background image should show for this section
- **Background prompt** — the exact prompt that will be sent to `image_create`
- **Animation prompt** — the exact prompt that will be sent to `animate` (e.g., "slow pan left to right, gentle wind in grass, 4 seconds")
- **Foreground palette** — text/UI colors chosen to contrast against the background palette while staying on-theme. Run a WCAG AA contrast check (≥4.5:1 for body, ≥3:1 for large text); if it fails, pick a different shade.

### 3.2 Transition strategy (per-section videos only)
If the user chose per-section videos in Stage 1.2, plan each section's first/last
frame so adjacent sections hand off cleanly — section N's last visual state should
roughly match section N+1's first visual state. Note this in the prompts.

### 3.3 Scroll mechanics
Document in `PLAN.md`:
- Each section spans roughly one viewport in height multiplied by a scroll-depth
  factor (e.g., 2.0 = user scrolls two screens to play the full clip). The factor
  is per-section; let the agent pick a sensible default per section's complexity.
- Total page height = sum of section heights.

---

## Stage 4 — Design review checkpoint (HARD GATE)

**Stop. Do not generate any images or videos yet.**

Present the full `PLAN.md` to the user and ask:

> "Here is the proposed plan: sections, copy, palette, prompts, and scroll mechanics.
> Image and video generation is slow and consumes credits, so I want to lock this in
> before we run it.
>
> What would you like to change? Reply 'approved' when you're happy with everything."

Iterate freely at this stage — edit text, swap colors, rewrite prompts, add or
remove sections. Re-show the diff each round. **Only proceed to Stage 5 once the
user explicitly approves.**

---

## Stage 5 — Background image generation

For each section, call `image_create` with the locked prompt. Save outputs to:
```
<output>/assets/backgrounds/section-<N>.png
```

After generation, show the user the resulting images and ask:
> "Are these backgrounds good, or should any be regenerated?"

Regenerate per the user's feedback before moving on. Loop until they approve.

If `image_create` is unavailable and the user is supplying their own images, ask
for the file paths now and copy them into `<output>/assets/backgrounds/`.

---

## Stage 6 — Animate backgrounds to clips

For each approved still, call `animate` with the locked animation prompt to produce
a short clip (typical length: 3–6 seconds). Save to:
```
<output>/assets/clips/section-<N>.mp4
```

If the user chose **one continuous video** in Stage 1.2, animate the hero still
into a longer scene that morphs through the planned moods, or use `video_create`
directly with a multi-beat prompt — whichever the available tool supports better.
Save as `<output>/assets/clips/main.mp4`.

If `animate` is unavailable, fall back to a still background for that section
(noted in `PLAN.md`) or accept a user-supplied clip path.

Preview each clip with the user and accept regen requests before moving on.

---

## Stage 7 — Frame extraction (for buttery scroll scrubbing)

The site uses a **frame-sequence canvas scrubber**, not the `<video>` element. This
gives uniform smoothness across browsers without codec-seek stutter.

### 7.1 Pick frame count
Default to **30 fps × clip-duration**, capped at **120 frames per clip**. The cap
keeps total asset size sane; if a section's clip is longer, drop the effective fps
proportionally rather than raising the cap.

### 7.2 Extract with ffmpeg
For each clip:
```powershell
ffmpeg -i <output>/assets/clips/section-<N>.mp4 `
       -vf "fps=<FPS>,scale=1920:-2:flags=lanczos" `
       -q:v 75 `
       <output>/assets/frames/section-<N>/frame-%04d.webp
```
Use **WebP** at q=75 for ~30–40% smaller frames than JPEG at similar quality. If
the project targets older browsers, switch to JPEG.

### 7.3 Record manifest
Write `<output>/assets/frames/manifest.json`:
```json
{
  "sections": [
    {
      "id": 1,
      "frames": 96,
      "pattern": "section-1/frame-%04d.webp",
      "scrollDepth": 2.0
    }
  ]
}
```
The site loader reads this at runtime — adding or regenerating a section means
re-writing the manifest, not editing JS.

---

## Stage 8 — Scaffold the site (user's chosen tech stack)

Generate the site at the path the user chose in Stage 1.2, in the structure they
asked for, using the tech stack they picked. Do **not** invent layout choices —
mirror what they described.

Whatever the stack, three things must be true of the generated site:
1. It can serve the static `assets/frames/` folder.
2. It loads `manifest.json` at startup.
3. It implements the scroll scrubber from Stage 9.

If the user gave only a vague stack ("just plain HTML"), confirm a minimal
structure with them before writing files.

---

## Stage 9 — Scroll-driven frame scrubber

This is the runtime piece that makes the background animate smoothly on scroll.
The pattern below is **reference, not prescription** — adapt it to the user's
chosen stack/framework idioms.

### 9.1 Core idea
- One full-viewport `<canvas>` fixed behind the content.
- On load, preload all frames for the current and next section as `Image` objects.
- On `scroll`, compute which section is on-screen and what fraction of its scroll
  depth has been consumed. Map that to a frame index and draw it. **No** timers,
  **no** `requestAnimationFrame` loop except for the single redraw per scroll
  event (rAF-throttled). When the user stops, drawing stops — exactly the
  "doesn't continuously play" behaviour requested.

### 9.2 Reference implementation (vanilla; adapt as needed)
```javascript
// background-scroll.js — drop-in for vanilla; port the same logic to React/Vue/etc.
const canvas = document.querySelector('#bg-canvas');
const ctx = canvas.getContext('2d', { alpha: false });
const sections = Array.from(document.querySelectorAll('[data-section]'));

let manifest, frameCache = new Map(), currentSection = -1, pending = false;

async function init() {
  manifest = await fetch('/assets/frames/manifest.json').then(r => r.json());
  resize();
  window.addEventListener('resize', resize);
  window.addEventListener('scroll', onScroll, { passive: true });
  await preloadSection(0);
  draw();
}

function resize() {
  canvas.width = innerWidth * devicePixelRatio;
  canvas.height = innerHeight * devicePixelRatio;
  canvas.style.width = innerWidth + 'px';
  canvas.style.height = innerHeight + 'px';
}

function frameUrl(section, idx) {
  return '/assets/frames/' + section.pattern.replace('%04d', String(idx).padStart(4, '0'));
}

function loadFrame(section, idx) {
  const key = section.id + ':' + idx;
  if (frameCache.has(key)) return frameCache.get(key);
  const img = new Image();
  img.src = frameUrl(section, idx);
  const p = new Promise(res => { img.onload = () => res(img); img.onerror = () => res(null); });
  frameCache.set(key, p);
  return p;
}

async function preloadSection(i) {
  const s = manifest.sections[i]; if (!s) return;
  for (let f = 0; f < s.frames; f++) loadFrame(s, f);
}

function onScroll() {
  if (pending) return;
  pending = true;
  requestAnimationFrame(() => { pending = false; draw(); });
}

async function draw() {
  // Which section is centred on screen?
  const mid = scrollY + innerHeight / 2;
  let active = 0;
  for (let i = 0; i < sections.length; i++) {
    const top = sections[i].offsetTop, h = sections[i].offsetHeight;
    if (mid >= top && mid < top + h) { active = i; break; }
  }
  const s = manifest.sections[active];
  if (active !== currentSection) {
    currentSection = active;
    preloadSection(active + 1);   // peek ahead
  }
  // Fraction through this section [0..1)
  const top = sections[active].offsetTop, h = sections[active].offsetHeight;
  const t = Math.min(0.999, Math.max(0, (scrollY + innerHeight / 2 - top) / h));
  const idx = Math.min(s.frames - 1, Math.floor(t * s.frames));
  const img = await loadFrame(s, idx);
  if (!img) return;
  // cover-fit
  const cw = canvas.width, ch = canvas.height;
  const ir = img.width / img.height, cr = cw / ch;
  let dw, dh, dx, dy;
  if (ir > cr) { dh = ch; dw = dh * ir; dx = (cw - dw) / 2; dy = 0; }
  else        { dw = cw; dh = dw / ir; dx = 0; dy = (ch - dh) / 2; }
  ctx.drawImage(img, dx, dy, dw, dh);
}

init();
```

### 9.3 HTML structure
```html
<canvas id="bg-canvas" style="position:fixed;inset:0;z-index:-1"></canvas>
<main>
  <section data-section="1" style="min-height:200vh">…content…</section>
  <section data-section="2" style="min-height:200vh">…content…</section>
</main>
```
`min-height` is `scrollDepth` × 100vh per the manifest.

### 9.4 Single-continuous-video variant
If the user picked one continuous video instead of per-section, the manifest has
one big `frames` array and the section loop above collapses to a single mapping:
fraction-of-total-page-scrolled → frame index. The rest is identical.

---

## Stage 10 — Foreground content

Layer the planned per-section copy and UI on top of the canvas. Use the
foreground palette decided in Stage 3 and re-checked in Stage 5. Each section's
content goes inside its `<section data-section="N">`.

Keep the foreground **lightweight** — no heavy frameworks unless the user's chosen
stack already brings them. The cost budget of this page belongs to the frame
sequence.

---

## Stage 11 — Local serve & verify loop

### 11.1 Serve
Whatever the stack:
- Vanilla / Astro / Vite / Next: use the stack's own dev server.
- Pure static: `bunx serve <output> -p 5173`.

### 11.2 Verify (Playwright or Chrome DevTools MCP)
Drive a real browser and check:
- Canvas paints a frame on initial load.
- Scroll triggers frame swaps without `<video>` element involvement (sanity check —
  there should be no `<video>` tag in the DOM for the background).
- No console errors.
- All frames in `manifest.json` return 200 OK.
- Lighthouse run — flag any LCP > 4s, CLS > 0.1, total transfer > 10 MB above the
  fold. Fix per section: drop fps, reduce frame width, or recompress.

### 11.3 Iterate
Loop with the user: show them the running site, take feedback, regenerate
backgrounds / re-animate / re-extract / rewrite copy as needed. Each iteration
updates `PLAN.md` so the source of truth stays current.

---

## Stage 12 — Final report

When the user signs off, write `<output>/REPORT.md`:
- Inputs (URL / BRD / idea).
- Final theme: palette, typography, tone.
- Sections: count, headlines, scroll depths.
- Asset budget: total frame size, total clip size.
- How to run the dev server.
- Known limitations (mobile fallback if any, asset-heavy sections, etc.).

---

## Notes & gotchas

- **Mobile** — frame scrubbing is heavy on mobile data and weaker GPUs. The
  workflow does **not** add a special mobile branch by default. If the user asks
  for one, swap the canvas for a static poster image below a chosen width
  breakpoint.
- **Reduced motion** — the workflow does **not** add a `prefers-reduced-motion`
  branch by default. Only add one if the user explicitly asks.
- **Asset size discipline** — 6 sections × 120 frames × ~120 KB/WebP ≈ 85 MB.
  That's the ceiling, not the target. Pull frame counts down for any section
  whose motion doesn't justify them.
- **Regeneration safety** — Stages 5, 6, and 7 are idempotent per-section. If
  the user wants to redo just section 3's background, only `section-3/*` assets
  and the manifest entry change.
- **Don't run media generation without Stage 4 approval.** Image/video API calls
  are slow and metered — the design checkpoint exists to protect the user from
  spending those tokens twice.
