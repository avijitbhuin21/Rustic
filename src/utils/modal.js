import { el, icon } from './dom.js';

/**
 * Open a lightweight modal. Returns a `close()` function.
 * The modal is appended to document.body and removed on close.
 *
 * @param {Object} opts
 * @param {string} opts.title
 * @param {HTMLElement|string} opts.body
 * @param {Array<{label:string, variant?:string, onClick?:Function}>} [opts.buttons]
 * @param {string} [opts.size]   — '' | 'sm' | 'lg'
 * @param {Function} [opts.onClose]
 */
export function openModal({ title, body, buttons = [], size = '', onClose = null }) {
  const backdrop = el('div', { class: 'rustic-modal-backdrop' });
  const modal = el('div', { class: `rustic-modal${size ? ` rustic-modal--${size}` : ''}` });

  const header = el('div', { class: 'rustic-modal__header' });
  header.appendChild(el('div', { class: 'rustic-modal__title' }, title || ''));
  const closeBtn = el('button', { class: 'rustic-modal__close', title: 'Close' });
  closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 14));
  header.appendChild(closeBtn);
  modal.appendChild(header);

  const bodyEl = el('div', { class: 'rustic-modal__body' });
  if (typeof body === 'string') bodyEl.textContent = body;
  else if (body) bodyEl.appendChild(body);
  modal.appendChild(bodyEl);

  let close;
  const btnRefs = [];

  if (buttons && buttons.length) {
    const footer = el('div', { class: 'rustic-modal__footer' });
    for (const b of buttons) {
      const btn = el('button', {
        class: `rustic-modal__btn${b.variant ? ` rustic-modal__btn--${b.variant}` : ''}`,
      }, b.label);
      btn.addEventListener('click', async () => {
        if (b.onClick) {
          const result = await b.onClick();
          if (result === false) return;
        }
        close();
      });
      footer.appendChild(btn);
      btnRefs.push(btn);
    }
    modal.appendChild(footer);
  }

  backdrop.appendChild(modal);
  document.body.appendChild(backdrop);

  close = () => {
    if (!backdrop.parentNode) return;
    backdrop.parentNode.removeChild(backdrop);
    document.removeEventListener('keydown', onKey);
    if (onClose) onClose();
  };

  const onKey = (e) => { if (e.key === 'Escape') close(); };
  document.addEventListener('keydown', onKey);

  closeBtn.addEventListener('click', close);
  backdrop.addEventListener('click', (e) => { if (e.target === backdrop) close(); });

  close.buttons = btnRefs;
  return close;
}
