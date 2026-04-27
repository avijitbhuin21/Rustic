// Lightweight loading-skeleton helpers. Use instead of leaving a panel
// blank during async loads — gives the user immediate feedback that
// something is happening and prevents the "is the app frozen?" question.

import { el } from '../utils/dom.js';

/**
 * Create a skeleton block of `count` shimmer rows. Each row is sized via
 * the `widths` array (percentages or px); cycles if shorter than `count`.
 */
export function createSkeletonRows(count = 5, widths = ['85%', '60%', '92%', '70%']) {
  const wrapper = el('div', { class: 'skeleton', 'aria-hidden': 'true' });
  for (let i = 0; i < count; i++) {
    const w = widths[i % widths.length];
    wrapper.appendChild(el('div', {
      class: 'skeleton__row',
      style: { width: w },
    }));
  }
  return wrapper;
}

/**
 * Create an empty-state placeholder. Use when a panel has no data to show
 * (and is not loading) so the user understands the absence is intentional.
 */
export function createEmptyState(title, body, action) {
  const wrapper = el('div', { class: 'empty-state' });
  if (title) wrapper.appendChild(el('div', { class: 'empty-state__title' }, title));
  if (body) wrapper.appendChild(el('div', { class: 'empty-state__body' }, body));
  if (action && action.label && typeof action.onClick === 'function') {
    const btn = el('button', { class: 'empty-state__action' }, action.label);
    btn.addEventListener('click', action.onClick);
    wrapper.appendChild(btn);
  }
  return wrapper;
}

/**
 * Create an error-state placeholder with an optional retry button.
 */
export function createErrorState(title, body, onRetry) {
  const wrapper = el('div', { class: 'empty-state empty-state--error' });
  wrapper.appendChild(el('div', { class: 'empty-state__title' }, title || 'Something went wrong'));
  if (body) wrapper.appendChild(el('div', { class: 'empty-state__body' }, body));
  if (typeof onRetry === 'function') {
    const btn = el('button', { class: 'empty-state__action' }, 'Retry');
    btn.addEventListener('click', onRetry);
    wrapper.appendChild(btn);
  }
  return wrapper;
}
