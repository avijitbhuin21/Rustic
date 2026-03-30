import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

/**
 * Skills panel for listing, installing, creating, and deleting skills.
 * @param {string} projectId — the current project ID
 */
export function createSkillsPanel(projectId) {
  const container = el('div', { class: 'skills-panel' });

  // Header
  const header = el('div', { class: 'skills-panel__header' });
  header.appendChild(el('span', { class: 'skills-panel__title' }, 'Skills'));

  const headerActions = el('div', { class: 'skills-panel__header-actions' });

  const createBtn = el('button', { class: 'skills-panel__btn', title: 'Create Skill' });
  createBtn.appendChild(icon('M12 5v14M5 12h14', 12));
  createBtn.addEventListener('click', () => showCreateForm());
  headerActions.appendChild(createBtn);
  header.appendChild(headerActions);

  container.appendChild(header);

  // Install bar
  const installBar = el('div', { class: 'skills-install-bar' });
  const installInput = el('input', {
    class: 'skills-install-bar__input',
    type: 'text',
    placeholder: 'owner/repo or GitHub URL',
  });
  const installBtn = el('button', { class: 'skills-install-bar__btn' }, 'Install');
  const installStatus = el('span', { class: 'skills-install-bar__status' });

  installBtn.addEventListener('click', async () => {
    const source = installInput.value.trim();
    if (!source) return;
    installBtn.disabled = true;
    installStatus.textContent = 'Installing…';
    installStatus.className = 'skills-install-bar__status';
    try {
      const skills = await api.installSkill(projectId, source);
      installInput.value = '';
      installStatus.textContent = `Installed ${skills.length} skill(s)`;
      installStatus.classList.add('skills-install-bar__status--ok');
      loadSkills();
    } catch (e) {
      installStatus.textContent = String(e).replace(/^Error: /, '');
      installStatus.classList.add('skills-install-bar__status--err');
    }
    installBtn.disabled = false;
  });

  installBar.appendChild(installInput);
  installBar.appendChild(installBtn);
  installBar.appendChild(installStatus);
  container.appendChild(installBar);

  // Form container (create form renders here)
  const formContainer = el('div', { class: 'skills-form-container' });
  container.appendChild(formContainer);

  // Skills list
  const skillList = el('div', { class: 'skills-list' });
  container.appendChild(skillList);

  let skills = [];

  async function loadSkills() {
    try {
      skills = (await api.listSkills(projectId)) || [];
    } catch (e) {
      skills = [];
    }
    renderList();
  }

  function renderList() {
    skillList.innerHTML = '';
    if (skills.length === 0) {
      skillList.appendChild(
        el('div', { class: 'skills-list__empty' }, 'No skills installed')
      );
      return;
    }
    for (const skill of skills) {
      const item = el('div', { class: 'skills-item' });

      const info = el('div', { class: 'skills-item__info' });
      const nameRow = el('div', { class: 'skills-item__name-row' });
      nameRow.appendChild(el('span', { class: 'skills-item__name' }, skill.name));
      nameRow.appendChild(
        el(
          'span',
          {
            class: `skills-item__badge skills-item__badge--${skill.scope}`,
          },
          skill.scope
        )
      );
      info.appendChild(nameRow);
      info.appendChild(
        el('span', { class: 'skills-item__description' }, skill.description)
      );

      const actions = el('div', { class: 'skills-item__actions' });

      // View body button
      const viewBtn = el('button', { title: 'View skill body' });
      viewBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6', 12));
      viewBtn.addEventListener('click', async () => {
        try {
          const body = await api.getSkillBody(projectId, skill.name);
          showBodyModal(skill.name, body);
        } catch (e) {
          alert(`Could not load skill: ${e}`);
        }
      });

      // Delete button
      const deleteBtn = el('button', { title: 'Delete skill' });
      deleteBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
      deleteBtn.addEventListener('click', async () => {
        if (!confirm(`Delete skill "${skill.name}"?`)) return;
        try {
          await api.deleteSkill(projectId, skill.name);
          loadSkills();
        } catch (e) {
          alert(`Delete failed: ${e}`);
        }
      });

      actions.appendChild(viewBtn);
      actions.appendChild(deleteBtn);
      item.appendChild(info);
      item.appendChild(actions);
      skillList.appendChild(item);
    }
  }

  function showCreateForm() {
    formContainer.innerHTML = '';
    const form = el('div', { class: 'skills-create-form' });

    const nameInput = el('input', {
      class: 'skills-create-form__input',
      type: 'text',
      placeholder: 'Skill name (e.g. code-review)',
    });
    const descInput = el('input', {
      class: 'skills-create-form__input',
      type: 'text',
      placeholder: 'Short description',
    });
    const bodyArea = el('textarea', {
      class: 'skills-create-form__textarea',
      placeholder: 'Skill instructions (shown to the model when activated)…',
      rows: 6,
    });

    const btnRow = el('div', { class: 'skills-create-form__buttons' });
    const saveBtn = el('button', { class: 'skills-create-form__save' }, 'Create');
    const cancelBtn = el('button', { class: 'skills-create-form__cancel' }, 'Cancel');

    saveBtn.addEventListener('click', async () => {
      const name = nameInput.value.trim();
      const description = descInput.value.trim();
      const body = bodyArea.value.trim();
      if (!name || !description) return;
      try {
        await api.createSkill(projectId, name, description, body);
        formContainer.innerHTML = '';
        loadSkills();
      } catch (e) {
        alert(`Create failed: ${e}`);
      }
    });

    cancelBtn.addEventListener('click', () => {
      formContainer.innerHTML = '';
    });

    btnRow.appendChild(saveBtn);
    btnRow.appendChild(cancelBtn);

    form.appendChild(el('div', { class: 'skills-create-form__label' }, 'New Skill'));
    form.appendChild(nameInput);
    form.appendChild(descInput);
    form.appendChild(bodyArea);
    form.appendChild(btnRow);
    formContainer.appendChild(form);
    nameInput.focus();
  }

  function showBodyModal(name, body) {
    // Reuse formContainer as a simple modal
    formContainer.innerHTML = '';
    const modal = el('div', { class: 'skills-body-modal' });
    modal.appendChild(el('div', { class: 'skills-body-modal__title' }, name));
    const pre = el('pre', { class: 'skills-body-modal__body' });
    pre.textContent = body;
    const closeBtn = el('button', { class: 'skills-body-modal__close' }, 'Close');
    closeBtn.addEventListener('click', () => { formContainer.innerHTML = ''; });
    modal.appendChild(pre);
    modal.appendChild(closeBtn);
    formContainer.appendChild(modal);
  }

  loadSkills();
  return container;
}
