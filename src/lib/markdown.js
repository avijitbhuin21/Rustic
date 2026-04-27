// Markdown rendering wrapper.
//
// Output of marked.parse() is HTML produced from untrusted input (LLM
// responses, MCP tool results, file contents shown via markdown preview, git
// commit messages). We always pipe it through DOMPurify before assigning to
// innerHTML so a prompt-injected `<img onerror>` cannot reach Tauri commands
// even if a future CSP regression lets script tags through.
//
// IMPORTANT: callers should use renderMarkdown(text) or renderMarkdownInline(text)
// instead of marked.parse(text). Direct use of marked.parse + innerHTML is
// effectively an XSS sink in this app.

import { marked } from 'marked';
import DOMPurify from 'dompurify';

// GFM line-break behavior: chat-view used marked.parse(text, { breaks: true,
// gfm: true }) at the call site. Set them as the global default so renderMarkdown
// stays simple.
marked.setOptions({ breaks: true, gfm: true });

// Allow common markdown output but strip:
// - <script>, <iframe>, <object>, <embed>, etc. (DOMPurify defaults)
// - Inline event handlers like onclick, onerror (DOMPurify defaults)
// - target=_blank without rel=noopener (we patch below)
//
// We allow class attributes so syntax-highlight CSS classes survive.
const PURIFY_OPTS = {
  USE_PROFILES: { html: true },
  ADD_ATTR: ['target', 'rel'],
};

// Force every <a target="_blank"> to also carry rel="noopener noreferrer".
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A' && node.getAttribute('target') === '_blank') {
    node.setAttribute('rel', 'noopener noreferrer');
  }
});

export function renderMarkdown(text) {
  if (text == null) return '';
  const raw = typeof text === 'string' ? text : String(text);
  const html = marked.parse(raw);
  return DOMPurify.sanitize(html, PURIFY_OPTS);
}

export function renderMarkdownInline(text) {
  if (text == null) return '';
  const raw = typeof text === 'string' ? text : String(text);
  // marked.parseInline avoids wrapping in a top-level <p>.
  const html = marked.parseInline(raw);
  return DOMPurify.sanitize(html, PURIFY_OPTS);
}

// Re-export marked for advanced cases (configuration, lexer, etc.) but the
// vast majority of call sites should use the wrappers above.
export { marked };
