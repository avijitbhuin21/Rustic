import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';
import { workspaceStore } from '../../state/workspace.js';

const DEFAULT_TEMPLATE = `{
  "mcpServers": {
    "example-server": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "env": {}
    }
  }
}
`;

/**
 * Open the MCP JSON editor.
 *
 * @param {Object} opts
 * @param {'user'|'project'} [opts.initialScope]
 * @param {boolean} [opts.blankTemplate]  If true, start with the example template instead of the file's current contents.
 * @param {Function} [opts.onSaved]       Called after a successful save so the caller can refresh its list.
 */
export function openMcpJsonModal(opts = {}) {
  const { initialScope = 'user', blankTemplate = false, onSaved } = opts;

  const projects = workspaceStore.getState('projects') || [];
  let scope = initialScope;
  let projectId = projects[0]?.id ?? null;

  const overlay = el('div', { class: 'mcp-modal-overlay' });
  const dialog = el('div', { class: 'mcp-modal' });

  // Header: title + scope tabs
  const header = el('div', { class: 'mcp-modal__header' });
  header.appendChild(el('div', { class: 'mcp-modal__title' }, 'MCP Servers'));

  const tabs = el('div', { class: 'mcp-modal__tabs' });
  const userTab = el('button', { class: 'mcp-modal__tab' }, 'User (global)');
  const projectTab = el('button', { class: 'mcp-modal__tab' }, 'Project');
  tabs.appendChild(userTab);
  tabs.appendChild(projectTab);
  header.appendChild(tabs);

  // Project picker (shown only for project scope)
  const projectPicker = el('select', { class: 'mcp-modal__project-picker' });
  for (const p of projects) {
    projectPicker.appendChild(el('option', { value: p.id }, p.name));
  }
  if (projects.length === 0) {
    const opt = el('option', { value: '', disabled: 'true' }, 'No projects — add one first');
    projectPicker.appendChild(opt);
    projectPicker.disabled = true;
  }

  // Scope-path hint
  const pathHint = el('div', { class: 'mcp-modal__path-hint' });

  // Textarea
  const textarea = el('textarea', {
    class: 'mcp-modal__editor',
    spellcheck: 'false',
    placeholder: DEFAULT_TEMPLATE,
  });

  // Error/status area
  const status = el('div', { class: 'mcp-modal__status' });

  // Footer buttons
  const footer = el('div', { class: 'mcp-modal__footer' });
  const cancelBtn = el('button', { class: 'mcp-modal__btn' }, 'Cancel');
  const saveBtn = el('button', { class: 'mcp-modal__btn mcp-modal__btn--primary' }, 'Save & Connect');
  footer.appendChild(cancelBtn);
  footer.appendChild(saveBtn);

  dialog.appendChild(header);
  dialog.appendChild(projectPicker);
  dialog.appendChild(pathHint);
  dialog.appendChild(textarea);
  dialog.appendChild(status);
  dialog.appendChild(footer);
  overlay.appendChild(dialog);

  function updateScopeUi() {
    userTab.classList.toggle('mcp-modal__tab--active', scope === 'user');
    projectTab.classList.toggle('mcp-modal__tab--active', scope === 'project');
    projectPicker.style.display = scope === 'project' ? 'block' : 'none';

    if (scope === 'user') {
      pathHint.textContent = 'Global — shared across all projects.';
    } else if (projectId) {
      const p = projects.find(x => x.id === projectId);
      pathHint.textContent = `.mcp.json in ${p?.name || 'project'} (committed to source).`;
    } else {
      pathHint.textContent = 'No project selected.';
    }
  }

  async function loadContent() {
    if (blankTemplate) {
      textarea.value = DEFAULT_TEMPLATE;
      return;
    }
    textarea.value = '';
    textarea.disabled = true;
    status.textContent = 'Loading…';
    status.className = 'mcp-modal__status';
    try {
      const projArg = scope === 'project' ? projectId : null;
      if (scope === 'project' && !projArg) {
        textarea.value = DEFAULT_TEMPLATE;
        status.textContent = '';
        return;
      }
      const content = await api.readMcpJson(scope, projArg);
      textarea.value = content || DEFAULT_TEMPLATE;
      status.textContent = '';
    } catch (e) {
      textarea.value = DEFAULT_TEMPLATE;
      status.textContent = `Could not read file: ${e}`;
      status.className = 'mcp-modal__status mcp-modal__status--error';
    } finally {
      textarea.disabled = false;
    }
  }

  function setScope(s) {
    if (s === scope) return;
    scope = s;
    updateScopeUi();
    loadContent();
  }

  userTab.addEventListener('click', () => setScope('user'));
  projectTab.addEventListener('click', () => {
    if (projects.length === 0) {
      status.textContent = 'Add a project first — project scope writes .mcp.json into the project root.';
      status.className = 'mcp-modal__status mcp-modal__status--error';
      return;
    }
    setScope('project');
  });
  projectPicker.addEventListener('change', () => {
    projectId = projectPicker.value || null;
    updateScopeUi();
    loadContent();
  });

  function finish() {
    overlay.remove();
    document.removeEventListener('keydown', onKey);
  }

  function onKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      finish();
    }
  }

  cancelBtn.addEventListener('click', finish);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) finish();
  });

  saveBtn.addEventListener('click', async () => {
    const content = textarea.value;

    // Client-side JSON validation before round-tripping.
    try {
      const parsed = JSON.parse(content);
      if (!parsed || typeof parsed !== 'object' || parsed.mcpServers === undefined) {
        throw new Error('Missing "mcpServers" object at the top level.');
      }
    } catch (e) {
      status.textContent = `JSON error: ${e.message}`;
      status.className = 'mcp-modal__status mcp-modal__status--error';
      return;
    }

    if (scope === 'project' && !projectId) {
      status.textContent = 'Select a project first.';
      status.className = 'mcp-modal__status mcp-modal__status--error';
      return;
    }

    saveBtn.disabled = true;
    cancelBtn.disabled = true;
    status.textContent = 'Saving & connecting…';
    status.className = 'mcp-modal__status';

    try {
      const projArg = scope === 'project' ? projectId : null;
      const results = await api.saveMcpJson(scope, projArg, content);
      renderConnectResults(results);
      if (typeof onSaved === 'function') onSaved();

      // Auto-close when every server connected cleanly. If any failed, keep
      // the modal open so the user can see the error and fix the JSON.
      const allOk = (results || []).length > 0 && (results || []).every(r => r.connected);
      if (allOk) {
        setTimeout(finish, 450);
      }
    } catch (e) {
      status.textContent = `Save failed: ${e}`;
      status.className = 'mcp-modal__status mcp-modal__status--error';
    } finally {
      saveBtn.disabled = false;
      cancelBtn.disabled = false;
    }
  });

  function renderConnectResults(results) {
    status.innerHTML = '';
    status.className = 'mcp-modal__status';

    if (!results || results.length === 0) {
      status.textContent = 'Saved. No servers configured.';
      return;
    }

    const ok = results.filter(r => r.connected);
    const bad = results.filter(r => !r.connected);

    const summary = el(
      'div',
      { class: 'mcp-modal__status-summary' },
      `Saved — ${ok.length} connected, ${bad.length} failed.`
    );
    status.appendChild(summary);

    const list = el('ul', { class: 'mcp-modal__result-list' });
    for (const r of results) {
      const li = el('li', {
        class: 'mcp-modal__result' + (r.connected ? ' mcp-modal__result--ok' : ' mcp-modal__result--err'),
      });
      const mark = el('span', { class: 'mcp-modal__result-mark' }, r.connected ? '✓' : '✗');
      const name = el('span', { class: 'mcp-modal__result-name' }, r.name);
      li.appendChild(mark);
      li.appendChild(name);
      if (r.connected) {
        li.appendChild(el('span', { class: 'mcp-modal__result-detail' }, `${r.toolCount} tool${r.toolCount === 1 ? '' : 's'}`));
      } else if (r.error) {
        li.appendChild(el('span', { class: 'mcp-modal__result-detail' }, r.error));
      }
      list.appendChild(li);
    }
    status.appendChild(list);
  }

  document.body.appendChild(overlay);
  document.addEventListener('keydown', onKey);
  updateScopeUi();
  loadContent();
}
