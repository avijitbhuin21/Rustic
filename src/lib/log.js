// Tiny debug-namespace logger gated on localStorage.
//
// Enable from devtools console:
//   localStorage.setItem('rustic:debug', 'FileTree,DnD')
//   localStorage.setItem('rustic:debug', '*')   // everything
//
// In normal operation no debug() call produces console output.
// Errors should NOT use this — use console.error or the toast helper.

let cachedSet = null;

function readEnabled() {
  try {
    const raw = localStorage.getItem('rustic:debug');
    if (!raw) return new Set();
    if (raw === '*') return new Set(['*']);
    return new Set(raw.split(',').map((s) => s.trim()).filter(Boolean));
  } catch {
    return new Set();
  }
}

function enabledFor(ns) {
  if (cachedSet === null) cachedSet = readEnabled();
  return cachedSet.has('*') || cachedSet.has(ns);
}

export function debug(namespace, ...args) {
  if (!enabledFor(namespace)) return;
  console.log(`[${namespace}]`, ...args);
}

export function debugFn(namespace) {
  return (...args) => debug(namespace, ...args);
}

// Reset the cache when the user toggles the flag at runtime.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (e) => {
    if (e.key === 'rustic:debug') cachedSet = null;
  });
}
