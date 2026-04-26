import { el, icon } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { renderMarkdown } from '../../utils/markdown.js';
import * as api from '../../lib/tauri-api.js';
import { showAlertDialog } from '../confirm-dialog.js';

const SKILL_INFO_HTML = `
  <p><strong>Creating a skill</strong> — Fill in the title and the full
     description (instructions loaded when the skill is activated). The short
     preview shown in the list is auto-generated from the first 150 characters
     of the description.</p>
  <p><strong>Installing from GitHub</strong> — Paste any of:</p>
  <ul>
    <li><code>owner/repo</code></li>
    <li><code>https://github.com/owner/repo</code></li>
    <li>A <code>blob</code> URL to a single <code>.md</code> file, e.g.
        <code>https://github.com/anthropics/skills/blob/main/skills/frontend-design/SKILL.md</code></li>
    <li>A <code>raw.githubusercontent.com</code> URL, e.g.
        <code>https://raw.githubusercontent.com/owner/repo/main/path/to/file.md</code></li>
  </ul>
  <p>After submitting a URL you'll see every <code>SKILL.md</code> found in the
     repo. Pick the ones you want and click <em>Install selected</em>.</p>
  <p>All skills are installed globally at
     <code>~/.rustic/skills/&lt;name&gt;/</code> and are available in every
     project.</p>
`;

/**
 * Header-actions element for the Skills collapsible (plus + info icons).
 * Call this and pass the returned element as the 4th arg of createCollapsible.
 */
export function createSkillsHeaderActions(onPlus, onInfo) {
  const wrap = el('div');
  const infoBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'About skills' });
  infoBtn.appendChild(icon('M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 4a1.5 1.5 0 1 1-1.5 1.5A1.5 1.5 0 0 1 12 6zm2 12h-4v-1h1v-5h-1v-1h3v6h1z', 14));
  infoBtn.addEventListener('click', onInfo);
  wrap.appendChild(infoBtn);

  const plusBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add skill' });
  plusBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  plusBtn.addEventListener('click', onPlus);
  wrap.appendChild(plusBtn);

  return wrap;
}

export function createSkillsPanel() {
  const container = el('div', { class: 'skills-panel' });

  // List
  const skillList = el('div', { class: 'skills-list' });
  container.appendChild(skillList);

  let skills = [];

  async function loadSkills() {
    try {
      skills = (await api.listSkills()) || [];
    } catch {
      skills = [];
    }
    renderList();
  }

  function renderList() {
    skillList.innerHTML = '';
    if (skills.length === 0) {
      skillList.appendChild(el('div', { class: 'skills-list__empty' }, 'No skills installed'));
      return;
    }
    for (let i = 0; i < skills.length; i++) {
      const skill = skills[i];
      const item = el('div', { class: 'skills-item' });

      const nameRow = el('div', { class: 'skills-item__name-row' });
      nameRow.appendChild(el('span', { class: 'skills-item__name' }, skill.name));

      const actions = el('div', { class: 'skills-item__actions' });

      const viewBtn = el('button', { title: 'View skill' });
      viewBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6', 14));
      viewBtn.addEventListener('click', () => openViewModal(skill));
      actions.appendChild(viewBtn);

      const editBtn = el('button', { title: 'Edit skill' });
      editBtn.appendChild(icon('M12 20h9 M16.5 3.5a2.121 2.121 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z', 14));
      editBtn.addEventListener('click', () => openCreateModal(skill));
      actions.appendChild(editBtn);

      const deleteBtn = el('button', { title: 'Delete skill' });
      deleteBtn.appendChild(icon('M3 6h18 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6', 14));
      deleteBtn.addEventListener('click', () => openDeleteModal(skill));
      actions.appendChild(deleteBtn);

      nameRow.appendChild(actions);
      item.appendChild(nameRow);
      item.appendChild(el('div', { class: 'skills-item__description' }, skill.description || ''));

      skillList.appendChild(item);
      if (i < skills.length - 1) {
        skillList.appendChild(el('div', { class: 'skills-item__divider' }));
      }
    }
  }

  async function openViewModal(skill) {
    const content = el('div', { class: 'skills-view__content' }, 'Loading…');
    openModal({ title: skill.name, body: content, size: 'lg' });

    try {
      const text = await api.getSkillBody(skill.name);
      content.innerHTML = '';
      content.appendChild(renderMarkdown(text || ''));
    } catch (e) {
      content.textContent = `Could not load skill: ${e}`;
    }
  }

  function openInfoModal() {
    const body = el('div', { class: 'rustic-modal__info' });
    body.innerHTML = SKILL_INFO_HTML;
    openModal({ title: 'About skills', body, buttons: [{ label: 'Close' }] });
  }

  // Step 1: chooser
  function openAddModal() {
    const body = el('div', { class: 'skills-chooser' });
    const installBtn = el('button', { class: 'skills-chooser__choice' });
    installBtn.appendChild(el('div', { class: 'skills-chooser__choice-title' }, 'Install from GitHub'));
    installBtn.appendChild(el('div', { class: 'skills-chooser__choice-desc' }, 'Import one or more SKILL.md files from a repo or direct URL.'));

    const createBtn = el('button', { class: 'skills-chooser__choice' });
    createBtn.appendChild(el('div', { class: 'skills-chooser__choice-title' }, 'Create custom skill'));
    createBtn.appendChild(el('div', { class: 'skills-chooser__choice-desc' }, 'Write a new skill with your own title and instructions.'));

    body.appendChild(installBtn);
    body.appendChild(createBtn);

    const close = openModal({ title: 'Add skill', body });

    installBtn.addEventListener('click', () => { close(); openInstallModal(); });
    createBtn.addEventListener('click', () => { close(); openCreateModal(); });
  }

  function openInstallModal() {
    const body = el('div', { class: 'skills-install-form' });

    body.appendChild(el('label', { class: 'rustic-modal__label' }, 'GitHub URL or owner/repo'));
    const urlRow = el('div', { class: 'skills-install-form__row' });
    const urlInput = el('input', {
      class: 'rustic-modal__input',
      type: 'text',
      placeholder: 'owner/repo or https://github.com/…/SKILL.md',
    });
    const fetchBtn = el('button', { class: 'rustic-modal__btn rustic-modal__btn--secondary' }, 'Fetch list');
    urlRow.appendChild(urlInput);
    urlRow.appendChild(fetchBtn);
    body.appendChild(urlRow);

    const status = el('div', { class: 'skills-install-form__status' });
    body.appendChild(status);

    const pickerArea = el('div', { class: 'skills-install-form__picker' });
    body.appendChild(pickerArea);

    let currentSource = '';
    let foundSkills = [];
    const selected = new Set();
    const nameOverrides = new Map(); // path -> user-edited name

    async function fetchList() {
      const src = urlInput.value.trim();
      if (!src) return;
      currentSource = src;
      status.textContent = 'Fetching…';
      status.className = 'skills-install-form__status';
      pickerArea.innerHTML = '';
      installActionBtn.disabled = true;
      try {
        foundSkills = (await api.listRepoSkills(src)) || [];
      } catch (e) {
        status.textContent = String(e).replace(/^Error: /, '');
        status.classList.add('skills-install-form__status--err');
        return;
      }
      status.textContent = `Found ${foundSkills.length} skill(s)`;
      status.classList.add('skills-install-form__status--ok');
      selected.clear();
      nameOverrides.clear();
      renderPicker();
    }

    function renderPicker() {
      pickerArea.innerHTML = '';
      if (foundSkills.length === 0) return;

      // "Select all" header row (shown only when there is more than one item)
      let selectAllCb = null;
      if (foundSkills.length > 1) {
        const head = el('div', { class: 'skills-picker__head' });
        selectAllCb = el('input', { type: 'checkbox', class: 'skills-picker__check' });
        selectAllCb.addEventListener('change', () => {
          const rowCbs = pickerArea.querySelectorAll('.skills-picker__row .skills-picker__check');
          rowCbs.forEach((cb, i) => {
            cb.checked = selectAllCb.checked;
            const s = foundSkills[i];
            if (selectAllCb.checked) selected.add(s.path);
            else selected.delete(s.path);
          });
          installActionBtn.disabled = selected.size === 0;
        });
        head.appendChild(selectAllCb);
        head.appendChild(el('span', { class: 'skills-picker__head-label' }, 'Select all'));
        pickerArea.appendChild(head);
      }

      const syncSelectAll = () => {
        if (!selectAllCb) return;
        selectAllCb.checked = selected.size === foundSkills.length;
        selectAllCb.indeterminate = selected.size > 0 && selected.size < foundSkills.length;
      };

      for (const s of foundSkills) {
        const row = el('div', { class: 'skills-picker__row' });
        const cb = el('input', { type: 'checkbox', class: 'skills-picker__check' });
        cb.addEventListener('change', () => {
          if (cb.checked) selected.add(s.path);
          else selected.delete(s.path);
          installActionBtn.disabled = selected.size === 0;
          syncSelectAll();
        });
        row.appendChild(cb);
        const textCol = el('div', { class: 'skills-picker__text' });
        const nameInput = el('input', {
          class: 'skills-picker__name-input',
          type: 'text',
          value: s.name,
          placeholder: 'Skill name',
        });
        nameOverrides.set(s.path, s.name);
        nameInput.addEventListener('input', () => {
          nameOverrides.set(s.path, nameInput.value);
        });
        nameInput.addEventListener('click', (e) => e.stopPropagation());
        textCol.appendChild(nameInput);
        textCol.appendChild(el('div', { class: 'skills-picker__desc' }, s.description || ''));
        row.appendChild(textCol);

        const viewBtn = el('button', { class: 'skills-picker__view', title: 'Preview skill' });
        viewBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6', 14));
        viewBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          openRepoSkillPreview(currentSource, s);
        });
        row.appendChild(viewBtn);

        row.addEventListener('click', (e) => {
          if (e.target === cb || e.target === nameInput || viewBtn.contains(e.target)) return;
          cb.checked = !cb.checked;
          cb.dispatchEvent(new Event('change'));
        });
        pickerArea.appendChild(row);
      }
      if (foundSkills.length === 1) {
        selected.add(foundSkills[0].path);
        pickerArea.querySelector('.skills-picker__row .skills-picker__check').checked = true;
      }
      installActionBtn.disabled = selected.size === 0;
      syncSelectAll();
    }

    async function openRepoSkillPreview(source, skill) {
      const content = el('div', { class: 'skills-view__content' }, 'Loading…');
      openModal({ title: skill.name, body: content, size: 'lg' });
      try {
        const text = await api.previewRepoSkill(source, skill.path);
        content.innerHTML = '';
        content.appendChild(renderMarkdown(text || ''));
      } catch (e) {
        content.textContent = `Could not load skill: ${e}`;
      }
    }

    fetchBtn.addEventListener('click', fetchList);
    urlInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); fetchList(); } });

    let installActionBtn = null;
    const close = openModal({
      title: 'Install from GitHub',
      body,
      size: 'lg',
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: 'Install selected',
          variant: 'primary',
          onClick: async () => {
            if (selected.size === 0) return false;
            status.textContent = 'Installing…';
            status.className = 'skills-install-form__status';
            try {
              const paths = Array.from(selected);
              const names = paths.map((p) => nameOverrides.get(p) || '');
              await api.installRepoSkills(currentSource, paths, names);
            } catch (e) {
              status.textContent = String(e).replace(/^Error: /, '');
              status.classList.add('skills-install-form__status--err');
              return false;
            }
            loadSkills();
            return true;
          },
        },
      ],
    });
    installActionBtn = close.buttons[1];
    installActionBtn.disabled = true;

    urlInput.focus();
  }

  function openCreateModal(existing = null) {
    const body = el('div', { class: 'skills-edit-form' });
    const nameInput = el('input', {
      class: 'rustic-modal__input',
      type: 'text',
      placeholder: 'e.g. code-review',
    });
    const bodyArea = el('textarea', {
      class: 'rustic-modal__textarea',
      placeholder: 'Full description / instructions (shown to the model when the skill is activated)…',
      rows: 12,
    });

    if (existing) {
      nameInput.value = existing.name;
    }

    body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Title'));
    body.appendChild(nameInput);
    body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Description'));
    body.appendChild(bodyArea);

    const err = el('div', { class: 'skills-install-form__status' });
    body.appendChild(err);

    const title = existing ? 'Edit skill' : 'Create skill';
    const saveLabel = existing ? 'Save' : 'Create';

    openModal({
      title,
      body,
      size: 'lg',
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: saveLabel,
          variant: 'primary',
          onClick: async () => {
            const name = nameInput.value.trim();
            const content = bodyArea.value;
            if (!name || !content.trim()) {
              err.textContent = 'Title and description are required';
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            try {
              if (existing) {
                await api.updateSkill(existing.name, name, content);
              } else {
                await api.createSkill(name, content);
              }
            } catch (e) {
              err.textContent = String(e).replace(/^Error: /, '');
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            loadSkills();
            return true;
          },
        },
      ],
    });

    if (existing) {
      // Prefill body from backend
      api.getSkillBody(existing.name).then((b) => { bodyArea.value = b || ''; }).catch(() => {});
    }
    nameInput.focus();
  }

  function openDeleteModal(skill) {
    const body = el('div', { class: 'rustic-modal__confirm' });
    body.appendChild(el('p', {}, `Delete skill "${skill.name}"? This cannot be undone.`));
    openModal({
      title: 'Delete skill',
      body,
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: 'Delete',
          variant: 'danger',
          onClick: async () => {
            try {
              await api.deleteSkill(skill.name);
              loadSkills();
            } catch (e) {
              await showAlertDialog('Delete failed', String(e));
              return false;
            }
          },
        },
      ],
    });
  }

  // Expose hooks for the collapsible header buttons
  container._onPlus = openAddModal;
  container._onInfo = openInfoModal;

  loadSkills();
  return container;
}
