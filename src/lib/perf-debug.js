// Lightweight diagnostics for the "Rustic freezes after task complete"
// investigation. The watchdogs catch main-thread blocks; the timing helpers
// are sprinkled into the rendering hot paths so a stalled region prints a
// `[freeze]` breadcrumb to the devtools console with a label and duration.
//
// Toggle off at runtime: `window.__rusticPerfOff = true`.

const SLOW_RENDER_MS = 50;          // log timed regions that exceed this
const LONG_TASK_MS = 80;             // browser-detected long tasks
const HEARTBEAT_TICK_MS = 250;
const HEARTBEAT_TOLERANCE_MS = 100;
const HUGE_PAYLOAD_BYTES = 32_000;   // log payloads larger than this

function active() {
  return typeof window === 'undefined' || !window.__rusticPerfOff;
}

let longTaskObserver = null;
export function installLongTaskObserver() {
  if (longTaskObserver) return;
  if (typeof PerformanceObserver === 'undefined') return;
  if (!PerformanceObserver.supportedEntryTypes?.includes('longtask')) return;
  try {
    longTaskObserver = new PerformanceObserver((list) => {
      if (!active()) return;
      for (const entry of list.getEntries()) {
        if (entry.duration < LONG_TASK_MS) continue;
        // eslint-disable-next-line no-console
        console.warn(`[freeze][longtask] ${entry.duration.toFixed(0)}ms blocked main thread`);
      }
    });
    longTaskObserver.observe({ entryTypes: ['longtask'] });
  } catch {
    // ignore — longtask is best-effort
  }
}

let heartbeatTimer = null;
let lastHeartbeat = 0;
export function installHeartbeat() {
  if (heartbeatTimer) return;
  lastHeartbeat = performance.now();
  const tick = () => {
    const now = performance.now();
    const gap = now - lastHeartbeat;
    if (active() && gap > HEARTBEAT_TICK_MS + HEARTBEAT_TOLERANCE_MS) {
      // eslint-disable-next-line no-console
      console.warn(`[freeze][heartbeat] main thread was blocked for ~${(gap - HEARTBEAT_TICK_MS).toFixed(0)}ms`);
    }
    lastHeartbeat = now;
    heartbeatTimer = setTimeout(tick, HEARTBEAT_TICK_MS);
  };
  heartbeatTimer = setTimeout(tick, HEARTBEAT_TICK_MS);
}

// Wrap a synchronous region. Logs only if it exceeds SLOW_RENDER_MS.
export function timeSync(label, fn) {
  if (!active()) return fn();
  const t0 = performance.now();
  try { return fn(); }
  finally {
    const ms = performance.now() - t0;
    if (ms >= SLOW_RENDER_MS) {
      // eslint-disable-next-line no-console
      console.warn(`[freeze][slow] ${label} — ${ms.toFixed(1)}ms`);
    }
  }
}

// Async variant.
export async function timeAsync(label, fn) {
  if (!active()) return fn();
  const t0 = performance.now();
  try { return await fn(); }
  finally {
    const ms = performance.now() - t0;
    if (ms >= SLOW_RENDER_MS) {
      // eslint-disable-next-line no-console
      console.warn(`[freeze][slow-async] ${label} — ${ms.toFixed(1)}ms`);
    }
  }
}

// Log when a string crosses a size threshold so we can spot the pasted
// blob, big tool result, or runaway summary that's about to feed marked /
// DOMPurify / a giant DOM build.
export function logBigString(label, str) {
  if (!active() || typeof str !== 'string') return;
  const len = str.length;
  if (len < HUGE_PAYLOAD_BYTES) return;
  let newlines = 0;
  for (let i = 0; i < len; i++) {
    if (str.charCodeAt(i) === 10) newlines++;
  }
  // eslint-disable-next-line no-console
  console.warn(`[freeze][big-payload] ${label}: ${len.toLocaleString()} chars, ${newlines.toLocaleString()} newlines`);
}

// One-shot tagged log we can drop next to suspected trigger sites so the
// long-task / heartbeat output is easy to correlate with what was running.
export function mark(label, extra) {
  if (!active()) return;
  // eslint-disable-next-line no-console
  console.log(`[freeze][mark] ${label}`, extra ?? '');
}
