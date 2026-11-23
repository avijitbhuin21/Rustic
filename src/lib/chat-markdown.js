// Chat markdown rendered one top-level block at a time.
//
// A streaming reply is re-rendered on every flush. Rendering it as a single
// document means lexing, parsing, sanitising and linkifying the whole reply
// each time, although only its last block changed. Here the reply is lexed
// once, and each top-level block (paragraph, list, code fence, table, …) is
// rendered and cached on its own source text, so a flush pays for the block
// that is still growing and nothing else. Settled messages go through the
// same path, so a block looks identical while streaming and afterwards.
import { marked } from 'marked';
import { sanitizeAgentHtml } from '@/lib/sanitize-html';
import { linkifyFilePaths } from '@/lib/file-links';

const OPTIONS = { breaks: true, gfm: true };

// Rendered HTML per block, keyed by the block's markdown source.
const BLOCK_CACHE = new Map();
const BLOCK_CACHE_MAX = 4000;

// Whole-document renders, for the documents that cannot be split (see `splittable`).
const DOCUMENT_CACHE = new Map();
const DOCUMENT_CACHE_MAX = 40;

/** Read `key`, marking it most recently used. */
function cacheGet(cache, key) {
  const hit = cache.get(key);
  if (hit === undefined) return undefined;
  cache.delete(key);
  cache.set(key, hit);
  return hit;
}

/** Store `key`, dropping the least recently used entry once `max` is reached. */
function cacheSet(cache, max, key, value) {
  if (cache.size >= max) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  cache.set(key, value);
}

/** Sanitise parser output and tag file paths, the same way for a block or a document. */
function finish(html) {
  return linkifyFilePaths(sanitizeAgentHtml(html));
}

/**
 * Whether each top-level token renders the same alone as inside the document.
 * Not when the document defines reference links (a definition changes how
 * other blocks render), has raw HTML blocks (an element may open in one block
 * and close in another), or has bare top-level text (the parser merges runs
 * of it into one paragraph).
 */
function splittable(tokens) {
  if (tokens.links && Object.keys(tokens.links).length > 0) return false;
  return !tokens.some((t) => t.type === 'html' || t.type === 'text' || t.type === 'def');
}

/**
 * Render chat markdown to a list of sanitised HTML strings, one per top-level
 * block, in order. A document that cannot be split comes back as one string.
 */
export function renderChatMarkdown(text) {
  if (!text) return [];
  let tokens;
  try {
    tokens = marked.lexer(text, OPTIONS);
  } catch {
    return [sanitizeAgentHtml(text)];
  }

  if (!splittable(tokens)) {
    const cached = cacheGet(DOCUMENT_CACHE, text);
    if (cached !== undefined) return [cached];
    let html;
    try {
      html = finish(marked.parser(tokens, OPTIONS));
    } catch {
      html = sanitizeAgentHtml(text);
    }
    cacheSet(DOCUMENT_CACHE, DOCUMENT_CACHE_MAX, text, html);
    return [html];
  }

  const blocks = tokens.filter((t) => t.type !== 'space');
  const out = [];
  for (let i = 0; i < blocks.length; i++) {
    const token = blocks[i];
    let html = cacheGet(BLOCK_CACHE, token.raw);
    if (html === undefined) {
      try {
        html = finish(marked.parser([token], OPTIONS));
      } catch {
        html = sanitizeAgentHtml(token.raw);
      }
      // The last block may still be growing: caching it would store one entry
      // per streamed prefix and push settled blocks out.
      if (i < blocks.length - 1) cacheSet(BLOCK_CACHE, BLOCK_CACHE_MAX, token.raw, html);
    }
    out.push(html);
  }
  return out;
}
