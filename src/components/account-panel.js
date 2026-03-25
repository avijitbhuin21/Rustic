import { el, icon } from '../utils/dom.js';
import { gitStore, setGitToken, checkGitToken } from '../state/git.js';
import * as api from '../lib/tauri-api.js';

export function createAccountPanel(anchorEl) {
  // Remove existing
  const existing = document.querySelector('.account-panel');
  if (existing) { existing.remove(); return; }

  const overlay = el('div', { class: 'account-panel' });
  const modal = el('div', { class: 'account-panel__modal' });

  const rect = anchorEl.getBoundingClientRect();
  modal.style.left = (rect.right + 8) + 'px';
  modal.style.bottom = (window.innerHeight - rect.bottom) + 'px';

  const header = el('div', { class: 'account-panel__header' }, 'Git Authentication');
  modal.appendChild(header);

  const hasToken = gitStore.getState('hasToken');

  if (hasToken) {
    renderLoggedIn(modal, overlay);
  } else {
    renderLoginOptions(modal, overlay);
  }

  overlay.appendChild(modal);
  document.body.appendChild(overlay);

  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) overlay.remove();
  });
}

async function renderLoggedIn(modal, overlay) {
  const statusRow = el('div', { class: 'account-panel__status' });
  statusRow.appendChild(icon('M5 12l5 5L20 7', 14));
  statusRow.appendChild(el('span', {}, 'Authenticated'));
  modal.appendChild(statusRow);

  // Try to load user info
  const userRow = el('div', { class: 'account-panel__user' });
  modal.appendChild(userRow);

  try {
    const user = await api.githubGetUser();
    if (user) {
      userRow.innerHTML = '';
      if (user.avatar_url) {
        const avatar = el('img', { class: 'account-panel__avatar', src: user.avatar_url });
        userRow.appendChild(avatar);
      }
      userRow.appendChild(el('span', { class: 'account-panel__username' }, user.login));
    }
  } catch {
    // Token might be a PAT — just show generic status
  }

  const logoutBtn = el('button', { class: 'account-panel__btn account-panel__btn--danger' }, 'Sign Out');
  logoutBtn.addEventListener('click', () => {
    setGitToken('');
    overlay.remove();
  });
  modal.appendChild(logoutBtn);
}

function renderLoginOptions(modal, overlay) {
  // OAuth button (primary)
  const oauthBtn = el('button', { class: 'account-panel__btn account-panel__btn--github' });
  oauthBtn.appendChild(el('span', {}, 'Sign in with GitHub'));
  oauthBtn.addEventListener('click', () => {
    modal.innerHTML = '';
    modal.appendChild(el('div', { class: 'account-panel__header' }, 'Git Authentication'));
    startOAuthFlow(modal, overlay);
  });
  modal.appendChild(oauthBtn);

  // Divider
  const divider = el('div', { class: 'account-panel__divider' });
  divider.appendChild(el('span', {}, 'or'));
  modal.appendChild(divider);

  // PAT fallback
  const desc = el('div', { class: 'account-panel__desc' }, 'Use a Personal Access Token');
  modal.appendChild(desc);

  const tokenInput = el('input', {
    class: 'account-panel__input',
    type: 'password',
    placeholder: 'ghp_xxxxxxxxxxxx',
    spellcheck: 'false',
  });
  modal.appendChild(tokenInput);

  const loginBtn = el('button', { class: 'account-panel__btn account-panel__btn--primary' }, 'Sign In');
  loginBtn.addEventListener('click', () => {
    const token = tokenInput.value.trim();
    if (token) {
      setGitToken(token);
      overlay.remove();
    }
  });
  modal.appendChild(loginBtn);

  tokenInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') loginBtn.click();
    if (e.key === 'Escape') overlay.remove();
  });
}

async function startOAuthFlow(modal, overlay) {
  const statusEl = el('div', { class: 'account-panel__desc' }, 'Requesting device code...');
  modal.appendChild(statusEl);

  try {
    const deviceData = await api.githubDeviceCode();
    if (!deviceData) {
      statusEl.textContent = 'Failed to start OAuth flow.';
      return;
    }

    statusEl.textContent = 'Enter this code on GitHub:';

    // Show the user code prominently
    const codeEl = el('div', { class: 'account-panel__device-code' }, deviceData.user_code);
    modal.appendChild(codeEl);

    // Copy button
    const copyBtn = el('button', { class: 'account-panel__btn account-panel__btn--secondary' }, 'Copy Code');
    copyBtn.addEventListener('click', () => {
      navigator.clipboard.writeText(deviceData.user_code);
      copyBtn.textContent = 'Copied!';
      setTimeout(() => { copyBtn.textContent = 'Copy Code'; }, 1500);
    });
    modal.appendChild(copyBtn);

    // Open GitHub link
    const linkBtn = el('button', { class: 'account-panel__btn account-panel__btn--github' }, 'Open GitHub');
    linkBtn.addEventListener('click', () => {
      window.open(deviceData.verification_uri, '_blank');
    });
    modal.appendChild(linkBtn);

    const waitingEl = el('div', { class: 'account-panel__waiting' }, 'Waiting for authorization...');
    modal.appendChild(waitingEl);

    // Poll for token
    const interval = (deviceData.interval || 5) * 1000;
    let attempts = 0;
    const maxAttempts = Math.ceil(deviceData.expires_in / (deviceData.interval || 5));

    const pollTimer = setInterval(async () => {
      attempts++;
      if (attempts > maxAttempts) {
        clearInterval(pollTimer);
        waitingEl.textContent = 'Code expired. Please try again.';
        return;
      }

      try {
        const token = await api.githubPollToken(deviceData.device_code);
        if (token) {
          clearInterval(pollTimer);
          checkGitToken();
          overlay.remove();
        }
      } catch (e) {
        const err = String(e);
        if (err.includes('authorization_pending')) {
          // Still waiting — this is normal
        } else if (err.includes('slow_down')) {
          // Back off — handled by interval
        } else if (err.includes('expired_token')) {
          clearInterval(pollTimer);
          waitingEl.textContent = 'Code expired. Please try again.';
        } else if (err.includes('access_denied')) {
          clearInterval(pollTimer);
          waitingEl.textContent = 'Authorization denied.';
        }
      }
    }, interval);

    // Cleanup on close
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) clearInterval(pollTimer);
    });

  } catch (e) {
    statusEl.textContent = 'Failed to start OAuth: ' + e;
  }
}
