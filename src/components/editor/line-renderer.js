import { el } from '../../utils/dom.js';

/**
 * Render a single line with syntax highlighting spans.
 * @param {Object} renderedLine - { line_number, text, spans: [{ start_col, end_col, highlight_class }] }
 * @returns {HTMLElement}
 */
export function renderLine(renderedLine) {
  const container = el('div', { class: 'editor-line' });

  const { text, spans } = renderedLine;

  if (!spans || spans.length === 0) {
    // No highlighting — render plain text
    container.appendChild(document.createTextNode(text || ' '));
    return container;
  }

  // Build spans covering the entire line
  let lastCol = 0;
  for (const span of spans) {
    // Gap before this span (unhighlighted text)
    if (span.start_col > lastCol) {
      const gapText = text.substring(lastCol, span.start_col);
      container.appendChild(document.createTextNode(gapText));
    }

    // Highlighted span
    const spanText = text.substring(span.start_col, span.end_col);
    if (spanText) {
      const spanEl = el('span', { class: `token-${span.highlight_class}` }, spanText);
      container.appendChild(spanEl);
    }

    lastCol = Math.max(lastCol, span.end_col);
  }

  // Remaining text after last span
  if (lastCol < text.length) {
    container.appendChild(document.createTextNode(text.substring(lastCol)));
  }

  // Empty line
  if (text.length === 0) {
    container.appendChild(document.createTextNode(' '));
  }

  return container;
}
