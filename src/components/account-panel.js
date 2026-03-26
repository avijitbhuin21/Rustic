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

  // Close on click outside, but not during OAuth polling
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay && !overlay.dataset.polling) overlay.remove();
  });
}

async function renderLoggedIn(modal, overlay) {
  // Header row: "Git Authentication ✓"
  const headerEl = modal.querySelector('.account-panel__header');
  if (headerEl) {
    headerEl.innerHTML = '';
    headerEl.appendChild(el('span', {}, 'Git Authentication'));
    const check = icon('M5 12l5 5L20 7', 12);
    check.style.color = 'var(--bright-green)';
    check.style.marginLeft = '6px';
    headerEl.appendChild(check);
    headerEl.style.display = 'flex';
    headerEl.style.alignItems = 'center';
  }

  // Single row: username + sign out
  const row = el('div', { class: 'account-panel__auth-row' });

  const nameEl = el('span', { class: 'account-panel__username' }, 'Signed in');
  row.appendChild(nameEl);

  const logoutBtn = el('button', { class: 'account-panel__btn account-panel__btn--danger' }, 'Sign Out');
  logoutBtn.addEventListener('click', () => {
    setGitToken('');
    overlay.remove();
  });
  row.appendChild(logoutBtn);

  modal.appendChild(row);

  // Fetch username
  try {
    const user = await api.githubGetUser();
    if (user) nameEl.textContent = user.login;
  } catch {
    // PAT or error — keep generic text
  }
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
      api.openUrl(deviceData.verification_uri);
    });
    modal.appendChild(linkBtn);

    const waitingEl = el('div', { class: 'account-panel__waiting' }, 'Waiting for authorization...');
    modal.appendChild(waitingEl);

    // Prevent overlay from closing during polling
    overlay.dataset.polling = 'true';

    // Poll for token using async loop (avoids setInterval + async issues)
    const pollInterval = (deviceData.interval || 5) * 1000;
    const maxTime = deviceData.expires_in * 1000;
    const startTime = Date.now();
    let cancelled = false;

    async function pollLoop() {
      while (!cancelled) {
        // Wait the required interval
        await new Promise(r => setTimeout(r, pollInterval));

        if (cancelled) break;
        if (Date.now() - startTime > maxTime) {
          delete overlay.dataset.polling;
          waitingEl.textContent = 'Code expired. Please try again.';
          return;
        }

        try {
          const token = await api.githubPollToken(deviceData.device_code);
          if (token) {
            // Success!
            await checkGitToken();
            overlay.remove();
            return;
          }
        } catch (e) {
          const err = String(e);
          if (err.includes('authorization_pending')) {
            // Normal — keep polling
            continue;
          } else if (err.includes('slow_down')) {
            // Back off — wait an extra interval
            await new Promise(r => setTimeout(r, pollInterval));
            continue;
          } else if (err.includes('expired_token')) {
            delete overlay.dataset.polling;
            waitingEl.textContent = 'Code expired. Please try again.';
            return;
          } else if (err.includes('access_denied')) {
            delete overlay.dataset.polling;
            waitingEl.textContent = 'Authorization denied.';
            return;
          } else {
            console.warn('OAuth poll unexpected:', err);
            // Keep trying — might be a transient error
            continue;
          }
        }
      }
    }

    pollLoop();

  } catch (e) {
    statusEl.textContent = 'Failed to start OAuth: ' + e;
  }
}
