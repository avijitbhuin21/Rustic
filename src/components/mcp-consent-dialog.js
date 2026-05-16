// MCP project-scope consent dialog (F-10).
//
// Project `.mcp.json` files name child processes that the agent spawns
// every turn — a hostile entry is effectively RCE. This dialog blocks the
// auto-load until the user has reviewed the exact byte sequence and clicked
// Approve. Consent is keyed on a SHA-256 of the file content, so any
// modification re-triggers the prompt.

import { el } from '../utils/dom.js';
import { trapFocus } from './confirm-dialog.js';
import * as api from '../lib/tauri-api.js';

// At most one consent dialog open at a time per (projectPath, contentHash).
// The backend emits the event again on every chat turn if consent is still
// missing — without this guard we'd stack modals.
const openKeys = new Set();

export function showMcpConsentDialog({ projectPath, contentHash, content, projectId }) {
  const key = `${projectPath}|${contentHash}`;
  if (openKeys.has(key)) return Promise.resolve(false);
  openKeys.add(key);

  return new Promise((resolve) => {
    let resolved = false;
    let releaseTrap = null;

    function finish(result) {
      if (resolved) return;
      resolved = true;
      openKeys.delete(key);
      if (releaseTrap) releaseTrap();
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve(result);
    }

    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        finish(false);
      }
    }

    const overlay = el('div', { class: 'confirm-dialog-overlay' });
    const dialog = el('div', {
      class: 'confirm-dialog mcp-consent-dialog',
      role: 'alertdialog',
      'aria-modal': 'true',
      'aria-labelledby': 'mcp-consent-title',
      'aria-describedby': 'mcp-consent-message',
      style: 'max-width: 720px; width: 90vw;',
    });

    dialog.appendChild(
      el('div', { class: 'confirm-dialog__title', id: 'mcp-consent-title' },
        'Approve MCP server config?'),
    );

    const intro = el('div', { class: 'confirm-dialog__message', id: 'mcp-consent-message' });
    intro.appendChild(document.createTextNode(
      'This project ships an .mcp.json file that will spawn external processes ' +
      'every time you message the agent. Review the contents and approve only if ' +
      'you trust them.'
    ));
    intro.appendChild(el('br'));
    const pathLine = el('code', { style: 'font-size: 11px; opacity: 0.7;' });
    pathLine.textContent = projectPath;
    intro.appendChild(pathLine);
    dialog.appendChild(intro);

    // Show the file content verbatim. textContent (no innerHTML) — defence in
    // depth in case a future regression lets HTML through.
    const pre = el('pre', {
      style: 'max-height: 320px; overflow: auto; padding: 12px; ' +
             'background: var(--color-surface-2, #1a1a1a); ' +
             'border-radius: 6px; font-size: 12px; white-space: pre-wrap; ' +
             'word-break: break-all; margin: 12px 0;',
    });
    pre.textContent = content;
    dialog.appendChild(pre);

    const hashLine = el('div', {
      style: 'font-size: 11px; opacity: 0.6; margin-bottom: 12px; font-family: monospace;',
    });
    hashLine.textContent = `sha256: ${contentHash}`;
    dialog.appendChild(hashLine);

    const actions = el('div', { class: 'confirm-dialog__actions' });
    const denyBtn = el('button', {
      class: 'confirm-dialog__btn confirm-dialog__btn--cancel',
    }, 'Deny');
    const approveBtn = el('button', {
      class: 'confirm-dialog__btn confirm-dialog__btn--save',
    }, 'Approve & connect');

    denyBtn.addEventListener('click', () => finish(false));
    approveBtn.addEventListener('click', async () => {
      approveBtn.disabled = true;
      approveBtn.textContent = 'Approving...';
      try {
        await api.approveMcpProjectConsent(projectId, contentHash);
        finish(true);
      } catch (e) {
        approveBtn.disabled = false;
        approveBtn.textContent = 'Approve & connect';
        const err = el('div', {
          style: 'color: var(--color-error, #f55); font-size: 12px; margin-top: 8px;',
        });
        err.textContent = `Approval failed: ${e}`;
        dialog.appendChild(err);
      }
    });

    actions.appendChild(denyBtn);
    actions.appendChild(approveBtn);
    dialog.appendChild(actions);

    overlay.appendChild(dialog);
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) finish(false);
    });

    document.body.appendChild(overlay);
    document.addEventListener('keydown', onKey);
    releaseTrap = trapFocus(dialog);
    denyBtn.focus();
  });
}

// Wire the global listener. Call once at app start.
//
// `getProjectIdForPath` lets the caller (main.js) translate the backend's
// projectPath -> projectId so we don't have to thread the workspace store
// through this module.
export async function initMcpConsentListener(getProjectIdForPath) {
  try {
    await api.onMcpConsentRequired((payload) => {
      if (!payload) return;
      const projectId = getProjectIdForPath ? getProjectIdForPath(payload.projectPath) : null;
      if (!projectId) {
        // No matching project in the workspace — backend should never emit
        // for a project not registered here, but bail rather than show an
        // unactionable modal.
        return;
      }
      showMcpConsentDialog({
        projectPath: payload.projectPath,
        contentHash: payload.contentHash,
        content: payload.content,
        projectId,
      });
    });
  } catch (e) {
    console.warn('[mcp] consent listener init failed', e);
  }
}
