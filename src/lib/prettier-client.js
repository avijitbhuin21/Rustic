// Lazy client around the Prettier Web Worker. The worker is only spawned the
// first time `formatWithPrettier` is called, then stays warm for subsequent
// saves. Each call awaits an id-tagged round-trip so concurrent saves don't
// cross their responses.

import PrettierWorker from './prettier-worker.js?worker';

// Monaco language ids Prettier knows how to handle. Kept in sync with the
// PARSER_BY_LANGUAGE map in prettier-worker.js so the renderer can decide
// whether to even dispatch — no point waking the worker for an unsupported
// language.
export const PRETTIER_LANGUAGES = new Set([
  'javascript', 'javascriptreact', 'jsx',
  'typescript', 'typescriptreact', 'tsx',
  'json', 'jsonc', 'json5',
  'css', 'scss', 'less',
  'html', 'vue', 'angular',
  'markdown', 'mdx',
  'yaml',
]);

export function isPrettierLanguage(language) {
  return !!language && PRETTIER_LANGUAGES.has(language.toLowerCase());
}

let worker = null;
let nextId = 1;
const pending = new Map();

function ensureWorker() {
  if (worker) return worker;
  worker = new PrettierWorker();
  worker.onmessage = (ev) => {
    const { id, ok, formatted, error } = ev.data || {};
    const slot = pending.get(id);
    if (!slot) return;
    pending.delete(id);
    if (ok) slot.resolve(formatted);
    else slot.reject(new Error(error || 'prettier failed'));
  };
  worker.onerror = (ev) => {
    // Bubble the error to every pending request — the worker is dead.
    const err = new Error(ev.message || 'prettier worker crashed');
    for (const { reject } of pending.values()) reject(err);
    pending.clear();
    try { worker.terminate(); } catch {}
    worker = null;
  };
  return worker;
}

export function formatWithPrettier(language, source, options) {
  return new Promise((resolve, reject) => {
    const w = ensureWorker();
    const id = nextId++;
    pending.set(id, { resolve, reject });
    w.postMessage({ id, language, source, options });
  });
}

// Allow the UI to release worker memory on demand (e.g. when toggling
// format-on-save off, or via an "Unload bundled formatters" action).
export function terminatePrettierWorker() {
  if (!worker) return;
  try { worker.terminate(); } catch {}
  worker = null;
  for (const { reject } of pending.values()) {
    reject(new Error('prettier worker terminated'));
  }
  pending.clear();
}
