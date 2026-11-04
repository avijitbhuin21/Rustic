// Shared clickable-file-path support for agent chat prose, tool-call cards
// and terminal output: detection (pure, regex based), resolution against the
// right project root, and opening in the editor (images → image preview).
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useEditor } from '@/state/editor';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { openFilePalette } from '@/components/command-palette';

// #region pure — no imports used below this marker until #endregion (unit-checked in node)

const KNOWN_EXTENSIONS = new Set([
  'js', 'jsx', 'ts', 'tsx', 'mjs', 'cjs', 'mts', 'cts', 'rs', 'py', 'rb', 'go', 'java',
  'kt', 'c', 'h', 'cpp', 'hpp', 'cc', 'cs', 'css', 'scss', 'less', 'html', 'htm',
  'json', 'jsonc', 'toml', 'yaml', 'yml', 'md', 'mdx', 'txt', 'sql', 'sh', 'ps1',
  'bat', 'cmd', 'lock', 'conf', 'ini', 'env', 'xml', 'svg', 'vue', 'svelte',
  'swift', 'php', 'ex', 'exs', 'zig', 'lua', 'proto', 'graphql', 'prisma', 'ipynb',
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'avif', 'pdf', 'csv', 'tsv',
  'log', 'docx', 'xlsx', 'mp3', 'wav', 'ogg', 'mp4', 'webm', 'mov', 'woff2', 'wasm',
]);
const KNOWN_BARE_FILES = new Set([
  'dockerfile', 'makefile', 'justfile', 'rakefile', 'gemfile', 'procfile',
  'license', 'readme', 'caddyfile', 'cargo.lock',
]);
// Library/product names that look like bare filenames in prose or code spans.
const BARE_DENY = new Set([
  'node.js', 'next.js', 'nuxt.js', 'vue.js', 'react.js', 'three.js', 'd3.js',
  'express.js', 'chart.js', 'ember.js', 'backbone.js', 'socket.io', 'p5.js',
  'solid.js', 'alpine.js', 'nest.js', 'deno.json',
]);

// Location suffix: `:12`, `:12:3`, `:12-20`, `#L12`, `#L12-L20`, `(12,3)`.
const LOC_SRC = String.raw`(?::(\d+)(?::(\d+)|-\d+)?|#L(\d+)(?:-L?\d+)?|\((\d+)(?:,\s?(\d+))?\))`;
const SEG = String.raw`[\w.@+$%~-]+`;
const ROOT = String.raw`(?:[A-Za-z]:[\\/]|~[\\/]|\.{1,2}[\\/]|[\\/])`;
// Embedded paths in free text. The lookbehind keeps us from starting in the
// middle of a word / URL / longer path.
const TEXT_PATH_RE = new RegExp(
  String.raw`(?<![\w.\\/:@~$%+-])(${ROOT}?${SEG}(?:[\\/]${SEG})*)(${LOC_SRC})?`,
  'g',
);
// Whole-string path (inline code / hrefs): segments may contain spaces.
const WHOLE_PATH_RE = new RegExp(
  String.raw`^(${ROOT}?[\w.@+$%~() -]+(?:[\\/][\w.@+$%~() -]+)*)(${LOC_SRC})?$`,
);
const URL_RE = /\b[a-z][a-z0-9+.-]*:\/\/[^\s<>"'`]+/gi;

function locFrom(m, base) {
  // m[base..] = the capture groups of LOC_SRC.
  const line = m[base] || m[base + 2] || m[base + 3];
  const col = m[base] ? m[base + 1] : m[base + 3] ? m[base + 4] : null;
  return {
    line: line ? parseInt(line, 10) || null : null,
    col: col ? parseInt(col, 10) || null : null,
  };
}

/**
 * Decide whether a bare path string (no location suffix) is a file path.
 * `allowBare` permits separator-less names with a known extension.
 */
export function isLikelyFilePath(path, { allowBare = false } = {}) {
  if (!path || path.length > 1024) return false;
  if (/[\\/]$/.test(path)) return false;
  if (/^[a-z][a-z0-9+.-]+:/i.test(path) && !/^[A-Za-z]:[\\/]/.test(path)) return false;
  const body = path.replace(/^[A-Za-z]:/, '');
  const hasSep = /[\\/]/.test(body);
  const segments = body.split(/[\\/]+/).filter(Boolean);
  const last = segments[segments.length - 1];
  if (!last || last === '.' || last === '..') return false;
  // Version numbers / dates / fractions: every segment purely numeric-ish.
  if (segments.every((s) => /^[\d.]+$/.test(s))) return false;
  const lower = last.toLowerCase();
  const dot = last.lastIndexOf('.');
  const ext = dot > 0 ? last.slice(dot + 1).toLowerCase() : null;
  if (!hasSep) {
    if (!allowBare) return false;
    if (BARE_DENY.has(lower)) return false;
    return (!!ext && KNOWN_EXTENSIONS.has(ext)) || KNOWN_BARE_FILES.has(lower);
  }
  if (KNOWN_BARE_FILES.has(lower)) return true;
  if (!ext || !/^[a-z0-9_]{1,10}$/i.test(ext) || /^\d+$/.test(ext)) return false;
  return true;
}

function stripWrapping(raw) {
  let t = raw.trim();
  t = t.replace(/^[(\[{<'"`]+/, '').replace(/[)\]}>'"`,;!?]+$/, '');
  return t;
}

/**
 * Parse a standalone string (inline code span, href, tool input value) into
 * { path, line, col } or null.
 */
export function parsePathCandidate(raw, { allowBare = true } = {}) {
  if (!raw || typeof raw !== 'string') return null;
  let text = stripWrapping(raw);
  if (/^file:\/\//i.test(text)) {
    text = text.replace(/^file:\/\/(localhost)?/i, '');
    if (/^\/[A-Za-z]:[\\/]/.test(text)) text = text.slice(1);
    try { text = decodeURI(text); } catch { /* keep raw */ }
  }
  // Trailing sentence period after the extension: "src/a.js." → "src/a.js".
  text = text.replace(/\.+$/, '');
  const m = WHOLE_PATH_RE.exec(text);
  if (!m) return null;
  const path = m[1].trim();
  if (!isLikelyFilePath(path, { allowBare })) return null;
  return { path, ...locFrom(m, 3) };
}

/**
 * Find embedded file paths in free text. Returns
 * [{ start, end, text, path, line, col }] with `end` exclusive.
 * `allowBare` additionally accepts separator-less names with known extensions.
 */
export function findPathsInText(text, { allowBare = false } = {}) {
  if (!text || typeof text !== 'string') return [];
  const urlRanges = [];
  for (const u of text.matchAll(URL_RE)) urlRanges.push([u.index, u.index + u[0].length]);
  const inUrl = (s, e) => urlRanges.some(([a, b]) => s < b && e > a);
  const out = [];
  TEXT_PATH_RE.lastIndex = 0;
  for (const m of text.matchAll(TEXT_PATH_RE)) {
    let path = m[1];
    const hasLoc = !!m[2];
    let matched = m[0];
    if (!hasLoc) {
      // Drop trailing sentence punctuation that the segment class swallowed.
      const trimmed = path.replace(/[.~-]+$/, '');
      matched = matched.slice(0, matched.length - (path.length - trimmed.length));
      path = trimmed;
    }
    // Leading "(" isn't in SEG, but a trailing ")" never is either; fine.
    const start = m.index;
    const end = start + matched.length;
    if (!path || inUrl(start, end)) continue;
    if (!isLikelyFilePath(path, { allowBare })) continue;
    out.push({ start, end, text: matched, path, ...locFrom(m, 3) });
  }
  return out;
}

export function isAbsolutePath(p) {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('/') || p.startsWith('\\');
}

/** Join + normalise `rel` against `base` (handles ./, ../, mixed separators). */
export function joinPath(base, rel) {
  if (!rel) return null;
  if (isAbsolutePath(rel)) return normalizeSeparators(rel, rel);
  if (!base) return null;
  const driveMatch = base.match(/^[a-zA-Z]:/);
  const drive = driveMatch ? driveMatch[0] : '';
  const rest = drive ? base.slice(drive.length) : base;
  const parts = [];
  for (const seg of `${rest}/${rel}`.split(/[\\/]+/)) {
    if (!seg || seg === '.') continue;
    if (seg === '..') parts.pop();
    else parts.push(seg);
  }
  const prefix = drive ? `${drive}/` : (base.startsWith('/') || base.startsWith('\\')) ? '/' : '';
  return normalizeSeparators(`${prefix}${parts.join('/')}`, base);
}

/** Use the reference path's separator style (keeps editor tab ids unique on Windows). */
export function normalizeSeparators(p, reference) {
  if (!p) return p;
  if (reference && /^[a-zA-Z]:/.test(reference) && reference.includes('\\')) return p.replace(/\//g, '\\');
  return p;
}

// #endregion pure

// ── Roots ───────────────────────────────────────────────────────────────────

/** All known project roots (explorer projects). */
export function projectRoots() {
  try {
    return (useExplorer.getState().projects || []).map((p) => p.root_path || p.root).filter(Boolean);
  } catch {
    return [];
  }
}

/** Root of the project that owns `taskId` (falls back to the agent's active project). */
export function rootForTask(taskId) {
  const s = useAgent.getState();
  const fallback = s.activeProject?.root || null;
  if (!taskId) return fallback;
  let projectId = null;
  let task = null;
  for (const [pid, list] of Object.entries(s.tasksByProject || {})) {
    const t = (list || []).find((x) => x.id === taskId);
    if (t) { projectId = pid; task = t; break; }
  }
  if (!task) task = (s.tasks || []).find((x) => x.id === taskId) || null;
  if (task?.project_root) return task.project_root;
  projectId = task?.project_id || projectId;
  if (projectId) {
    const p = (useExplorer.getState().projects || []).find((x) => x.id === projectId);
    if (p?.root_path) return p.root_path;
    if (s.activeProject?.id === projectId && s.activeProject.root) return s.activeProject.root;
  }
  return fallback;
}

/** Root for the chat currently on screen. */
export function activeChatRoot() {
  return rootForTask(useAgent.getState().activeTaskId);
}

// ── Opening ─────────────────────────────────────────────────────────────────

async function statPath(path) {
  try {
    const s = await invoke('stat_path', { path });
    if (!s) return { exists: false, isDir: false };
    if (Array.isArray(s)) return { exists: true, isDir: !!s[1] };
    return { exists: !!s.exists, isDir: !!s.isDir };
  } catch {
    return { exists: false, isDir: false };
  }
}

async function suffixMatches(rel, root) {
  try {
    const files = await invoke('list_project_files', { rootPath: root, maxFiles: 20000 });
    if (!Array.isArray(files)) return [];
    const needle = rel.replace(/\\/g, '/').replace(/^\.\//, '').toLowerCase();
    return files.filter((f) => {
      const norm = f.replace(/\\/g, '/').toLowerCase();
      return norm === needle || norm.endsWith(`/${needle}`);
    });
  } catch {
    return [];
  }
}

function openResolved(abs, line, col) {
  const nav = line ? { line, matchStart: col ? col - 1 : 0, matchEnd: col ? col - 1 : 0 } : null;
  useEditor.getState().openFile(abs, nav);
}

/**
 * Resolve and open a file link. `target` is a string or { path, line, col }.
 * Relative paths try `baseDir` first, then every project root, then a
 * suffix search of the project files. Failures toast instead of failing
 * silently.
 */
export async function openFileLink(target, { baseDir = null } = {}) {
  const parsed = typeof target === 'string' ? parsePathCandidate(target) || { path: target } : target;
  if (!parsed?.path) return;
  let { path } = parsed;
  const { line = null, col = null } = parsed;
  try { if (/%[0-9a-f]{2}/i.test(path)) path = decodeURIComponent(path); } catch { /* keep */ }
  const base = baseDir || activeChatRoot();
  const roots = [base, ...projectRoots()].filter((r, i, a) => r && a.indexOf(r) === i);

  const candidates = isAbsolutePath(path)
    ? [joinPath(null, path)]
    : path.startsWith('~')
      ? []
      : roots.map((r) => joinPath(r, path)).filter(Boolean);

  let sawDir = false;
  for (const abs of candidates) {
    const st = await statPath(abs);
    if (st.exists && !st.isDir) { openResolved(abs, line, col); return; }
    if (st.isDir) sawDir = true;
  }
  if (sawDir) { toast.error(`Path is a folder: ${path}`); return; }

  if (!isAbsolutePath(path)) {
    const rel = path.replace(/^~[\\/]/, '');
    for (const r of roots) {
      const matches = await suffixMatches(rel, r);
      if (matches.length === 1) { openResolved(normalizeSeparators(matches[0], r), line, col); return; }
      if (matches.length > 1) { openFilePalette(rel.replace(/\\/g, '/')); return; }
    }
  }
  toast.error(`File not found: ${path}`);
}

// ── Rendering helpers ───────────────────────────────────────────────────────

export const FILE_LINK_CLASSES =
  'cursor-pointer underline decoration-dotted underline-offset-2 hover:text-primary';

/** Tag an element (code span / span / anchor) as a clickable file link. */
export function markFileLinkElement(el, candidate) {
  el.setAttribute('data-file-link', '1');
  el.setAttribute('data-path', candidate.path);
  if (candidate.line) el.setAttribute('data-line', String(candidate.line));
  if (candidate.col) el.setAttribute('data-col', String(candidate.col));
  el.setAttribute('title', 'Open in editor');
  el.className = `${el.className ? `${el.className} ` : ''}${FILE_LINK_CLASSES}`;
}

/**
 * Post-process sanitized markdown HTML: inline code spans, plain-text runs and
 * local-file anchors that point at files get data attributes so a delegated
 * click handler can open them.
 */
export function linkifyFilePaths(html) {
  if (!html) return html;
  const doc = new DOMParser().parseFromString(html, 'text/html');
  let changed = false;

  for (const a of doc.querySelectorAll('a[href]')) {
    const href = a.getAttribute('href') || '';
    if (!href || href.startsWith('#') || /^(https?|mailto):/i.test(href)) continue;
    const candidate = parsePathCandidate(href, { allowBare: true });
    if (candidate) {
      markFileLinkElement(a, candidate);
      changed = true;
    }
  }

  for (const code of doc.querySelectorAll('code')) {
    if (code.closest('pre, a')) continue;
    const candidate = parsePathCandidate(code.textContent, { allowBare: true });
    if (candidate) {
      markFileLinkElement(code, candidate);
      changed = true;
    }
  }

  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const textNodes = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n.parentElement?.closest('a, code, pre')) continue;
    if (n.nodeValue && /[\\/]/.test(n.nodeValue)) textNodes.push(n);
  }
  for (const node of textNodes) {
    const text = node.nodeValue;
    const hits = findPathsInText(text);
    if (!hits.length) continue;
    const frag = doc.createDocumentFragment();
    let last = 0;
    for (const h of hits) {
      if (h.start > last) frag.appendChild(doc.createTextNode(text.slice(last, h.start)));
      const span = doc.createElement('span');
      span.textContent = h.text;
      markFileLinkElement(span, h);
      frag.appendChild(span);
      last = h.end;
    }
    if (last < text.length) frag.appendChild(doc.createTextNode(text.slice(last)));
    node.parentNode.replaceChild(frag, node);
    changed = true;
  }

  return changed ? doc.body.innerHTML : html;
}

/** Click handler for elements tagged by markFileLinkElement. Returns true if handled. */
export function handleFileLinkClick(e, baseDir) {
  const el = e.target?.closest?.('[data-file-link]');
  if (!el) return false;
  e.preventDefault();
  e.stopPropagation();
  openFileLink(
    { path: el.dataset.path, line: Number(el.dataset.line) || null, col: Number(el.dataset.col) || null },
    { baseDir },
  );
  return true;
}
