import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

/**
 * Workflows panel for listing, creating, and deleting workflows.
 * @param {string} projectId — the current project ID
 */
export function createWorkflowsPanel(projectId) {
  const container = el('div', { class: 'workflows-panel' });

  // Header
  const header = el('div', { class: 'workflows-panel__header' });
  header.appendChild(el('span', { class: 'workflows-panel__title' }, 'Workflows'));

  const headerActions = el('div', { class: 'workflows-panel__header-actions' });
  const createBtn = el('button', { class: 'workflows-panel__btn', title: 'Create Workflow' });
  createBtn.appendChild(icon('M12 5v14M5 12h14', 12));
  createBtn.addEventListener('click', () => showCreateForm());
  headerActions.appendChild(createBtn);
  header.appendChild(headerActions);

  container.appendChild(header);

  // Form container (create form and body modal render here)
  const formContainer = el('div', { class: 'workflows-form-container' });
  container.appendChild(formContainer);

  // Workflows list
  const workflowList = el('div', { class: 'workflows-list' });
  container.appendChild(workflowList);

  let workflows = [];

  async function loadWorkflows() {
    try {
      workflows = (await api.listWorkflows(projectId)) || [];
    } catch (e) {
      workflows = [];
    }
    renderList();
  }

  function renderList() {
    workflowList.innerHTML = '';
    if (workflows.length === 0) {
      workflowList.appendChild(
        el('div', { class: 'workflows-list__empty' }, 'No workflows defined')
      );
      return;
    }
    for (const workflow of workflows) {
      const item = el('div', { class: 'workflows-item' });

      const info = el('div', { class: 'workflows-item__info' });
      info.appendChild(el('span', { class: 'workflows-item__name' }, workflow.name));
      if (workflow.description) {
        info.appendChild(
          el('span', { class: 'workflows-item__description' }, workflow.description)
        );
      }

      const actions = el('div', { class: 'workflows-item__actions' });

      // Trigger button (play icon)
      const triggerBtn = el('button', { title: 'Trigger workflow' });
      triggerBtn.appendChild(icon('M5 3l14 9-14 9V3z', 12));
      triggerBtn.addEventListener('click', async () => {
        try {
          const body = await api.getWorkflowBody(projectId, workflow.name);
          document.dispatchEvent(
            new CustomEvent('workflow-trigger', { detail: { name: workflow.name, body } })
          );
        } catch (e) {
          alert(`Could not load workflow: ${e}`);
        }
      });

      // View body button (eye icon)
      const viewBtn = el('button', { title: 'View workflow body' });
      viewBtn.appendChild(
        icon(
          'M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6',
          12
        )
      );
      viewBtn.addEventListener('click', async () => {
        try {
          const body = await api.getWorkflowBody(projectId, workflow.name);
          showBodyModal(workflow.name, body);
        } catch (e) {
          alert(`Could not load workflow: ${e}`);
        }
      });

      // Delete button
      const deleteBtn = el('button', { title: 'Delete workflow' });
      deleteBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
      deleteBtn.addEventListener('click', async () => {
        if (!confirm(`Delete workflow "${workflow.name}"?`)) return;
        try {
          await api.deleteWorkflow(projectId, workflow.name);
          loadWorkflows();
        } catch (e) {
          alert(`Delete failed: ${e}`);
        }
      });

      actions.appendChild(triggerBtn);
      actions.appendChild(viewBtn);
      actions.appendChild(deleteBtn);
      item.appendChild(info);
      item.appendChild(actions);
      workflowList.appendChild(item);
    }
  }

  function showCreateForm() {
    formContainer.innerHTML = '';
    const form = el('div', { class: 'workflows-create-form' });

    const nameInput = el('input', {
      class: 'workflows-create-form__input',
      type: 'text',
      placeholder: 'Workflow name (e.g. deploy-staging)',
    });
    const descInput = el('input', {
      class: 'workflows-create-form__input',
      type: 'text',
      placeholder: 'Short description',
    });
    const bodyArea = el('textarea', {
      class: 'workflows-create-form__textarea',
      placeholder: 'Workflow prompt sent to the agent when triggered…',
      rows: 6,
    });

    const btnRow = el('div', { class: 'workflows-create-form__buttons' });
    const saveBtn = el('button', { class: 'workflows-create-form__save' }, 'Create');
    const cancelBtn = el('button', { class: 'workflows-create-form__cancel' }, 'Cancel');

    saveBtn.addEventListener('click', async () => {
      const name = nameInput.value.trim();
      const description = descInput.value.trim();
      const body = bodyArea.value.trim();
      if (!name || !description) return;
      try {
        await api.createWorkflow(projectId, name, description, body);
        formContainer.innerHTML = '';
        loadWorkflows();
      } catch (e) {
        alert(`Create failed: ${e}`);
      }
    });

    cancelBtn.addEventListener('click', () => {
      formContainer.innerHTML = '';
    });

    btnRow.appendChild(saveBtn);
    btnRow.appendChild(cancelBtn);

    form.appendChild(el('div', { class: 'workflows-create-form__label' }, 'New Workflow'));
    form.appendChild(nameInput);
    form.appendChild(descInput);
    form.appendChild(bodyArea);
    form.appendChild(btnRow);
    formContainer.appendChild(form);
    nameInput.focus();
  }

  function showBodyModal(name, body) {
    formContainer.innerHTML = '';
    const modal = el('div', { class: 'workflows-body-modal' });
    modal.appendChild(el('div', { class: 'workflows-body-modal__title' }, name));
    const pre = el('pre', { class: 'workflows-body-modal__body' });
    pre.textContent = body;
    const closeBtn = el('button', { class: 'workflows-body-modal__close' }, 'Close');
    closeBtn.addEventListener('click', () => {
      formContainer.innerHTML = '';
    });
    modal.appendChild(pre);
    modal.appendChild(closeBtn);
    formContainer.appendChild(modal);
  }

  loadWorkflows();
  return container;
}
