import { el, icon } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { renderMarkdown } from '../../utils/markdown.js';
import * as api from '../../lib/tauri-api.js';

const WORKFLOW_INFO_HTML = `
  <p><strong>Creating a workflow</strong> — Fill in the title and the full
     description (the prompt sent to the agent when the workflow is triggered).
     The short preview shown in the list is auto-generated from the first 150
     characters of the description.</p>
  <p>Workflows are stored locally at
     <code>~/.rustic/workflows/&lt;name&gt;.md</code> and are available in every
     project.</p>
`;

export function createWorkflowsHeaderActions(onPlus, onInfo) {
  const wrap = el('div');
  const infoBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'About workflows' });
  infoBtn.appendChild(icon('M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 4a1.5 1.5 0 1 1-1.5 1.5A1.5 1.5 0 0 1 12 6zm2 12h-4v-1h1v-5h-1v-1h3v6h1z', 14));
  infoBtn.addEventListener('click', onInfo);
  wrap.appendChild(infoBtn);

  const plusBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add workflow' });
  plusBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  plusBtn.addEventListener('click', onPlus);
  wrap.appendChild(plusBtn);

  return wrap;
}

export function createWorkflowsPanel() {
  const container = el('div', { class: 'workflows-panel' });

  const workflowList = el('div', { class: 'workflows-list' });
  container.appendChild(workflowList);

  let workflows = [];

  async function loadWorkflows() {
    try {
      workflows = (await api.listWorkflows()) || [];
    } catch {
      workflows = [];
    }
    renderList();
  }

  function renderList() {
    workflowList.innerHTML = '';
    if (workflows.length === 0) {
      workflowList.appendChild(el('div', { class: 'workflows-list__empty' }, 'No workflows defined'));
      return;
    }
    for (let i = 0; i < workflows.length; i++) {
      const workflow = workflows[i];
      const item = el('div', { class: 'workflows-item' });

      const nameRow = el('div', { class: 'workflows-item__name-row' });
      nameRow.appendChild(el('span', { class: 'workflows-item__name' }, workflow.name));

      const actions = el('div', { class: 'workflows-item__actions' });

      const viewBtn = el('button', { title: 'View workflow' });
      viewBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6', 14));
      viewBtn.addEventListener('click', () => openViewModal(workflow));
      actions.appendChild(viewBtn);

      const editBtn = el('button', { title: 'Edit workflow' });
      editBtn.appendChild(icon('M12 20h9 M16.5 3.5a2.121 2.121 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z', 14));
      editBtn.addEventListener('click', () => openCreateModal(workflow));
      actions.appendChild(editBtn);

      const deleteBtn = el('button', { title: 'Delete workflow' });
      deleteBtn.appendChild(icon('M3 6h18 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6', 14));
      deleteBtn.addEventListener('click', () => openDeleteModal(workflow));
      actions.appendChild(deleteBtn);

      nameRow.appendChild(actions);
      item.appendChild(nameRow);
      item.appendChild(el('div', { class: 'workflows-item__description' }, workflow.description || ''));

      workflowList.appendChild(item);
      if (i < workflows.length - 1) {
        workflowList.appendChild(el('div', { class: 'workflows-item__divider' }));
      }
    }
  }

  async function openViewModal(workflow) {
    const content = el('div', { class: 'skills-view__content' }, 'Loading…');
    openModal({ title: workflow.name, body: content, size: 'lg' });

    try {
      const text = await api.getWorkflowBody(workflow.name);
      content.innerHTML = '';
      content.appendChild(renderMarkdown(text || ''));
    } catch (e) {
      content.textContent = `Could not load workflow: ${e}`;
    }
  }

  function openInfoModal() {
    const body = el('div', { class: 'rustic-modal__info' });
    body.innerHTML = WORKFLOW_INFO_HTML;
    openModal({ title: 'About workflows', body, buttons: [{ label: 'Close' }] });
  }

  function openCreateModal(existing = null) {
    const body = el('div', { class: 'skills-edit-form' });
    const nameInput = el('input', {
      class: 'rustic-modal__input',
      type: 'text',
      placeholder: 'e.g. deploy-staging',
    });
    const bodyArea = el('textarea', {
      class: 'rustic-modal__textarea',
      placeholder: 'Full description / prompt sent to the agent when this workflow is triggered…',
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

    openModal({
      title: existing ? 'Edit workflow' : 'Create workflow',
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
              err.textContent = 'Title and description are required';
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            try {
              if (existing) {
                await api.updateWorkflow(existing.name, name, content);
              } else {
                await api.createWorkflow(name, content);
              }
            } catch (e) {
              err.textContent = String(e).replace(/^Error: /, '');
              err.className = 'skills-install-form__status skills-install-form__status--err';
              return false;
            }
            loadWorkflows();
            return true;
          },
        },
      ],
    });

    if (existing) {
      api.getWorkflowBody(existing.name).then((b) => { bodyArea.value = b || ''; }).catch(() => {});
    }
    nameInput.focus();
  }

  function openDeleteModal(workflow) {
    const body = el('div', { class: 'rustic-modal__confirm' });
    body.appendChild(el('p', {}, `Delete workflow "${workflow.name}"? This cannot be undone.`));
    openModal({
      title: 'Delete workflow',
      body,
      buttons: [
        { label: 'Cancel', variant: 'secondary' },
        {
          label: 'Delete',
          variant: 'danger',
          onClick: async () => {
            try {
              await api.deleteWorkflow(workflow.name);
              loadWorkflows();
            } catch (e) {
              alert(`Delete failed: ${e}`);
              return false;
            }
          },
        },
      ],
    });
  }

  container._onPlus = () => openCreateModal();
  container._onInfo = openInfoModal;

  loadWorkflows();
  return container;
}
