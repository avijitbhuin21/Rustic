import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

export function createMcpConfig() {
  const container = el('div', { class: 'mcp-config' });

  const header = el('div', { class: 'mcp-config__header' });
  header.appendChild(el('span', { class: 'mcp-config__title' }, 'MCP Servers'));

  const addBtn = el('button', { class: 'mcp-config__add', title: 'Add Server' });
  addBtn.appendChild(icon('M12 5v14M5 12h14', 12));
  addBtn.addEventListener('click', showAddForm);
  header.appendChild(addBtn);

  const serverList = el('div', { class: 'mcp-server-list' });
  const formContainer = el('div', { class: 'mcp-form-container' });

  container.appendChild(header);
  container.appendChild(formContainer);
  container.appendChild(serverList);

  let servers = [];

  async function loadServers() {
    try {
      servers = (await api.listMcpServers()) || [];
      renderList();
    } catch (e) {
      console.error('Failed to load MCP servers:', e);
    }
  }

  function renderList() {
    serverList.innerHTML = '';
    if (servers.length === 0) {
      serverList.appendChild(el('div', { class: 'mcp-server-list__empty' }, 'No MCP servers configured'));
      return;
    }

    for (const server of servers) {
      const item = el('div', { class: 'mcp-server' });

      const info = el('div', { class: 'mcp-server__info' });
      info.appendChild(el('span', { class: 'mcp-server__name' }, server.name));
      const transport = server.transport.type === 'stdio'
        ? `stdio: ${server.transport.command}`
        : `sse: ${server.transport.url}`;
      info.appendChild(el('span', { class: 'mcp-server__transport' }, transport));

      const actions = el('div', { class: 'mcp-server__actions' });

      const testBtn = el('button', { title: 'Test Connection' });
      testBtn.appendChild(icon('M22 11.08V12a10 10 0 1 1-5.93-9.14', 12));
      testBtn.addEventListener('click', async () => {
        testBtn.disabled = true;
        try {
          const tools = await api.testMcpServer(server.id);
          alert(`Connected! Found ${tools.length} tool(s):\n${tools.map(t => t.name).join('\n')}`);
        } catch (e) {
          alert(`Connection failed: ${e}`);
        }
        testBtn.disabled = false;
      });

      const removeBtn = el('button', { title: 'Remove' });
      removeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
      removeBtn.addEventListener('click', async () => {
        await api.removeMcpServer(server.id);
        loadServers();
      });

      actions.appendChild(testBtn);
      actions.appendChild(removeBtn);

      item.appendChild(info);
      item.appendChild(actions);
      serverList.appendChild(item);
    }
  }

  function showAddForm() {
    formContainer.innerHTML = '';
    const form = el('div', { class: 'mcp-add-form' });

    const nameInput = el('input', { class: 'mcp-add-form__input', placeholder: 'Server name', type: 'text' });

    const transportSelect = el('select', { class: 'mcp-add-form__select' });
    transportSelect.appendChild(el('option', { value: 'stdio' }, 'Stdio'));
    transportSelect.appendChild(el('option', { value: 'sse' }, 'SSE'));

    const commandInput = el('input', { class: 'mcp-add-form__input', placeholder: 'Command (e.g. npx)', type: 'text' });
    const argsInput = el('input', { class: 'mcp-add-form__input', placeholder: 'Args (comma separated)', type: 'text' });
    const urlInput = el('input', { class: 'mcp-add-form__input', placeholder: 'URL', type: 'text', style: { display: 'none' } });

    transportSelect.addEventListener('change', () => {
      const isStdio = transportSelect.value === 'stdio';
      commandInput.style.display = isStdio ? 'block' : 'none';
      argsInput.style.display = isStdio ? 'block' : 'none';
      urlInput.style.display = isStdio ? 'none' : 'block';
    });

    const btnRow = el('div', { class: 'mcp-add-form__buttons' });
    const saveBtn = el('button', { class: 'mcp-add-form__save' }, 'Add');
    const cancelBtn = el('button', { class: 'mcp-add-form__cancel' }, 'Cancel');

    saveBtn.addEventListener('click', async () => {
      const name = nameInput.value.trim();
      if (!name) return;

      const transportType = transportSelect.value;
      const command = transportType === 'stdio' ? commandInput.value.trim() : null;
      const args = transportType === 'stdio' ? argsInput.value.split(',').map(s => s.trim()).filter(Boolean) : null;
      const url = transportType === 'sse' ? urlInput.value.trim() : null;

      try {
        await api.addMcpServer(name, transportType, command, args, url);
        formContainer.innerHTML = '';
        loadServers();
      } catch (e) {
        console.error('Failed to add MCP server:', e);
      }
    });

    cancelBtn.addEventListener('click', () => {
      formContainer.innerHTML = '';
    });

    btnRow.appendChild(saveBtn);
    btnRow.appendChild(cancelBtn);

    form.appendChild(nameInput);
    form.appendChild(transportSelect);
    form.appendChild(commandInput);
    form.appendChild(argsInput);
    form.appendChild(urlInput);
    form.appendChild(btnRow);
    formContainer.appendChild(form);
  }

  loadServers();
  return container;
}
