import { el } from '../../utils/dom.js';

/**
 * Render the gutter (line numbers) for visible lines.
 * @param {Array} lines - rendered lines with line_number
 * @param {number} activeLine - current cursor line (1-based)
 * @returns {HTMLElement}
 */
export function renderGutter(lines, activeLine) {
  const gutter = el('div', { class: 'editor-gutter' });

  for (const line of lines) {
    const isActive = line.line_number === activeLine;
    const num = el('div', {
      class: `editor-gutter__line ${isActive ? 'editor-gutter__line--active' : ''}`,
    }, String(line.line_number));
    gutter.appendChild(num);
  }

  return gutter;
}
