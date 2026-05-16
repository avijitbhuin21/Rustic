// Explorer-only clipboard for file/folder copy & cut. The OS clipboard cannot
// carry "this is an absolute path that should be copied/moved" semantics that
// survive across our Tauri webview <-> backend boundary, so we maintain our
// own in-process clipboard. The OS clipboard is also written (with the path
// as plain text) so the user can still paste the path into other apps.
//
// State shape:
//   mode:  'copy' | 'cut' | null
//   items: [{ path, name, is_dir, projectName }]
//
// `cut` is honored by `pasteIntoDir`: after a successful copy to the new
// location, the source paths are deleted (matching standard file-manager
// move semantics). On a copy-paste, sources are left alone.

import * as api from '../lib/tauri-api.js';

let state = {
  mode: null,        // 'copy' | 'cut' | null
  items: [],         // entries above
};

/**
 * Try to read absolute file paths from the OS clipboard. Different apps
 * write paths to the clipboard in different ways:
 *   - Windows Explorer / Finder: native file list (NOT exposed to webviews)
 *   - VS Code, IntelliJ, "Copy Path": newline / NUL / quoted text list
 *   - Drag-pasted text: a single absolute path
 *
 * The Tauri webview can't see the native clipboard file list, but it CAN
 * read text. We grab `clipboard.readText()`, split on newlines / quotes /
 * NUL bytes, then ask the backend `stat_path` to confirm each candidate
 * actually exists on disk. Any that resolve become paste sources.
 *
 * Returns [{ path, name, is_dir, projectName: null }] — same shape as
 * internal clipboard entries.
 */
export async function readOsClipboardPaths() {
  // Phase 1: ask the backend for the OS clipboard's native FILE LIST. This
  // is what the user gets when they Ctrl+C a file in Windows Explorer /
  // Finder — a list of absolute paths the webview can't see directly.
  let nativePaths = [];
  try {
    nativePaths = (await api.readClipboardFiles()) || [];
    console.log('[explorer-clipboard] backend file list:', nativePaths);
  } catch (e) {
    console.log('[explorer-clipboard] backend file list read failed:', e?.message || e);
  }

  // Phase 2: also pull text — covers "Copy as path" from Explorer (which
  // puts a quoted path on CF_TEXT) and the standard text-list format used
  // by VS Code / IntelliJ "Copy Path".
  let text = '';
  try {
    text = await navigator.clipboard.readText();
  } catch (e) {
    console.log('[explorer-clipboard] OS clipboard readText failed:', e?.message || e);
  }
  if (text) {
    console.log('[explorer-clipboard] clipboard text (first 200 chars):', text.slice(0, 200));
  }

  const textCandidates = (text || '')
    .split(/[\r\n\0]+/)
    .map(s => s.trim())
    .map(s => s.replace(/^"+|"+$/g, '')) // strip surrounding double quotes
    .map(s => s.replace(/^'+|'+$/g, '')) // strip surrounding single quotes
    .filter(Boolean)
    .slice(0, 64);

  // Merge native + text, dedupe by absolute path. Native list wins so we
  // don't make `n` extra `stat` calls when both surfaces have the same
  // entries.
  const seen = new Set();
  const merged = [];
  for (const p of nativePaths) {
    if (!p) continue;
    const key = p.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(p);
  }
  for (const p of textCandidates) {
    if (!looksLikeAbsolutePath(p)) continue;
    const key = p.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(p);
  }

  console.log('[explorer-clipboard] merged path candidates:', merged);

  const results = [];
  for (const candidate of merged) {
    try {
      const stat = await api.statPath(candidate);
      if (!stat) continue;
      const [name, isDir] = stat;
      results.push({
        path: candidate,
        name,
        is_dir: isDir,
        projectName: null, // unknown — clipboard came from outside our workspace
      });
    } catch (e) {
      // ignore — path didn't exist or couldn't be stat'd
    }
  }
  console.log('[explorer-clipboard] OS clipboard resolved sources:', results);
  return results;
}


function looksLikeAbsolutePath(s) {
  if (!s) return false;
  // Windows: starts with drive letter (e.g. "D:\..." or "D:/..."), or UNC
  // path "\\server\share". POSIX: starts with "/".
  if (/^[a-zA-Z]:[\\/]/.test(s)) return true;
  if (s.startsWith('\\\\')) return true;
  if (s.startsWith('/')) return true;
  return false;
}


const listeners = new Set();

function emit() {
  for (const fn of listeners) {
    try { fn(state); } catch (e) { console.error('[explorer-clipboard] listener err:', e); }
  }
}

export function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getClipboard() {
  return { mode: state.mode, items: [...state.items] };
}

export function isCutPath(path) {
  return state.mode === 'cut' && state.items.some(i => i.path === path);
}

/** True when there's anything to paste. */
export function hasClipboard() {
  return state.mode !== null && state.items.length > 0;
}

export function clear() {
  if (state.mode === null && state.items.length === 0) return;
  state = { mode: null, items: [] };
  emit();
}

/**
 * `entries` is an array of { path, name, is_dir, projectName }.
 * Mirrors OS clipboard with comma-separated paths so pasting into a text
 * field elsewhere produces something sensible.
 */
export function copyItems(entries) {
  if (!entries || entries.length === 0) return;
  state = { mode: 'copy', items: entries.map(e => ({ ...e })) };
  writeOsClipboard(entries, false);
  emit();
}

export function cutItems(entries) {
  if (!entries || entries.length === 0) return;
  state = { mode: 'cut', items: entries.map(e => ({ ...e })) };
  writeOsClipboard(entries, true);
  emit();
}

/**
 * Push the entries onto the OS clipboard so the user can paste them in
 * other apps (Windows Explorer, Finder, Outlook, Slack, etc.). We do this
 * in two layers:
 *   1. CF_TEXT-style path list via the webview clipboard so apps that only
 *      know how to read text get something useful (and so the user can
 *      paste the path into a chat or terminal).
 *   2. Native file-list (CF_HDROP / NSFilenamesPboardType / text/uri-list)
 *      via the backend `write_clipboard_files` command so file managers
 *      treat the paste as an actual file copy/move.
 */
function writeOsClipboard(entries, isCut) {
  try {
    const text = entries.map(e => e.path).join('\n');
    navigator.clipboard.writeText(text).catch(() => {});
  } catch { /* ignore — non-critical */ }

  // Fire-and-forget on the native file list. We don't await it because we
  // don't want to block the UI on a Powershell launch (~150ms on Windows).
  // Errors are logged but ignored so a failed clipboard write doesn't break
  // the internal copy/cut state.
  api.writeClipboardFiles(entries.map(e => e.path), !!isCut)
    .then(() => {
      console.log('[explorer-clipboard] wrote OS file list (cut=%s) for %d items', !!isCut, entries.length);
    })
    .catch((err) => {
      console.warn('[explorer-clipboard] OS file-list write failed:', err);
    });
}


/**
 * Paste the current clipboard into `dstDir`.
 *
 * Returns an array of created destination paths (one per source). On `cut`,
 * sources are deleted *after* every copy succeeds — partial failures leave
 * the source in place so the user doesn't lose data.
 */
export async function pasteIntoDir(dstDir) {
  // Resolve sources: prefer the internal explorer clipboard, otherwise fall
  // back to absolute paths the user copied from another app via the OS
  // clipboard (Windows Explorer's Copy Path, VS Code's "Copy Path", etc.).
  let mode = state.mode;
  let items = state.items;

  if (!hasClipboard()) {
    const osItems = await readOsClipboardPaths();
    if (osItems.length === 0) {
      // No file paths on the OS clipboard. Before giving up, try bitmap data:
      // images copied from a browser / screenshot tool / paint program live
      // on the clipboard as raw pixels with no backing file, so the path-
      // based reads above can't find them. The backend writes any image bytes
      // it finds directly into `dstDir` and returns the new path.
      try {
        const written = await api.pasteClipboardImageInto(dstDir);
        if (written) {
          console.log('[explorer-clipboard] paste: wrote bitmap from clipboard ->', written);
          return [written];
        }
      } catch (e) {
        console.warn('[explorer-clipboard] paste: bitmap read failed:', e?.message || e);
      }
      console.log('[explorer-clipboard] paste: nothing to paste (internal clipboard empty + OS clipboard had no usable paths or images)');
      return [];
    }
    mode = 'copy'; // External pastes are always copy — we can't safely move
                   // a file out of someone else's app.
    items = osItems;
  }

  console.log('[explorer-clipboard] paste mode=%s items=%d -> %s', mode, items.length, dstDir);

  const created = [];
  const succeeded = []; // sources whose copy succeeded — eligible for cut-delete
  for (const item of items) {
    try {
      // Prevent pasting an item into itself or a descendant of itself.
      if (item.is_dir && isSameOrInside(dstDir, item.path)) {
        console.warn('[explorer-clipboard] skipping paste into self/descendant:', item.path, '->', dstDir);
        continue;
      }
      const dst = await api.copyEntry(item.path, dstDir, null);
      console.log('[explorer-clipboard] copied', item.path, '->', dst);
      if (dst) {
        created.push(dst);
        succeeded.push(item);
      }
    } catch (e) {
      console.error('[explorer-clipboard] copy failed:', item.path, '->', dstDir, e);
    }
  }

  // Only do the cut-delete on items that came from the *internal* clipboard.
  // OS-clipboard items are always copy-only (mode forced to 'copy' above).
  if (mode === 'cut' && succeeded.length > 0) {
    for (const item of succeeded) {
      try {
        await api.deleteEntry(item.path);
      } catch (e) {
        console.error('[explorer-clipboard] cut-delete failed:', item.path, e);
      }
    }
    // After a cut, the clipboard is consumed.
    state = { mode: null, items: [] };
    emit();
  }

  return created;
}


/** Path equality / ancestor check that ignores trailing slash + slash style. */
function isSameOrInside(maybeChild, maybeParent) {
  const norm = (p) => (p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
  const child = norm(maybeChild);
  const parent = norm(maybeParent);
  if (!child || !parent) return false;
  if (child === parent) return true;
  return child.startsWith(parent + '/');
}
