import { el } from '../utils/dom.js';

/**
 * Show a themed confirmation dialog. Replaces native window.confirm so all
 * dialogs share the app's look-and-feel.
 *
 * @param {string} title
 * @param {string} message
 * @param {object} [options]
 * @param {string} [options.confirmLabel] Text on the confirm button. Defaults to `title` (preserves the legacy "Delete" / "Delete" pairing for callers that pass a verb as the title).
 * @param {string} [options.cancelLabel='Cancel']
 * @param {boolean} [options.danger=true] Style the confirm button as a destructive action. Defaults to true so existing destructive callers don't regress; pass `false` for benign confirms.
 * @returns {Promise<boolean>} true if confirmed, false if cancelled.
 */
export function showConfirmDialog(title, message, options = {}) {
  const {
    confirmLabel = title,
    cancelLabel = 'Cancel',
    danger = true,
  } = options;

  return new Promise((resolve) => {
    let resolved = false;

    function finish(result) {
      if (resolved) return;
      resolved = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve(result);
    }

    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        finish(false);
      } else if (e.key === 'Enter') {
        e.preventDefault();
        finish(true);
      }
    }

    const overlay = el('div', { class: 'confirm-dialog-overlay' });
    const dialog = el('div', { class: 'confirm-dialog' });

    const titleEl = el('div', { class: 'confirm-dialog__title' }, title);
    const messageEl = el('div', { class: 'confirm-dialog__message' }, message);

    const actions = el('div', { class: 'confirm-dialog__actions' });

    const cancelBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--cancel' }, cancelLabel);
    const confirmBtn = el('button', {
      class: `confirm-dialog__btn ${danger ? 'confirm-dialog__btn--discard' : 'confirm-dialog__btn--save'}`,
    }, confirmLabel);

    cancelBtn.addEventListener('click', () => finish(false));
    confirmBtn.addEventListener('click', () => finish(true));

    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);

    dialog.appendChild(titleEl);
    dialog.appendChild(messageEl);
    dialog.appendChild(actions);

    overlay.appendChild(dialog);
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) finish(false);
    });

    document.body.appendChild(overlay);
    document.addEventListener('keydown', onKey);

    cancelBtn.focus();
  });
}

/**
 * Themed equivalent of native window.alert — single OK button.
 * @returns {Promise<void>} resolves when dismissed.
 */
export function showAlertDialog(title, message) {
  return new Promise((resolve) => {
    let resolved = false;
    function finish() {
      if (resolved) return;
      resolved = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve();
    }
    function onKey(e) {
      if (e.key === 'Escape' || e.key === 'Enter') {
        e.preventDefault();
        finish();
      }
    }

    const overlay = el('div', { class: 'confirm-dialog-overlay' });
    const dialog = el('div', { class: 'confirm-dialog' });
    dialog.appendChild(el('div', { class: 'confirm-dialog__title' }, title));
    dialog.appendChild(el('div', { class: 'confirm-dialog__message' }, message));
    const actions = el('div', { class: 'confirm-dialog__actions' });
    const okBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--save' }, 'OK');
    okBtn.addEventListener('click', finish);
    actions.appendChild(okBtn);
    dialog.appendChild(actions);
    overlay.appendChild(dialog);
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) finish();
    });
    document.body.appendChild(overlay);
    document.addEventListener('keydown', onKey);
    okBtn.focus();
  });
}

/**
 * Show a confirmation dialog for unsaved changes.
 * Returns a promise that resolves to 'save', 'discard', or 'cancel'.
 */
export function showUnsavedDialog(fileName) {
  return new Promise((resolve) => {
    let resolved = false;

    function finish(result) {
      if (resolved) return;
      resolved = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve(result);
    }

    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        finish('cancel');
      } else if (e.key === 'Enter') {
        e.preventDefault();
        finish('save');
      }
    }

    const overlay = el('div', { class: 'confirm-dialog-overlay' });

    const dialog = el('div', { class: 'confirm-dialog' });

    const title = el('div', { class: 'confirm-dialog__title' }, 'Unsaved Changes');
    const message = el('div', { class: 'confirm-dialog__message' },
      `Do you want to save the changes you made to ${fileName}?`);
    const subtitle = el('div', { class: 'confirm-dialog__subtitle' },
      'Your changes will be lost if you don\'t save them.');

    const actions = el('div', { class: 'confirm-dialog__actions' });

    const cancelBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--cancel' }, 'Cancel');
    const discardBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--discard' }, 'Don\'t Save');
    const saveBtn = el('button', { class: 'confirm-dialog__btn confirm-dialog__btn--save' }, 'Save');

    cancelBtn.addEventListener('click', () => finish('cancel'));
    discardBtn.addEventListener('click', () => finish('discard'));
    saveBtn.addEventListener('click', () => finish('save'));

    actions.appendChild(cancelBtn);
    actions.appendChild(discardBtn);
    actions.appendChild(saveBtn);

    dialog.appendChild(title);
    dialog.appendChild(message);
    dialog.appendChild(subtitle);
    dialog.appendChild(actions);

    overlay.appendChild(dialog);
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) finish('cancel');
    });

    document.body.appendChild(overlay);
    document.addEventListener('keydown', onKey);

    saveBtn.focus();
  });
}
