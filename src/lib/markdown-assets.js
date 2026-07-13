// Shared helpers for markdown surfaces (file preview, agent chat) that let
// rendered HTML reference LOCAL workspace files: `![img](./shot.png)` or a
// link to `./docs/other.md`. Local image paths are rewritten to a URL the
// webview can actually load (Tauri asset protocol on desktop, `/api/asset`
// on web — both come from `convertFileSrc`, which the web build aliases to
// the HTTP shim). Local link clicks open the target file in an editor tab.
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { open as shellOpen } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import { useEditor } from '@/state/editor';
import { openFilePalette } from '@/components/command-palette';

/** Return the directory portion of a path (handles / and \ separators). */
export function dirname(path) {
  if (!path) return null;
  const i = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return i > 0 ? path.slice(0, i) : null;
}

/** True when the href points outside the local filesystem (scheme, //, #). */
export function isExternalHref(href) {
  return /^[a-z][a-z0-9+.-]+:/i.test(href) || href.startsWith('//');
}

function isAbsolutePath(p) {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('/') || p.startsWith('\\');
}

/** Resolve a markdown href to an absolute local path, or null if it can't be. */
export function resolveLocalPath(href, baseDir) {
  if (!href) return null;
  let target = href.split(/[?#]/)[0];
  if (!target) return null;
  try {
    target = decodeURIComponent(target);
  } catch {
    // keep the raw value if it isn't valid percent-encoding
  }
  if (isExternalHref(target)) return null;
  if (isAbsolutePath(target)) return target;
  if (!baseDir) return null;

  const driveMatch = baseDir.match(/^[a-zA-Z]:/);
  const drive = driveMatch ? driveMatch[0] : '';
  const baseRest = drive ? baseDir.slice(drive.length) : baseDir;
  const out = [];
  for (const seg of `${baseRest}/${target}`.split(/[\\/]+/)) {
    if (!seg || seg === '.') continue;
    if (seg === '..') out.pop();
    else out.push(seg);
  }
  const prefix = drive ? `${drive}/` : baseDir.startsWith('/') || baseDir.startsWith('\\') ? '/' : '';
  return `${prefix}${out.join('/')}`;
}

/** Rewrite local img/video/audio srcs in sanitized HTML to loadable asset URLs. */
export function rewriteLocalAssetSrcs(html, baseDir) {
  if (!html || !html.includes('src=')) return html;
  const doc = new DOMParser().parseFromString(html, 'text/html');
  let changed = false;
  for (const el of doc.querySelectorAll('img[src], video[src], audio[src], source[src]')) {
    const src = el.getAttribute('src');
    if (!src || isExternalHref(src) || src.startsWith('data:') || src.startsWith('blob:')) continue;
    const abs = resolveLocalPath(src, baseDir);
    if (abs) {
      el.setAttribute('src', convertFileSrc(abs));
      changed = true;
    }
  }
  return changed ? doc.body.innerHTML : html;
}

// Only these URL schemes may leave the app via shell.open. Anything else
// (file:, javascript:, custom protocol handlers like ms-settings:, etc.)
// is a lateral-movement / code-execution vector when fed attacker-controlled
// markdown, so it's blocked with a toast instead of opened.
const ALLOWED_EXTERNAL_SCHEMES = new Set(['http:', 'https:', 'mailto:']);

/**
 * Open an external href in the default browser, but only when its scheme is
 * on the allow-list. `new URL()` (rather than regex) does the parsing so
 * scheme-smuggling tricks ("java\tscript:", "%68ttp:") don't slip through.
 */
export function openExternalHref(href) {
  let url;
  try {
    url = new URL(href);
  } catch {
    toast.error(`Blocked malformed link: ${href}`);
    return;
  }
  if (!ALLOWED_EXTERNAL_SCHEMES.has(url.protocol)) {
    toast.error(`Blocked link with disallowed scheme: ${url.protocol}`);
    return;
  }
  shellOpen(url.href).catch((err) => toast.error(`Failed to open link: ${err}`));
}

/**
 * Delegated click handler for links inside rendered markdown: in-page anchors
 * keep native behaviour, external URLs open in the default browser (scheme
 * allow-listed to http/https/mailto), and local file paths (relative to
 * `baseDir`) open in an editor tab.
 */
export function handleMarkdownLinkClick(e, baseDir) {
  const anchor = e.target?.closest?.('a');
  if (!anchor) return;
  // SVG <a> elements may carry the target in xlink:href instead of href.
  const href =
    anchor.getAttribute('href') ||
    anchor.getAttributeNS?.('http://www.w3.org/1999/xlink', 'href');
  if (!href || href.startsWith('#')) return;

  e.preventDefault();
  e.stopPropagation();

  if (isExternalHref(href)) {
    openExternalHref(href);
    return;
  }

  const abs = resolveLocalPath(href, baseDir);
  if (abs) {
    useEditor.getState().openFile(abs);
  } else {
    toast.error(`Cannot resolve link target: ${href}`);
  }
}

// ── Clickable file paths in chat markdown ───────────────────────────────────
// The agent cites paths like `src/state/editor.js:176` (usually in inline
// code). linkifyFilePaths() marks path-looking strings in rendered HTML as
// clickable; openWorkspaceFile() resolves + stats them on click and opens an
// editor tab, navigating to the cited line when one is present.

// Extensions that make a bare filename (no separator, e.g. `package.json`)
// clickable. Separator-less strings otherwise stay inert so prose like
// `useEditor.getState` isn't misread as a path.
const KNOWN_EXTENSIONS = new Set([
  'js', 'jsx', 'ts', 'tsx', 'mjs', 'cjs', 'rs', 'py', 'rb', 'go', 'java',
  'kt', 'c', 'h', 'cpp', 'hpp', 'cc', 'cs', 'css', 'scss', 'less', 'html',
  'json', 'jsonc', 'toml', 'yaml', 'yml', 'md', 'txt', 'sql', 'sh', 'ps1',
  'bat', 'cmd', 'lock', 'conf', 'ini', 'env', 'xml', 'svg', 'vue', 'svelte',
  'swift', 'php', 'ex', 'exs', 'zig', 'lua', 'proto', 'graphql', 'prisma',
]);
const KNOWN_BARE_FILES = new Set(['dockerfile', 'makefile', 'justfile', 'rakefile', 'gemfile', 'procfile']);

// Whole-string test for inline code spans: optional root, segments, optional :line(:col).
const CODE_PATH_RE = /^(?:[A-Za-z]:[\\/]|\.{1,2}[\\/]|[\\/])?[\w\-.@+]+(?:[\\/][\w\-.@+ ]+)*(?::\d+(?::\d+)?)?$/;
// Embedded matches in plain text: at least one separator required.
const TEXT_PATH_RE = /(?:[A-Za-z]:[\\/]|\.{1,2}[\\/])?[\w\-.@+]+(?:[\\/][\w\-.@+]+)+(?::\d+(?::\d+)?)?/g;

/**
 * Parse a path-looking string into { path, line } or return null when it
 * doesn't qualify as a local file path.
 */
export function parsePathCandidate(raw, { requireSep = true } = {}) {
  if (!raw) return null;
  let text = raw.trim();
  if (!CODE_PATH_RE.test(text)) return null;
  const lineMatch = text.match(/:(\d+)(?::\d+)?$/);
  let line = null;
  if (lineMatch) {
    line = parseInt(lineMatch[1], 10);
    text = text.slice(0, -lineMatch[0].length);
  }
  if (!text || /[\\/]$/.test(text)) return null;
  const hasSep = /[\\/]/.test(text.replace(/^[A-Za-z]:/, ''));
  if (requireSep && !hasSep) return null;
  const segments = text.split(/[\\/]+/);
  const last = segments[segments.length - 1];
  if (!last) return null;
  const dot = last.lastIndexOf('.');
  const ext = dot > 0 ? last.slice(dot + 1).toLowerCase() : null;
  if (!hasSep) {
    if (!(ext && KNOWN_EXTENSIONS.has(ext)) && !KNOWN_BARE_FILES.has(last.toLowerCase())) return null;
  } else if (!ext || !/^\w{1,10}$/.test(ext)) {
    if (!KNOWN_BARE_FILES.has(last.toLowerCase())) return null;
  }
  return { path: text, line };
}

const FILE_LINK_CLASSES = 'cursor-pointer underline decoration-dotted underline-offset-2 hover:text-primary';

function markFileLink(el, candidate) {
  el.setAttribute('data-file-link', '1');
  el.setAttribute('data-path', candidate.path);
  if (candidate.line) el.setAttribute('data-line', String(candidate.line));
  el.setAttribute('title', 'Open in editor');
  el.className = `${el.className ? `${el.className} ` : ''}${FILE_LINK_CLASSES}`;
}

/**
 * Post-process sanitized markdown HTML: tag inline code spans and plain-text
 * runs that look like local file paths with data attributes so a delegated
 * click handler can open them in the editor.
 */
export function linkifyFilePaths(html) {
  if (!html) return html;
  const doc = new DOMParser().parseFromString(html, 'text/html');
  let changed = false;

  for (const code of doc.querySelectorAll('code')) {
    if (code.closest('pre, a')) continue;
    const candidate = parsePathCandidate(code.textContent, { requireSep: false });
    if (candidate) {
      markFileLink(code, candidate);
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
    let last = 0;
    let frag = null;
    for (const m of text.matchAll(TEXT_PATH_RE)) {
      const candidate = parsePathCandidate(m[0]);
      if (!candidate) continue;
      frag ??= doc.createDocumentFragment();
      if (m.index > last) frag.appendChild(doc.createTextNode(text.slice(last, m.index)));
      const span = doc.createElement('span');
      span.textContent = m[0];
      markFileLink(span, candidate);
      frag.appendChild(span);
      last = m.index + m[0].length;
    }
    if (frag) {
      if (last < text.length) frag.appendChild(doc.createTextNode(text.slice(last)));
      node.parentNode.replaceChild(frag, node);
      changed = true;
    }
  }

  return changed ? doc.body.innerHTML : html;
}

/** Stat a local path, normalizing the desktop tuple / web object shapes. */
async function statLocalPath(path) {
  try {
    const s = await invoke('stat_path', { path });
    if (!s) return { exists: false, isDir: false };
    if (Array.isArray(s)) return { exists: true, isDir: !!s[1] };
    return { exists: !!s.exists, isDir: !!s.isDir };
  } catch {
    return { exists: false, isDir: false };
  }
}

/** Find workspace files whose path ends with the cited (possibly bare) path. */
async function findWorkspaceMatches(path, baseDir) {
  if (!baseDir) return [];
  try {
    const files = await invoke('list_project_files', { rootPath: baseDir, maxFiles: 5000 });
    if (!Array.isArray(files)) return [];
    const needle = path.replace(/\\/g, '/').toLowerCase();
    return files.filter((f) => {
      const norm = f.replace(/\\/g, '/').toLowerCase();
      return norm === needle || norm.endsWith(`/${needle}`);
    });
  } catch {
    return [];
  }
}

/**
 * Resolve a cited file path (relative paths against `baseDir`), verify it
 * exists, and open it in an editor tab — jumping to `line` when given.
 */
export async function openWorkspaceFile(path, line, baseDir) {
  const abs = resolveLocalPath(path, baseDir);
  if (!abs) {
    toast.error(`Cannot resolve path: ${path}`);
    return;
  }
  const stat = await statLocalPath(abs);
  if (!stat.exists) {
    const matches = await findWorkspaceMatches(path, baseDir);
    if (matches.length === 1) {
      useEditor.getState().openFile(matches[0], line ? { line } : null);
      return;
    }
    if (matches.length > 1) {
      openFilePalette(path.replace(/\\/g, '/'));
      return;
    }
    toast.error(`File not found: ${path}`);
    return;
  }
  if (stat.isDir) {
    toast.error(`Path is a folder: ${path}`);
    return;
  }
  useEditor.getState().openFile(abs, line ? { line } : null);
}
