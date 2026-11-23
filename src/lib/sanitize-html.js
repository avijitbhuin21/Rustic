// The one DOMPurify policy for HTML that Rustic did not write itself: rendered
// markdown (agent output, .md files, skill bodies) and notebook HTML output.
// DOMPurify's defaults keep forms, form controls, <style> and inline styles,
// which is enough to submit the window to another origin, draw over the app,
// or fetch a URL through CSS. Everything injected with dangerouslySetInnerHTML
// from those sources must come through here.
import createDOMPurify from 'dompurify';

const FORBID_TAGS = [
  'form',
  'button',
  'select',
  'option',
  'optgroup',
  'datalist',
  'textarea',
  'style',
  'dialog',
  'map',
  'area',
];

const FORBID_ATTR = [
  'style',
  'action',
  'formaction',
  'form',
  'autofocus',
  'ping',
  'background',
  'usemap',
  'contenteditable',
];

const CONFIG = { FORBID_TAGS, FORBID_ATTR };

// Mermaid draws with a <style> block, inline styles and HTML labels inside
// <foreignObject>; the rest of the policy still applies to its output.
const DIAGRAM_CONFIG = {
  FORBID_TAGS: FORBID_TAGS.filter((t) => t !== 'style'),
  FORBID_ATTR: FORBID_ATTR.filter((a) => a !== 'style'),
  ADD_TAGS: ['foreignobject'],
  ADD_ATTR: ['dominant-baseline'],
  HTML_INTEGRATION_POINTS: { foreignobject: true },
};

const SVG_FILE_CONFIG = { USE_PROFILES: { svg: true, svgFilters: true } };

/** Resolve CSS escapes (`\72 ` is `r`, `\(` is `(`) so a pattern sees what the CSS parser sees. */
export function decodeCssEscapes(css) {
  return css.replace(/\\([0-9a-f]{1,6})[ \t\n\r\f]?|\\([^\n\r\f])/gi, (_, hex, ch) => {
    if (ch !== undefined) return ch;
    const code = parseInt(hex, 16);
    return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : '\ufffd';
  });
}

/**
 * Remove everything in a piece of CSS that can fetch: `@import`, `image-set()`
 * and every `url()` that is not a same-document `#fragment`. CSS that has none
 * of them is returned untouched.
 */
export function stripCssFetches(css) {
  if (!css) return css;
  const flat = decodeCssEscapes(css);
  const cleaned = flat
    .replace(/@import\b[^;]*;?/gi, '')
    .replace(/(?:-webkit-)?image-set\([^;}]*/gi, 'none')
    .replace(/url\(\s*(['"]?)([^'")]*)\1\s*\)?/gi, (match, _quote, target) =>
      target.trim().startsWith('#') ? match : 'none',
    );
  return cleaned === flat ? css : cleaned;
}

// Attributes that make the browser fetch a URL without a click.
const FETCHING_ATTRS = ['src', 'poster', 'href', 'xlink:href'];

const NETWORK_PROTOCOLS = new Set(['http:', 'https:', 'ws:', 'wss:', 'ftp:']);

/**
 * True for a URL that leaves the app's own origin over the network. Resolved
 * with the URL parser, not a pattern: `https:\\host` and `https:host` are
 * network URLs too.
 */
export function isRemoteUrl(value) {
  if (!value) return false;
  let url;
  try {
    url = new URL(value, window.location.href);
  } catch {
    return false;
  }
  if (!NETWORK_PROTOCOLS.has(url.protocol)) return false;
  return url.origin !== window.location.origin;
}

function remoteHost(url) {
  try {
    return new URL(url, window.location.href).host;
  } catch {
    return '';
  }
}

/** Swap a remote <img> for a placeholder the user has to click to load. */
function blockRemoteImage(img, src) {
  const doc = img.ownerDocument;
  const placeholder = doc.createElement('span');
  placeholder.className = 'remote-image-blocked';
  placeholder.setAttribute('role', 'button');
  placeholder.setAttribute('tabindex', '0');
  placeholder.setAttribute('data-remote-image', src);
  const alt = img.getAttribute('alt');
  if (alt) placeholder.setAttribute('data-remote-alt', alt);
  placeholder.setAttribute('title', src);
  const host = remoteHost(src);
  placeholder.textContent = host ? `Load image from ${host}` : 'Load remote image';
  img.replaceWith(placeholder);
}

function makePurifier({ blockRemote }) {
  const purifier = createDOMPurify(window);
  purifier.addHook('afterSanitizeAttributes', (node) => {
    if (!node.getAttribute) return;

    // GFM task lists render as checkboxes; no other input is kept.
    if (node.tagName === 'INPUT') {
      if ((node.getAttribute('type') || '').toLowerCase() !== 'checkbox') {
        node.remove();
        return;
      }
      node.setAttribute('disabled', '');
      node.removeAttribute('name');
      node.removeAttribute('value');
    }

    if (!blockRemote) return;

    // A candidate list is not worth parsing: no markdown renderer emits one.
    node.removeAttribute('srcset');

    if (node.tagName === 'IMG') {
      const src = node.getAttribute('src') || '';
      if (isRemoteUrl(src)) {
        blockRemoteImage(node, src);
        return;
      }
    }
    // Links are followed only on a click; every other remote reference would
    // be fetched as soon as the HTML is inserted.
    if (node.tagName !== 'A') {
      for (const attr of FETCHING_ATTRS) {
        if (node.hasAttribute(attr) && isRemoteUrl(node.getAttribute(attr))) {
          node.removeAttribute(attr);
        }
      }
    }
    // Inline styles and paint attributes (fill, mask, filter) survive only in
    // diagrams; a path in a title or href is not CSS and is left alone.
    for (const attr of Array.from(node.attributes || [])) {
      if (attr.name === 'style' || /^\s*u/i.test(attr.value) || attr.value.includes('\\')) {
        if (attr.name !== 'style' && !/^\s*url\(/i.test(decodeCssEscapes(attr.value))) continue;
        const cleaned = stripCssFetches(attr.value);
        if (cleaned !== attr.value) node.setAttribute(attr.name, cleaned);
      }
    }
  });
  if (blockRemote) {
    purifier.addHook('uponSanitizeElement', (node) => {
      if (node.nodeName && node.nodeName.toLowerCase() === 'style') {
        node.textContent = stripCssFetches(node.textContent);
      }
    });
  }
  return purifier;
}

let filePurifier = null;
let agentPurifier = null;
let svgPurifier = null;

/**
 * Sanitize HTML from a file the user opened (markdown preview, notebook).
 * Remote images still load, as they do in any README viewer.
 */
export function sanitizeHtml(html) {
  if (!html) return '';
  filePurifier ??= makePurifier({ blockRemote: false });
  return filePurifier.sanitize(html, CONFIG);
}

/**
 * Sanitize HTML rendered from model output. Nothing remote is fetched on
 * render: text an injected prompt put in an image URL would otherwise leave
 * the machine without a click. Remote images become click-to-load placeholders.
 */
export function sanitizeAgentHtml(html) {
  if (!html) return '';
  agentPurifier ??= makePurifier({ blockRemote: true });
  return agentPurifier.sanitize(html, CONFIG);
}

/**
 * Sanitize the SVG a diagram renderer produced. With `agent` the source was
 * model output, so nothing remote is fetched: remote images become
 * click-to-load placeholders and CSS loses every `url()` that leaves the page.
 */
export function sanitizeDiagramSvg(svg, { agent = false } = {}) {
  if (!svg) return '';
  if (agent) {
    agentPurifier ??= makePurifier({ blockRemote: true });
    return agentPurifier.sanitize(svg, DIAGRAM_CONFIG);
  }
  filePurifier ??= makePurifier({ blockRemote: false });
  return filePurifier.sanitize(svg, DIAGRAM_CONFIG);
}

/** Sanitize an .svg file the user opened: SVG elements and filters only, no scripts or HTML. */
export function sanitizeSvgFile(text) {
  if (!text) return '';
  svgPurifier ??= createDOMPurify(window);
  return svgPurifier.sanitize(text, SVG_FILE_CONFIG);
}

/** Replace a blocked-image placeholder with the image it stands for. */
function loadBlockedImage(placeholder) {
  const src = placeholder.getAttribute('data-remote-image');
  if (!isRemoteUrl(src)) return;
  const img = placeholder.ownerDocument.createElement('img');
  img.setAttribute('src', src);
  img.setAttribute('alt', placeholder.getAttribute('data-remote-alt') || '');
  img.setAttribute('referrerpolicy', 'no-referrer');
  placeholder.replaceWith(img);
}

/** Install the click and keyboard handlers that load a blocked image on request. */
export function installRemoteImageLoader() {
  const activate = (e) => {
    const placeholder = e.target?.closest?.('[data-remote-image]');
    if (!placeholder) return;
    if (e.type === 'keydown' && e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    e.stopPropagation();
    loadBlockedImage(placeholder);
  };
  document.addEventListener('click', activate, true);
  document.addEventListener('keydown', activate, true);
}
