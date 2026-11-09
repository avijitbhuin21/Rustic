// Unsaved editor buffers, keyed by tab id. Monaco remounts (and re-reads the
// file from disk) whenever the user switches tabs, so without this cache any
// unsaved edit was silently replaced by the on-disk content.
import { useEditor } from '@/state/editor';

const buffers = new Map();

/** Returns the cached `{ value, original }` for a tab, or null. */
export function getUnsavedBuffer(tabId) {
  return buffers.get(tabId) ?? null;
}

/** Records (or clears, when clean) the live buffer for a tab. */
export function setUnsavedBuffer(tabId, value, original) {
  if (value === original) buffers.delete(tabId);
  else buffers.set(tabId, { value, original });
}

/** Forgets a tab's unsaved buffer (after save / discard). */
export function dropUnsavedBuffer(tabId) {
  buffers.delete(tabId);
}

// Drop buffers for tabs that were closed so a reopened file starts from disk.
useEditor.subscribe((s) => {
  if (buffers.size === 0) return;
  const open = new Set();
  for (const g of s.groups ?? []) for (const t of g.tabs) open.add(t.id);
  for (const id of buffers.keys()) if (!open.has(id)) buffers.delete(id);
});
