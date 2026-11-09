// After a redeploy the old index references chunk hashes that no longer exist,
// so lazy imports fail. One reload fetches the new index; the sessionStorage
// guard stops a reload loop when the chunk is genuinely broken.

const KEY = 'rustic:chunk-reload-at';
const WINDOW_MS = 30_000;

const PATTERNS = [
  /Failed to fetch dynamically imported module/i,
  /Importing a module script failed/i,
  /error loading dynamically imported module/i,
  /Unable to preload CSS/i,
  /ChunkLoadError/i,
];

/** True when `err` is a failed lazy-chunk / dynamic-import load. */
export function isChunkLoadError(err) {
  if (!err) return false;
  const text = `${err.name || ''} ${err.message || err}`;
  return PATTERNS.some((re) => re.test(text));
}

/** Reload once per 30 s window; returns whether a reload was triggered. */
export function reloadOnceForChunkError(storage = window.sessionStorage, reload = () => window.location.reload()) {
  const now = Date.now();
  let last = 0;
  try { last = Number(storage.getItem(KEY)) || 0; } catch { /* storage blocked */ }
  if (now - last < WINDOW_MS) return false;
  try { storage.setItem(KEY, String(now)); } catch { /* storage blocked */ }
  reload();
  return true;
}

/** Install the global listeners that auto-recover from stale lazy chunks. */
export function installChunkReload() {
  window.addEventListener('vite:preloadError', (event) => {
    if (reloadOnceForChunkError()) event.preventDefault();
  });
  window.addEventListener('unhandledrejection', (event) => {
    if (isChunkLoadError(event.reason)) reloadOnceForChunkError();
  });
}
