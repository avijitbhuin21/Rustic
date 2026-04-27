import { el } from '../../utils/dom.js';

// Module-level config — updated via setRendererConfig()
let rendererConfig = {
  render_whitespace: 'none',
  show_zero_width: false,
  bracket_pair_colorization: false,
};

export function setRendererConfig(config) {
  rendererConfig = { ...rendererConfig, ...config };
}

// Zero-width characters to detect
const ZERO_WIDTH_RE = /[\u200B\u200C\u200D\uFEFF\u200E\u200F\u2060\u2028\u2029]/;
const ZERO_WIDTH_NAMES = {
  '\u200B': 'ZWSP',
  '\u200C': 'ZWNJ',
  '\u200D': 'ZWJ',
  '\uFEFF': 'BOM',
  '\u200E': 'LRM',
  '\u200F': 'RLM',
  '\u2060': 'WJ',
  '\u2028': 'LS',
  '\u2029': 'PS',
};

const BRACKET_OPEN = new Set(['(', '[', '{']);
const BRACKET_CLOSE = new Set([')', ']', '}']);
const BRACKET_COLOR_COUNT = 4;

/**
 * Render a single line with syntax highlighting spans.
 * @param {Object} renderedLine - { line_number, text, spans: [{ start_col, end_col, highlight_class }] }
 * @returns {HTMLElement}
 */
export function renderLine(renderedLine) {
  // The data-line attribute lets the click/hover hit-test resolve the
  // logical line number from the DOM directly (via e.target.closest), which
  // is more robust than geometry math when word-wrap is on or when the
  // virtualization tracker is mid-update.
  const container = el('div', {
    class: 'editor-line',
    'data-line': String(renderedLine.line_number),
  });
  const { text, spans } = renderedLine;

  if (!text || text.length === 0) {
    container.appendChild(document.createTextNode(' '));
    return container;
  }

  // Build segments: array of { text, className? } covering the entire line
  const segments = buildSegments(text, spans);

  // Apply whitespace / zero-width / bracket decorations and append to container
  const wsMode = rendererConfig.render_whitespace;
  const showZW = rendererConfig.show_zero_width;
  const bracketColor = rendererConfig.bracket_pair_colorization;
  let bracketDepth = 0;

  // Precompute leading/trailing whitespace boundaries for 'boundary' mode
  let leadingEnd = 0;
  let trailingStart = text.length;
  if (wsMode === 'boundary') {
    while (leadingEnd < text.length && (text[leadingEnd] === ' ' || text[leadingEnd] === '\t')) leadingEnd++;
    while (trailingStart > leadingEnd && (text[trailingStart - 1] === ' ' || text[trailingStart - 1] === '\t')) trailingStart--;
  }

  let charIndex = 0;
  for (const seg of segments) {
    const needsDecoration = wsMode !== 'none' || showZW || bracketColor;

    if (!needsDecoration) {
      // Fast path: no decorations needed
      if (seg.className) {
        container.appendChild(el('span', { class: seg.className }, seg.text));
      } else {
        container.appendChild(document.createTextNode(seg.text));
      }
      charIndex += seg.text.length;
      continue;
    }

    // Slow path: character-by-character decoration within each segment
    let buf = '';
    const flushBuf = () => {
      if (!buf) return;
      if (seg.className) {
        container.appendChild(el('span', { class: seg.className }, buf));
      } else {
        container.appendChild(document.createTextNode(buf));
      }
      buf = '';
    };

    for (let i = 0; i < seg.text.length; i++) {
      const ch = seg.text[i];
      const globalIdx = charIndex + i;

      // Zero-width character detection
      if (showZW && ZERO_WIDTH_RE.test(ch)) {
        flushBuf();
        const label = ZERO_WIDTH_NAMES[ch] || 'ZW';
        const codePoint = 'U+' + ch.charCodeAt(0).toString(16).toUpperCase().padStart(4, '0');
        const zwEl = el('span', { class: 'zero-width-char', title: `${label} (${codePoint})` }, label);
        container.appendChild(zwEl);
        continue;
      }

      // Whitespace rendering
      const shouldShowWS = wsMode === 'all' ||
        (wsMode === 'boundary' && (globalIdx < leadingEnd || globalIdx >= trailingStart));

      if (shouldShowWS && ch === ' ') {
        flushBuf();
        container.appendChild(el('span', { class: 'ws-space' }, '\u00B7'));
        continue;
      }
      if (shouldShowWS && ch === '\t') {
        flushBuf();
        container.appendChild(el('span', { class: 'ws-tab' }, '\u2192\t'));
        continue;
      }

      // Bracket pair colorization
      if (bracketColor && BRACKET_OPEN.has(ch)) {
        flushBuf();
        const colorClass = `bracket-color-${bracketDepth % BRACKET_COLOR_COUNT}`;
        const wrapper = seg.className
          ? el('span', { class: `${seg.className} ${colorClass}` }, ch)
          : el('span', { class: colorClass }, ch);
        container.appendChild(wrapper);
        bracketDepth++;
        continue;
      }
      if (bracketColor && BRACKET_CLOSE.has(ch)) {
        flushBuf();
        bracketDepth = Math.max(0, bracketDepth - 1);
        const colorClass = `bracket-color-${bracketDepth % BRACKET_COLOR_COUNT}`;
        const wrapper = seg.className
          ? el('span', { class: `${seg.className} ${colorClass}` }, ch)
          : el('span', { class: colorClass }, ch);
        container.appendChild(wrapper);
        continue;
      }

      buf += ch;
    }

    flushBuf();
    charIndex += seg.text.length;
  }

  return container;
}

/**
 * Build flat segment array from text and syntax spans.
 * Each segment: { text: string, className?: string }
 */
function buildSegments(text, spans) {
  if (!spans || spans.length === 0) {
    return [{ text }];
  }

  // Tree-sitter regularly emits overlapping captures for the same range —
  // e.g. a JSON object key gets BOTH `string` and `property`. The previous
  // implementation walked spans in order and emitted each one's text
  // verbatim, which produced visible duplicates like `"name""name"`.
  //
  // Fix: paint per-character classes into a flat array (later spans
  // overwrite earlier ones, matching tree-sitter's standard "more specific
  // captures come later" convention), then group consecutive same-class
  // characters into segments.
  const cls = new Array(text.length).fill(null);
  for (const span of spans) {
    const cn = `token-${span.highlight_class}`;
    const start = Math.max(0, span.start_col);
    const end = Math.min(text.length, span.end_col);
    for (let i = start; i < end; i++) cls[i] = cn;
  }

  const segments = [];
  let i = 0;
  while (i < text.length) {
    const c = cls[i];
    let j = i + 1;
    while (j < text.length && cls[j] === c) j++;
    if (c === null) segments.push({ text: text.substring(i, j) });
    else segments.push({ text: text.substring(i, j), className: c });
    i = j;
  }
  return segments;
}
