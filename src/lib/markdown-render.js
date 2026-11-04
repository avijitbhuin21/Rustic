// Shared markdown → sanitized HTML renderer (editor .md preview, skill bodies).
// One configured `marked` instance with highlight.js for fenced code; output is
// always run through DOMPurify. Style the result with the `rustic-markdown` class.
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js/lib/common';
import DOMPurify from 'dompurify';
import 'highlight.js/styles/github-dark.css';

// Created once at module load so the lexer + hooks aren't rebuilt per render.
// Only highlight.js's "common" subset is registered to keep the bundle small.
const md = new Marked(
  markedHighlight({
    emptyLangClass: 'hljs',
    langPrefix: 'hljs language-',
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext';
      try {
        return hljs.highlight(code, { language, ignoreIllegals: true }).value;
      } catch {
        return code;
      }
    },
  }),
  { gfm: true, breaks: true },
);

/** Render markdown `text` to sanitized HTML (GFM checkboxes and hljs classes kept). */
export function renderMarkdown(text) {
  if (!text) return '';
  const raw = md.parse(text);
  return DOMPurify.sanitize(raw, {
    ADD_TAGS: ['input'],
    ADD_ATTR: ['type', 'checked', 'disabled', 'class'],
  });
}
