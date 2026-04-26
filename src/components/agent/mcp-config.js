import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';
import { openMcpJsonModal } from './mcp-json-modal.js';
import { workspaceStore } from '../../state/workspace.js';
import { showConfirmDialog, showAlertDialog } from '../confirm-dialog.js';

/**
 * Header-actions element for the MCP Servers collapsible.
 * Matches the Skills / Workflows / Rules pattern — goes into the 4th arg of createCollapsible.
 */
export function createMcpHeaderActions(onEditJson, onAddNew) {
  const wrap = el('div');

  const editBtn = el('button', {
    class: 'settings-collapsible__action-btn settings-collapsible__action-btn--wide',
    title: 'Edit mcp.json',
  });
  editBtn.appendChild(
    icon('M8 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2h-2M9 3h6v4H9z', 13)
  );
  editBtn.appendChild(el('span', {}, 'Edit JSON'));
  editBtn.addEventListener('click', onEditJson);
  wrap.appendChild(editBtn);

  const plusBtn = el('button', { class: 'settings-collapsible__action-btn', title: 'Add server' });
  plusBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  plusBtn.addEventListener('click', onAddNew);
  wrap.appendChild(plusBtn);

  return wrap;
}

/**
 * MCP server panel. Shows the merged list of servers from both scope files
 * (user `mcp.json` + each project's `.mcp.json`). All editing happens through
 * the JSON modal — there is no field-based form. Each row shows its last-known
 * connection status (connected / failed).
 */
export function createMcpConfig() {
  const container = el('div', { class: 'mcp-config' });
  const serverList = el('div', { class: 'mcp-server-list' });
  container.appendChild(serverList);

  let servers = [];

  async function loadServers() {
    try {
      const projects = workspaceStore.getState('projects') || [];
      const merged = new Map();

      // User scope first — then layer each project's .mcp.json on top.
      const userList = await api.listMcpServers(null);
      for (const s of userList || []) merged.set(s.id, s);

      for (const p of projects) {
        try {
          const list = await api.listMcpServers(p.id);
          for (const s of list || []) merged.set(s.id, s);
        } catch {
          // project without .mcp.json — ignore
        }
      }

      servers = Array.from(merged.values());
      renderList();
    } catch (e) {
      console.error('Failed to load MCP servers:', e);
    }
  }

  function renderList() {
    serverList.innerHTML = '';
    if (servers.length === 0) {
      const empty = el('div', { class: 'mcp-server-list__empty' });
      empty.appendChild(el('div', {}, 'No MCP servers configured.'));
      empty.appendChild(
        el(
          'div',
          { class: 'mcp-server-list__empty-hint' },
          'Click "Edit JSON" to add one. Format matches Claude Code\'s .mcp.json.'
        )
      );
      serverList.appendChild(empty);
      return;
    }

    for (const server of servers) {
      const item = el('div', { class: 'mcp-server' });

      const info = el('div', { class: 'mcp-server__info' });
      const nameRow = el('div', { class: 'mcp-server__name-row' });

      const statusDot = el('span', { class: 'mcp-server__status-dot ' + dotClass(server.status) });
      nameRow.appendChild(statusDot);

      nameRow.appendChild(el('span', { class: 'mcp-server__name' }, server.name));

      const scopeBadgeClass = server.scope === 'project'
        ? 'mcp-server__badge mcp-server__badge--project'
        : 'mcp-server__badge mcp-server__badge--user';
      const scopeBadgeText = server.scope === 'project' ? '.mcp.json' : 'user';
      nameRow.appendChild(el('span', { class: scopeBadgeClass }, scopeBadgeText));

      const statusLabel = statusText(server.status);
      if (statusLabel) {
        const cls = 'mcp-server__status-label mcp-server__status-label--' + statusClass(server.status);
        const labelEl = el('span', { class: cls }, statusLabel);
        if (server.status?.state === 'failed' && server.status.error) {
          labelEl.title = server.status.error;
        }
        nameRow.appendChild(labelEl);
      }

      info.appendChild(nameRow);

      const transport = server.transport.type === 'stdio'
        ? `stdio: ${server.transport.command}${(server.transport.args || []).length ? ' ' + server.transport.args.join(' ') : ''}`
        : `sse: ${server.transport.url}`;
      info.appendChild(el('span', { class: 'mcp-server__transport' }, transport));

      const actions = el('div', { class: 'mcp-server__actions' });

      const testBtn = el('button', { title: 'Re-test connection' });
      testBtn.appendChild(icon('M22 11.08V12a10 10 0 1 1-5.93-9.14', 12));
      testBtn.addEventListener('click', async () => {
        testBtn.disabled = true;
        try {
          await api.testMcpServer(server.id);
        } catch (e) {
          // ignored — status will reflect the failure
        }
        testBtn.disabled = false;
        loadServers();
      });

      const removeBtn = el('button', { title: 'Remove from file' });
      removeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 12));
      removeBtn.addEventListener('click', async () => {
        const ok = await showConfirmDialog(
          'Remove MCP server',
          `Remove "${server.name}" from ${server.scope === 'project' ? '.mcp.json' : 'user mcp.json'}?`,
          { confirmLabel: 'Remove' },
        );
        if (!ok) return;
        try {
          await api.removeMcpServer(server.id);
          loadServers();
        } catch (e) {
          await showAlertDialog('Failed to remove', String(e));
        }
      });

      actions.appendChild(testBtn);
      actions.appendChild(removeBtn);

      item.appendChild(info);
      item.appendChild(actions);
      serverList.appendChild(item);
    }
  }

  // Expose reload for the outer Edit-JSON / + buttons that live on the collapsible header.
  container._reload = loadServers;
  container._openEditJson = () => openMcpJsonModal({ onSaved: loadServers });
  container._openAddNew = () => openMcpJsonModal({ blankTemplate: true, onSaved: loadServers });

  loadServers();
  return container;
}

function dotClass(status) {
  if (!status) return 'mcp-server__status-dot--unknown';
  switch (status.state) {
    case 'connected': return 'mcp-server__status-dot--ok';
    case 'failed':    return 'mcp-server__status-dot--err';
    default:          return 'mcp-server__status-dot--unknown';
  }
}

function statusClass(status) {
  if (!status) return 'unknown';
  return status.state === 'connected' ? 'ok'
       : status.state === 'failed'    ? 'err'
       :                                  'unknown';
}

function statusText(status) {
  if (!status) return 'not connected';
  switch (status.state) {
    case 'connected': return null; // live connection — the green dot is enough
    case 'failed':    return 'failed';
    default:          return 'not connected';
  }
}
