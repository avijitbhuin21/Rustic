import { el, icon } from '../utils/dom.js';
import * as api from '../lib/tauri-api.js';
import { checkoutBranch, createBranch, rebase } from '../state/git.js';
import { showToast, showErrorToast } from './toast.js';

export function createBranchSwitcher(projectId, anchorEl) {
  // Remove existing switcher
  const existing = document.querySelector('.branch-switcher');
  if (existing) { existing.remove(); return; }

  const overlay = el('div', { class: 'branch-switcher' });
  const modal = el('div', { class: 'branch-switcher__modal' });

  // Position near anchor
  const rect = anchorEl.getBoundingClientRect();
  modal.style.left = rect.left + 'px';
  modal.style.bottom = (window.innerHeight - rect.top + 4) + 'px';

  // Search input
  const input = el('input', {
    class: 'branch-switcher__input',
    type: 'text',
    placeholder: 'Switch to branch...',
    spellcheck: 'false',
  });

  const list = el('div', { class: 'branch-switcher__list' });
  const actions = el('div', { class: 'branch-switcher__actions' });

  // Create new branch button
  const createBtn = el('button', { class: 'branch-switcher__create' });
  createBtn.appendChild(icon('M12 5v14M5 12h14', 12));
  createBtn.appendChild(el('span', {}, 'Create New Branch'));
  createBtn.addEventListener('click', () => {
    const name = input.value.trim();
    if (!name) {
      showToast('Type a branch name in the search box first', { kind: 'error' });
      input.focus();
      return;
    }
    createBranch(projectId, name, true)
      .then(close)
      .catch((e) => showErrorToast('Create branch failed', e));
  });
  actions.appendChild(createBtn);

  // Rebase button
  const rebaseBtn = el('button', { class: 'branch-switcher__create' });
  rebaseBtn.appendChild(icon('M6 3v12M18 3v12M6 15l12-12', 12));
  rebaseBtn.appendChild(el('span', {}, 'Rebase onto...'));
  rebaseBtn.addEventListener('click', () => {
    const name = input.value.trim();
    if (!name) {
      showToast('Type the branch to rebase onto in the search box first', { kind: 'error' });
      input.focus();
      return;
    }
    rebase(projectId, name)
      .then(close)
      .catch((e) => showErrorToast('Rebase failed', e));
  });
  actions.appendChild(rebaseBtn);

  modal.appendChild(input);
  modal.appendChild(list);
  modal.appendChild(actions);
  overlay.appendChild(modal);
  document.body.appendChild(overlay);

  // Load branches
  let allBranches = [];
  api.gitBranches(projectId).then(branches => {
    if (!branches) return;
    allBranches = branches;
    renderBranches(branches);
  });

  function renderBranches(branches) {
    list.innerHTML = '';
    for (const b of branches) {
      const item = el('div', { class: 'branch-switcher__item' + (b.is_head ? ' branch-switcher__item--active' : '') });

      if (b.is_head) {
        item.appendChild(icon('M5 12l5 5L20 7', 12));
      } else {
        item.appendChild(el('span', { style: { width: '12px', display: 'inline-block' } }));
      }

      const nameSpan = el('span', { class: 'branch-switcher__name' }, b.name);
      item.appendChild(nameSpan);

      if (b.is_remote) {
        item.appendChild(el('span', { class: 'branch-switcher__tag' }, 'remote'));
      }

      if (!b.is_head && !b.is_remote) {
        item.addEventListener('click', () => {
          checkoutBranch(projectId, b.name)
            .then(close)
            .catch((e) => showErrorToast('Checkout failed', e));
        });
      } else if (b.is_remote) {
        item.title = 'Remote branch — checkout requires a local tracking branch (not yet supported)';
        item.style.opacity = '0.6';
      } else if (b.is_head) {
        item.title = 'Already on this branch';
      }

      list.appendChild(item);
    }
  }

  // Filter on input
  input.addEventListener('input', () => {
    const query = input.value.toLowerCase();
    const filtered = allBranches.filter(b => b.name.toLowerCase().includes(query));
    renderBranches(filtered);
  });

  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') close();
    if (e.key === 'Enter') {
      const query = input.value.trim();
      const match = allBranches.find(b => b.name === query && !b.is_remote);
      if (match && !match.is_head) {
        checkoutBranch(projectId, match.name).then(close).catch(() => {});
      }
    }
  });

  function close() {
    overlay.remove();
  }

  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) close();
  });

  requestAnimationFrame(() => input.focus());
}
