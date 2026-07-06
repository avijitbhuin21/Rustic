// Shared helpers for markdown surfaces (file preview, agent chat) that let
// rendered HTML reference LOCAL workspace files: `![img](./shot.png)` or a
// link to `./docs/other.md`. Local image paths are rewritten to a URL the
// webview can actually load (Tauri asset protocol on desktop, `/api/asset`
// on web — both come from `convertFileSrc`, which the web build aliases to
// the HTTP shim). Local link clicks open the target file in an editor tab.
import { convertFileSrc } from '@tauri-apps/api/core';
import { open as shellOpen } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import { useEditor } from '@/state/editor';

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
