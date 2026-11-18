import { useEditor } from '@/state/editor';

/** Path of the file in the focused editor group's active tab, or null. */
export function selectActiveFilePath(s) {
  const g = (s.groups || []).find((x) => x.id === s.activeGroupId);
  const t = g?.tabs.find((x) => x.id === g.activeId);
  return t?.path || null;
}

/** Subscribes to the active editor file path. */
export function useActiveFilePath() {
  return useEditor(selectActiveFilePath);
}

/** Normalises a path for comparison (forward slashes, no trailing slash, case-folded on Windows drives). */
export function pathKey(p) {
  const s = (p || '').replace(/\\/g, '/').replace(/\/+$/, '');
  return /^[a-zA-Z]:\//.test(s) ? s.toLowerCase() : s;
}

/** True when `path` lies strictly inside `root`. */
export function isInside(root, path) {
  const r = pathKey(root);
  const p = pathKey(path);
  return !!r && p.startsWith(`${r}/`);
}
