import { el, icon } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { renderMarkdown } from '../../utils/markdown.js';
import { workspaceStore } from '../../state/workspace.js';
import * as api from '../../lib/tauri-api.js';
import { showAlertDialog } from '../confirm-dialog.js';

const RULE_INFO_HTML = `
  <p><strong>What are rules?</strong> — Rules are user-defined instructions
     that get injected directly into the agent's system prompt. Use them to
     enforce conventions, coding preferences, or any behavior you want the
     agent to follow for every task.</p>
  <p><strong>Where are they used?</strong> — Active rules are appended to
     the system prompt of every task started after activation.</p>
  <p><strong>Activation states (3-state toggle):</strong></p>
  <ul>
    <li><strong>Inactive</strong> — defined but never injected.</li>
    <li><strong>Global</strong> — injected for every project on this machine.</li>
    <li><strong>Project</strong> — injected only when the currently open project is active.</li>
  </ul>
  <p>Rules are stored globally at
     <code>~/.rustic/rules/&lt;name&gt;.md</code>. Newly created rules default to
     <em>inactive</em> — toggle them using the slider on each row.</p>
`;

function currentProjectRoot() {
  const projects = workspaceStore.getState('projects') || [];
  if (!projects.length) return null;
  return projects[0].root_path || null;
}

export function createRulesHeaderActions(onPlus, onInfo) {
  const wrap = el('div');
  const infoBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'About rules' });
  infoBtn.appendChild(icon('M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 4a1.5 1.5 0 1 1-1.5 1.5A1.5 1.5 0 0 1 12 6zm2 12h-4v-1h1v-5h-1v-1h3v6h1z', 14));
  infoBtn.addEventListener('click', onInfo);
  wrap.appendChild(infoBtn);

  const plusBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add rule' });
  plusBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  plusBtn.addEventListener('click', onPlus);
  wrap.appendChild(plusBtn);

  return wrap;
}

export function createRulesPanel() {
  const container = el('div', { class: 'rules-panel' });

  // Single flat list — same shape as the skills panel — instead of three
  // separate group sections. Group context (Global / Project / Inactive) now
  // shows up as a small badge on each row, so the user can see state at a
  // glance without the visual breakage that came with separate empty-state
  // panels.
  const ruleList = el('div', { class: 'rules-list' });
  container.appendChild(ruleList);

  let rules = [];

  async function loadRules() {
    const projectRoot = currentProjectRoot();
    try {
      rules = (await api.listRules(projectRoot)) || [];
    } catch {
      rules = [];
    }
    render();
  }

  function render() {
    ruleList.innerHTML = '';
    if (rules.length === 0) {
      ruleList.appendChild(el('div', { class: 'rules-list__empty' }, 'No rules defined'));
      return;
    }

    // Sort: global → project → inactive (most active first), then by name.
    const order = { global: 0, project: 1, inactive: 2 };
    const sorted = rules.slice().sort((a, b) => {
      const oa = order[a.state] ?? 99;
      const ob = order[b.state] ?? 99;
      if (oa !== ob) return oa - ob;
      return a.name.localeCompare(b.name);
    });

    for (let i = 0; i < sorted.length; i++) {
      const rule = sorted[i];
      const item = el('div', { class: 'rules-item' });

      const info = el('div', { class: 'rules-item__info' });

      const nameRow = el('div', { class: 'rules-item__name-row' });
      nameRow.appendChild(el('span', { class: 'rules-item__name' }, rule.name));
      nameRow.appendChild(buildStateBadge(rule.state));
      info.appendChild(nameRow);
      info.appendChild(el('div', { class: 'rules-item__description' }, rule.description || ''));
      item.appendChild(info);

      const actions = el('div', { class: 'rules-item__actions' });
      // 3-state slider stays visible (not hover-only) — it's the row's
      // primary control, hiding it would force users to discover hover
      // affordance just to flip a rule on/off.
      actions.appendChild(buildSlider(rule));

      const viewBtn = el('button', { title: 'View rule' });
      viewBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6', 14));
      viewBtn.addEventListener('click', () => openViewModal(rule));
      actions.appendChild(viewBtn);

      const editBtn = el('button', { title: 'Edit rule' });
      editBtn.appendChild(icon('M12 20h9 M16.5 3.5a2.121 2.121 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z', 14));
      editBtn.addEventListener('click', () => openCreateModal(rule));
      actions.appendChild(editBtn);

      const deleteBtn = el('button', { title: 'Delete rule' });
      deleteBtn.appendChild(icon('M3 6h18 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6', 14));
      deleteBtn.addEventListener('click', () => openDeleteModal(rule));
      actions.appendChild(deleteBtn);

      item.appendChild(actions);
      ruleList.appendChild(item);

      if (i < sorted.length - 1) {
        ruleList.appendChild(el('div', { class: 'rules-item__divider' }));
      }
    }
  }

  function buildStateBadge(state) {
    const map = {
      global: { text: 'Global', cls: 'rules-item__badge--global' },
      project: { text: 'Project', cls: 'rules-item__badge--project' },
      inactive: { text: 'Off', cls: 'rules-item__badge--inactive' },
    };
    const meta = map[state] || map.inactive;
    return el('span', { class: `rules-item__badge ${meta.cls}` }, meta.text);
  }

  function buildSlider(rule) {
    const slider = el('div', { class: 'rules-slider', title: 'Cycle: Inactive → Global → Project' });
    slider.setAttribute('data-state', rule.state);

    const optInactive = el('div', { class: 'rules-slider__opt', 'data-value': 'inactive', title: 'Inactive' }, 'Off');
    const optGlobal = el('div', { class: 'rules-slider__opt', 'data-value': 'global', title: 'Active globally' }, 'G');
    const optProject = el('div', { class: 'rules-slider__opt', 'data-value': 'project', title: 'Active for this project' }, 'P');
    slider.appendChild(optInactive);
    slider.appendChild(optGlobal);
    slider.appendChild(optProject);

    const setState = async (target) => {
      const projectRoot = currentProjectRoot();
      if (target === 'project' && !projectRoot) {
        await showAlertDialog('No project open', 'Cannot set rule as project-active without an open project.');
        return;
      }
      try {
        await api.setRuleActivation(rule.name, target, projectRoot);
        await loadRules();
      } catch (e) {
        await showAlertDialog('Failed to update rule', String(e));
      }
    };

    // Clicking an option sets directly to that state
    slider.addEventListener('click', (ev) => {
      const opt = ev.target.closest('.rules-slider__opt');
      if (!opt) {
        // Click on gap → cycle next state
        const order = ['inactive', 'global', 'project'];
        const next = order[(order.indexOf(rule.state) + 1) % order.length];
        if (rule.state !== 'inactive' || next === 'global' || currentProjectRoot()) {
          setState(next);
        }
        return;
      }
      const target = opt.getAttribute('data-value');
      if (target !== rule.state) setState(target);
    });

    return slider;
  }

  async function openViewModal(rule) {
    const content = el('div', { class: 'skills-view__content' }, 'Loading…');
    openModal({ title: rule.name, body: content, size: 'lg' });
    try {
      const text = await api.getRuleBody(rule.name);
      content.innerHTML = '';
      content.appendChild(renderMarkdown(text || ''));
    } catch (e) {
      content.textContent = `Could not load rule: ${e}`;
    }
  }

  function openInfoModal() {
    const body = el('div', { class: 'rustic-modal__info' });
    body.innerHTML = RULE_INFO_HTML;
    openModal({ title: 'About rules', body, buttons: [{ label: 'Close' }] });
  }

  function openCreateModal(existing = null) {
    const body = el('div', { class: 'skills-edit-form' });
    const nameInput = el('input', {
      class: 'rustic-modal__input',
      type: 'text',
      placeholder: 'e.g. always-use-pytest',
    });
    const bodyArea = el('textarea', {
      class: 'rustic-modal__textarea',
      placeholder: 'Full rule text (injected into the agent\'s system prompt when active)…',
      rows: 12,
    });

    if (existing) {
      nameInput.value = existing.name;
    }

    body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Title'));
    body.appendChild(nameInput);
    body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Rule'));
    body.appendChild(bodyArea);

    const err = el('div', { class: 'skills-install-form__status' });
    body.appendChild(err);

    openModal({
      title: existing ? 'Edit rule' : 'Create rule',
      body,
      size: 'lg',
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: existing ? 'Save' : 'Create',
          variant: 'primary',
          onClick: async () => {
            const name = nameInput.value.trim();
            const content = bodyArea.value;
            if (!name || !content.trim()) {
              err.textContent = 'Title and rule body are required';
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            try {
              if (existing) {
                await api.updateRule(existing.name, name, content);
              } else {
                await api.createRule(name, content);
              }
            } catch (e) {
              err.textContent = String(e).replace(/^Error: /, '');
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            loadRules();
            return true;
          },
        },
      ],
    });

    if (existing) {
      api.getRuleBody(existing.name).then((b) => { bodyArea.value = b || ''; }).catch(() => {});
    }
    nameInput.focus();
  }

  function openDeleteModal(rule) {
    const body = el('div', { class: 'rustic-modal__confirm' });
    body.appendChild(el('p', {}, `Delete rule "${rule.name}"? This cannot be undone.`));
    openModal({
      title: 'Delete rule',
      body,
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: 'Delete',
          variant: 'danger',
          onClick: async () => {
            try {
              await api.deleteRule(rule.name);
              loadRules();
            } catch (e) {
              await showAlertDialog('Delete failed', String(e));
              return false;
            }
          },
        },
      ],
    });
  }

  container._onPlus = () => openCreateModal();
  container._onInfo = openInfoModal;

  loadRules();
  return container;
}
