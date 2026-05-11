---
name: landing-page-cloning-workflow
description: Fully mirror a public website to a local folder, rewrite all references so it runs offline, then iteratively verify against a real browser console until zero errors / missing assets remain.
---

# Landing page cloning workflow

## Purpose
Fully mirror a public website to a local folder, rewrite all references so it runs offline, then iteratively verify against a real browser console until **zero** errors / missing assets remain.

## Invocation
Run this workflow with a single argument: the target URL.

> Example: "Run landing-page-cloning-workflow on https://example.com/"

If no URL is provided, **ask the user** for one before doing anything else.

---

## Stage 0 — Preflight: tool detection

Before touching the network, check what's installed and ask the user to fill any gaps.

### 0.1 Mirror tool (wget or HTTrack)
Run these checks:
```powershell
wget --version
httrack --version
```

Decision tree:
- **Both installed** → ask the user which to use (default: `wget`).
- **Only one installed** → use it, inform the user.
- **Neither installed** → ask the user:
  > "Neither `wget` nor `httrack` is installed. Which would you like to install?
  > 1. wget (recommended — simpler, faster)
  > 2. httrack (GUI option, better for very large sites)
  > 3. Skip — use Playwright-only mode (slower but no install needed)"

  Then install the chosen one via `winget`:
  - `winget install JernejSimoncic.Wget`
  - `winget install HTTrack.WinHTTrack`

### 0.2 Headless browser / inspection tool
We need a way to drive a real browser to (a) catch JS-loaded assets and (b) read the console for errors.

Check availability in this order:
1. **Chrome DevTools MCP server** — check if it's registered/available in the current session.
2. **Playwright** — `bunx playwright --version`
3. **Puppeteer** — `bun pm ls puppeteer` in project
4. **Plain Chromium with `--remote-debugging-port`** as last resort

If none are available, ask the user:
> "No headless-browser tooling is installed. Which would you like to use?
> 1. Chrome DevTools MCP server (best for interactive console inspection)
> 2. Playwright (most reliable for crawling + asset capture)
> 3. Puppeteer (lighter alternative)"

Then install the chosen one:
- MCP: instruct user to add the server to their MCP config (cannot auto-install).
- Playwright: `bun add -d playwright; bunx playwright install chromium`
- Puppeteer: `bun add -d puppeteer`

**Permission gate**: Before launching any browser (headless or not), explicitly ask the user for permission per project rules.

### 0.3 Local static server
Verify a static server is available for the verify stage:
```powershell
bunx serve --version
```
If missing: `bun add -g serve`.

---

## Stage 1 — Setup workspace

1. Create folder `cloned-website/` at project root (skip if exists; ask user whether to wipe or resume).
2. Inside it, create:
   - `site/` — the mirrored files
   - `logs/` — crawl logs, console dumps, missing-asset reports
   - `scripts/` — generated crawler / verifier scripts

---

## Stage 2 — Bulk mirror (static sweep)

If `wget` was chosen:
```powershell
wget --mirror --convert-links --adjust-extension --page-requisites --no-parent `
     --no-host-directories --restrict-file-names=windows `
     -e robots=off -U "Mozilla/5.0" `
     -P cloned-website/site `
     <URL>
```

If `httrack` was chosen:
```powershell
httrack <URL> -O cloned-website/site -%v --robots=0
```

If **Playwright-only** mode was chosen, skip to Stage 3 — the crawler will do everything.

Log all output to `cloned-website/logs/mirror.log`.

---

## Stage 3 — Dynamic asset capture (headless browser crawl)

The static mirror in Stage 2 misses assets that are injected by JavaScript at runtime
(lazy-loaded images, fonts requested by CSS variables, fetch/XHR JSON, web-component
templates, hashed bundle URLs). Stage 3 visits each crawled page in a real browser,
records every successful network response, and saves anything the static mirror missed.

### 3.1 Generate the crawler script
Write `cloned-website/scripts/crawl.mjs` using the headless tool chosen in Stage 0.

A Playwright reference implementation:
```javascript
// cloned-website/scripts/crawl.mjs
import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath, URL } from 'node:url';

const TARGET = process.argv[2];
const OUT = 'cloned-website/site';
const ORIGIN = new URL(TARGET).origin;

const browser = await chromium.launch();
const ctx = await browser.newContext({ userAgent: 'Mozilla/5.0 RusticClone/1.0' });
const page = await ctx.newPage();

const seen = new Set();
const queue = [TARGET];
const missing = [];

ctx.on('response', async (resp) => {
  try {
    const url = resp.url();
    if (!url.startsWith(ORIGIN)) return;     // skip 3rd-party
    if (resp.status() >= 400) { missing.push({ url, status: resp.status() }); return; }
    const u = new URL(url);
    let rel = u.pathname.replace(/^\/+/, '');
    if (rel === '' || rel.endsWith('/')) rel += 'index.html';
    const out = join(OUT, rel);
    await mkdir(dirname(out), { recursive: true });
    const body = await resp.body().catch(() => null);
    if (body) await writeFile(out, body);
  } catch (e) { /* swallow per-response */ }
});

while (queue.length) {
  const url = queue.shift();
  if (seen.has(url)) continue;
  seen.add(url);
  console.log('→', url);
  try {
    await page.goto(url, { waitUntil: 'networkidle', timeout: 30_000 });
    // Scroll to trigger lazy loaders / intersection observers
    await page.evaluate(async () => {
      for (let y = 0; y < document.body.scrollHeight; y += 800) {
        window.scrollTo(0, y); await new Promise(r => setTimeout(r, 100));
      }
    });
    const hrefs = await page.$$eval('a[href]', as => as.map(a => a.href));
    for (const h of hrefs) {
      if (h.startsWith(ORIGIN) && !h.includes('#') && !seen.has(h)) queue.push(h);
    }
  } catch (e) { missing.push({ url, error: String(e) }); }
}

await writeFile('cloned-website/logs/crawl-missing.json', JSON.stringify(missing, null, 2));
await browser.close();
console.log(`done. pages=${seen.size} missing=${missing.length}`);
```

### 3.2 Run the crawler
```powershell
bunx playwright install chromium   # idempotent
node cloned-website/scripts/crawl.mjs <URL> 2>&1 | Tee-Object cloned-website/logs/crawl.log
```

### 3.3 Audit
Read `cloned-website/logs/crawl-missing.json`. For each missing asset:
- 4xx/5xx → record in `logs/skipped.txt` (likely a CDN gate or auth-walled asset).
- network/timeout → retry once with a longer `waitUntil: 'load'`.

---

## Stage 4 — Rewrite references

Even after Stages 2 and 3 there will be absolute URLs baked into HTML/CSS/JS that
still point at the live origin. Rewrite them to project-relative paths.

### 4.1 Build the rewrite map
For every file under `cloned-website/site/`:
1. Compute its path relative to `site/`.
2. The matching absolute URL is `<ORIGIN>/<relative-path>` (and `<ORIGIN>/` for `index.html`).

### 4.2 Apply rewrites
Use a one-shot Node script `cloned-website/scripts/rewrite.mjs` rather than `sed`/`Edit` —
binary assets (images, fonts) must be skipped:

```javascript
// cloned-website/scripts/rewrite.mjs
import { readdir, readFile, writeFile, stat } from 'node:fs/promises';
import { join, extname } from 'node:path';

const ORIGIN = process.argv[2];
const ROOT = 'cloned-website/site';
const TEXT_EXT = new Set(['.html', '.htm', '.css', '.js', '.mjs', '.json', '.svg', '.xml', '.txt']);

async function* walk(dir) {
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) yield* walk(p); else yield p;
  }
}

let edits = 0;
for await (const file of walk(ROOT)) {
  if (!TEXT_EXT.has(extname(file).toLowerCase())) continue;
  const before = await readFile(file, 'utf8');
  let after = before
    .replaceAll(`${ORIGIN}/`, '/')          // absolute → root-relative
    .replaceAll(ORIGIN, '');                // bare origin
  // protocol-relative variants
  const host = ORIGIN.replace(/^https?:/, '');
  after = after.replaceAll(`${host}/`, '/').replaceAll(host, '');
  if (after !== before) { await writeFile(file, after); edits++; }
}
console.log(`rewrote ${edits} files`);
```

Run it:
```powershell
node cloned-website/scripts/rewrite.mjs <ORIGIN>
```

### 4.3 Patch lingering CDNs
If the page uses 3rd-party CDNs (fonts.googleapis.com, cdn.jsdelivr.net, …) and the
user wants a fully-offline clone, download those too and rewrite as in 4.1–4.2. Otherwise
note them in `logs/external-cdns.txt` and leave them remote.

---

## Stage 5 — Local serve & smoke test

```powershell
bunx serve cloned-website/site -p 5173
```
Open `http://localhost:5173/` in a normal browser and visually confirm the home page
renders. If it's clearly broken (blank page, layout collapse), stop and inspect
DevTools manually — the verify loop assumes a roughly-working baseline.

---

## Stage 6 — Verify loop (until zero errors)

This is the iterative heart of the workflow. Repeat until the report is clean.

### 6.1 Generate the verifier script
`cloned-website/scripts/verify.mjs` — walks every HTML page on the local server and
captures: console errors, failed network requests, unresolved `<img>`/`<script>`/`<link>` URLs.

```javascript
// cloned-website/scripts/verify.mjs
import { chromium } from 'playwright';
import { readdir, writeFile } from 'node:fs/promises';
import { join, relative } from 'node:path';

const ROOT = 'cloned-website/site';
const BASE = 'http://localhost:5173';

async function* htmlPages(dir) {
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) yield* htmlPages(p);
    else if (p.endsWith('.html')) yield p;
  }
}

const browser = await chromium.launch();
const ctx = await browser.newContext();
const report = [];

for await (const file of htmlPages(ROOT)) {
  const rel = '/' + relative(ROOT, file).replaceAll('\\', '/');
  const url = BASE + rel.replace(/index\.html$/, '');
  const page = await ctx.newPage();
  const errors = [], failed = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));
  page.on('requestfailed', r => failed.push({ url: r.url(), reason: r.failure()?.errorText }));
  page.on('response', r => { if (r.status() >= 400) failed.push({ url: r.url(), status: r.status() }); });
  try { await page.goto(url, { waitUntil: 'networkidle', timeout: 20_000 }); }
  catch (e) { errors.push(`navigation: ${e.message}`); }
  if (errors.length || failed.length) report.push({ page: rel, errors, failed });
  await page.close();
}
await browser.close();
await writeFile('cloned-website/logs/verify-report.json', JSON.stringify(report, null, 2));
console.log(`pages-with-issues=${report.length}`);
```

### 6.2 Run it & read the report
```powershell
node cloned-website/scripts/verify.mjs
```
If `pages-with-issues=0` → **done, jump to Stage 7**.

### 6.3 Triage the report
For each entry in `verify-report.json`, classify by root cause:

| Symptom                                  | Fix                                                                 |
|------------------------------------------|---------------------------------------------------------------------|
| 404 on `/foo/bar.png`                    | Re-fetch from origin → save under `site/foo/bar.png`.               |
| Absolute URL still hitting origin        | Re-run Stage 4 with the missed URL pattern added.                   |
| Inline JS references hashed bundle path  | Locate the bundle name in the HTML, fetch it explicitly.            |
| CORS / mixed-content error               | Asset is on an external CDN — either localize it or whitelist it.   |
| Page-error from missing global (`gtag`…) | 3rd-party analytics — stub it: inject `window.gtag = () => {}`.     |

Apply fixes, then **loop back to 6.2**. Keep iterating until the report is empty.

**Safety cap:** if the same error survives 5 iterations, stop and surface the report to
the user — something needs human judgement (auth wall, signed URL, intentional dynamic
content).

---

## Stage 7 — Final report

Once verify is clean, write `cloned-website/REPORT.md`:

- **Source URL** and date cloned.
- **Pages mirrored:** count + list.
- **Assets mirrored:** count + total size.
- **External CDNs left remote:** from `logs/external-cdns.txt`.
- **Skipped (4xx/5xx) assets:** from `logs/skipped.txt`.
- **How to serve locally:** `bunx serve cloned-website/site -p 5173`.

Confirm with the user that the clone meets their needs before exiting.

---

## Notes & gotchas
- **Single-page apps (React/Vue/etc.):** the static sweep gets you the shell + bundles
  but route-based content lives in JS. Rely on Stage 3 to walk client-side routes — you
  may need to extend `crawl.mjs` to call `history.pushState` for each known route.
- **Auth-walled content:** out of scope. The workflow assumes a public site.
- **Large sites:** above ~500 pages, switch wget → httrack and add a depth cap. Don't
  let the crawl run unbounded.
- **Re-runs:** Stages 2–6 are all idempotent — safe to resume after a crash. Stage 1
  guards against accidental wipes.
