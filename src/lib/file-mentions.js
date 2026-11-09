import { useEditor } from '@/state/editor';

const FILE_REFS_FOOTER_RE =
  /(?:\n\n)?\[Referenced files[^\]\n]*\]((?:\n[ \t]*-[^\n]*)+)\s*$/;

/** Splits text into plain and `@path` mention segments for the given relative paths. */
export function splitFileMentions(text, paths) {
  const src = text || '';
  const list = [...new Set((paths || []).filter(Boolean))].sort((a, b) => b.length - a.length);
  if (list.length === 0 || !src.includes('@')) return [{ type: 'text', text: src }];
  const out = [];
  let buf = '';
  let i = 0;
  while (i < src.length) {
    if (src[i] === '@' && (i === 0 || /\s|[(["'`]/.test(src[i - 1]))) {
      const hit = list.find((p) => src.startsWith(p, i + 1) && !/[\w./\\-]/.test(src[i + 1 + p.length] || ''));
      if (hit) {
        if (buf) out.push({ type: 'text', text: buf });
        buf = '';
        out.push({ type: 'file', path: hit, start: i, end: i + 1 + hit.length });
        i += 1 + hit.length;
        continue;
      }
    }
    buf += src[i];
    i += 1;
  }
  if (buf) out.push({ type: 'text', text: buf });
  return out;
}

/** Removes the `[Referenced files]` footer from a sent message; returns `{ text, paths }`. */
export function stripFileRefsFooter(text) {
  if (!text) return { text: text || '', paths: [] };
  const m = FILE_REFS_FOOTER_RE.exec(text);
  if (!m) return { text, paths: [] };
  const paths = m[1]
    .split('\n')
    .map((l) => l.replace(/^[ \t]*-[ \t]*/, '').trim())
    .filter(Boolean);
  return { text: text.slice(0, m.index), paths };
}

/** Opens a project-relative path in the editor. */
export function openFileMention(projectRoot, relPath) {
  if (!relPath) return;
  const isAbs = /^([a-zA-Z]:[\\/]|[\\/])/.test(relPath);
  const root = (projectRoot || '').replace(/[\\/]+$/, '');
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  const abs = isAbs || !root ? relPath : `${root}${sep}${relPath.replace(/^[\\/]+/, '')}`;
  useEditor.getState().openFile(abs);
}
