import { el, icon } from '../../utils/dom.js';
import { agentStore, sendMessage, setTaskPermissions, respondToPermission } from '../../state/agent.js';
import * as api from '../../lib/tauri-api.js';
import { loadProviderConfigs } from '../settings/ai-settings.js';
import { marked } from 'marked';

function abbreviateModel(model) {
  if (!model) return '?';
  if (model.includes('claude-opus'))   return 'Opus '    + (model.match(/(\d+\.\d+|\d+)/)?.[0] ?? '');
  if (model.includes('claude-sonnet')) return 'Sonnet '  + (model.match(/(\d+\.\d+|\d+)/)?.[0] ?? '');
  if (model.includes('claude-haiku'))  return 'Haiku '   + (model.match(/(\d+\.\d+|\d+)/)?.[0] ?? '');
  if (model.startsWith('gpt-'))        return model.replace('gpt-', 'GPT-');
  if (/^o\d/.test(model))              return model.toUpperCase();
  if (model.includes('gemini'))        return model.replace('gemini-', 'Gemini ').replace('-', ' ');
  return model.length > 20 ? model.slice(0, 18) + '…' : model;
}

const TURN_LIMIT_REACHED = 'TurnLimitReached';

export function createChatView() {
  const container = el('div', { class: 'chat-view' });

  // Chat header bar (cost / status)
  const headerBar = el('div', { class: 'chat-header-bar' });
  const headerTitle = el('div', { class: 'chat-header-bar__title' });
  const costDisplay = el('div', { class: 'chat-header-bar__cost', title: 'Token usage and estimated cost' });
  const headerStop = el('button', { class: 'chat-header-bar__stop chat-header-bar__stop--hidden', title: 'Stop task' });
  headerStop.appendChild(icon('M18 6L6 18M6 6l12 12', 13));
  headerStop.appendChild(el('span', {}, 'Stop'));
  headerStop.addEventListener('click', async () => {
    const taskId = agentStore.getState('activeTaskId');
    if (taskId) { headerStop.disabled = true; try { await api.abortTask(taskId); } finally { headerStop.disabled = false; } }
  });
  headerBar.appendChild(headerTitle);
  headerBar.appendChild(costDisplay);
  headerBar.appendChild(headerStop);

  function updateCostDisplay() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) { costDisplay.textContent = ''; return; }
    const task = agentStore.getState('tasks')[taskId];
    const cost = task?.cost;
    if (!cost) { costDisplay.textContent = ''; return; }

    const totalTokens = (cost.total_input_tokens || 0) + (cost.total_output_tokens || 0);
    const usd = cost.estimated_cost_usd || 0;
    const tokensStr = totalTokens >= 1000
      ? `~${(totalTokens / 1000).toFixed(1)}k tokens`
      : `~${totalTokens} tokens`;
    const costStr = usd > 0
      ? usd < 0.001 ? `<$0.001` : `$${usd.toFixed(3)}`
      : '';

    costDisplay.textContent = costStr ? `${tokensStr} · ${costStr}` : tokensStr;
    costDisplay.title = [
      `Input: ${cost.total_input_tokens?.toLocaleString() ?? 0}`,
      `Output: ${cost.total_output_tokens?.toLocaleString() ?? 0}`,
      cost.total_cache_read_tokens > 0 ? `Cache read: ${cost.total_cache_read_tokens?.toLocaleString()}` : null,
      `Turns: ${cost.turn_count ?? 0}`,
      `Est. cost: $${usd.toFixed(4)}`,
    ].filter(Boolean).join('\n');
  }

  function updateHeaderBar() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      headerTitle.textContent = '';
      headerStop.classList.add('chat-header-bar__stop--hidden');
      return;
    }
    const task = agentStore.getState('tasks')[taskId];
    headerTitle.textContent = task?.title || '';

    const isRunning = task?.status === 'Running';
    headerStop.classList.toggle('chat-header-bar__stop--hidden', !isRunning);
  }

  // Messages area
  const messagesArea = el('div', { class: 'chat-messages' });

  // Approval requests area (shown between messages and input)
  const approvalArea = el('div', { class: 'chat-approval-area' });

  // Sub-agents panel (shown when active sub-agents exist)
  const subagentsPanel = el('div', { class: 'chat-subagents-panel chat-subagents-panel--hidden' });
  let subagentExpandedIds = new Set();

  function renderSubagentsPanel() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      subagentsPanel.classList.add('chat-subagents-panel--hidden');
      return;
    }
    const allSubagents = agentStore.getState('subagents');
    const taskAgents = allSubagents[taskId];
    if (!taskAgents || Object.keys(taskAgents).length === 0) {
      subagentsPanel.classList.add('chat-subagents-panel--hidden');
      return;
    }

    subagentsPanel.classList.remove('chat-subagents-panel--hidden');
    subagentsPanel.innerHTML = '';

    const entries = Object.values(taskAgents);
    const runningCount = entries.filter((a) => a.status === 'running').length;

    const header = el('div', { class: 'chat-subagents-header' });
    const title = el('span', { class: 'chat-subagents-header__title' },
      runningCount > 0 ? `Sub-agents (${runningCount} running)` : `Sub-agents (${entries.length} done)`
    );
    header.appendChild(title);
    subagentsPanel.appendChild(header);

    for (const agent of entries) {
      const row = el('div', { class: 'chat-subagent-row' });
      const statusDot = el('span', { class: `chat-subagent-row__status chat-subagent-row__status--${agent.status}` });
      if (agent.status === 'running') {
        statusDot.appendChild(el('span', { class: 'chat-subagent-spinner' }));
      } else if (agent.status === 'completed') {
        statusDot.textContent = '✓';
      } else {
        statusDot.textContent = '✕';
      }
      const idLabel = el('span', { class: 'chat-subagent-row__id' }, agent.agentId);
      const modelLabel = el('span', { class: 'chat-subagent-row__model' }, abbreviateModel(agent.model));

      row.appendChild(statusDot);
      row.appendChild(idLabel);
      row.appendChild(modelLabel);

      const isExpanded = subagentExpandedIds.has(agent.agentId);
      if (isExpanded) row.classList.add('chat-subagent-row--expanded');

      row.addEventListener('click', () => {
        if (subagentExpandedIds.has(agent.agentId)) {
          subagentExpandedIds.delete(agent.agentId);
        } else {
          subagentExpandedIds.add(agent.agentId);
        }
        renderSubagentsPanel();
      });

      subagentsPanel.appendChild(row);

      if (isExpanded && agent.output) {
        const output = el('div', { class: 'chat-subagent-output' });
        output.textContent = agent.output;
        subagentsPanel.appendChild(output);
      }
    }
  }

  // Turn budget warning banner (shown above input when budget is low or exhausted)
  const budgetBanner = el('div', { class: 'chat-budget-banner chat-budget-banner--hidden' });

  function renderBudgetBanner() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) { budgetBanner.classList.add('chat-budget-banner--hidden'); return; }

    const task = agentStore.getState('tasks')[taskId];
    const warnings = agentStore.getState('turnBudgetWarnings');
    const warning = warnings[taskId];
    const isLimitReached = task?.status === TURN_LIMIT_REACHED;

    budgetBanner.innerHTML = '';

    if (isLimitReached) {
      budgetBanner.classList.remove('chat-budget-banner--hidden');
      budgetBanner.classList.add('chat-budget-banner--limit');
      budgetBanner.classList.remove('chat-budget-banner--warn');
      const msg = el('span', { class: 'chat-budget-banner__msg' }, 'Turn limit reached — agent stopped.');
      const continueBtn = el('button', { class: 'chat-budget-banner__btn' }, 'Continue (+20 turns)');
      continueBtn.addEventListener('click', async () => {
        continueBtn.disabled = true;
        try {
          await api.extendTurnBudget(taskId, 20);
          await sendMessage(taskId, 'Please continue from where you left off.');
        } catch (e) {
          console.error('Failed to continue task:', e);
          continueBtn.disabled = false;
        }
      });
      budgetBanner.appendChild(msg);
      budgetBanner.appendChild(continueBtn);
    } else if (warning) {
      budgetBanner.classList.remove('chat-budget-banner--hidden');
      budgetBanner.classList.remove('chat-budget-banner--limit');
      budgetBanner.classList.add('chat-budget-banner--warn');
      budgetBanner.textContent = `${warning.turns_remaining} turns remaining — agent is wrapping up.`;
    } else {
      budgetBanner.classList.add('chat-budget-banner--hidden');
      budgetBanner.classList.remove('chat-budget-banner--limit', 'chat-budget-banner--warn');
    }
  }

  // Input area
  const inputArea = el('div', { class: 'chat-input-area' });
  const textarea = el('textarea', {
    class: 'chat-input',
    placeholder: 'Send a message...',
    rows: '2',
  });

  // Bottom toolbar: mode pill + send button
  const inputToolbar = el('div', { class: 'chat-input-toolbar' });

  // Model selector
  const modelBtn = el('button', { class: 'chat-model-btn', title: 'Switch model' });
  let modelDropdownOpen = false;
  let modelDropdown = null;
  let aiConfig = null;

  async function loadAiConfig() {
    try { aiConfig = await api.getAiConfig(); } catch { aiConfig = null; }
  }
  loadAiConfig();

  function getCurrentModel() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return '';
    const task = agentStore.getState('tasks')[taskId];
    return task?.model || task?.info?.model || '';
  }

  function getCurrentProviderType() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return '';
    const task = agentStore.getState('tasks')[taskId];
    return task?.provider_type || task?.info?.provider_type || task?.providerType || '';
  }

  function updateModelBtn() {
    const model = getCurrentModel();
    modelBtn.textContent = '';
    const label = el('span', {}, abbreviateModel(model) || 'Model');
    modelBtn.appendChild(label);
    modelBtn.appendChild(icon('M19 9l-7 7-7-7', 10));
  }

  function closeModelDropdown() {
    if (modelDropdown) {
      modelDropdown.remove();
      modelDropdown = null;
      modelDropdownOpen = false;
    }
  }

  modelBtn.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (modelDropdownOpen) { closeModelDropdown(); return; }

    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;

    // Build model list from locally-cached provider configs (all connected providers)
    const configs = loadProviderConfigs();
    const providerEntries = Object.entries(configs)
      .filter(([, cfg]) => cfg.apiKey && cfg.models?.length);

    if (providerEntries.length === 0) {
      // Fall back to backend config if nothing cached locally
      if (!aiConfig) await loadAiConfig();
      if (!aiConfig?.providers?.length) return;
    }

    modelDropdownOpen = true;
    modelDropdown = el('div', { class: 'chat-model-dropdown' });
    const currentModel = getCurrentModel();

    if (providerEntries.length > 0) {
      for (const [providerId, cfg] of providerEntries) {
        const groupHeader = el('div', { class: 'chat-model-dropdown__group' }, providerId);
        modelDropdown.appendChild(groupHeader);

        for (const modelId of cfg.models) {
          const item = el('div', {
            class: `chat-model-dropdown__item${modelId === currentModel ? ' chat-model-dropdown__item--active' : ''}`,
          });
          item.textContent = modelId;
          item.title = modelId;
          item.addEventListener('click', async (ev) => {
            ev.stopPropagation();
            closeModelDropdown();
            try {
              await api.switchModel(taskId, providerId, modelId);
            } catch (err) {
              console.error('Failed to switch model:', err);
            }
          });
          modelDropdown.appendChild(item);
        }
      }
    } else {
      // Fallback: backend config only has default models
      for (const provider of (aiConfig?.providers || []).filter((p) => p.enabled)) {
        const groupHeader = el('div', { class: 'chat-model-dropdown__group' }, provider.provider_type);
        modelDropdown.appendChild(groupHeader);
        const modelId = provider.default_model;
        if (!modelId) continue;
        const item = el('div', {
          class: `chat-model-dropdown__item${modelId === currentModel ? ' chat-model-dropdown__item--active' : ''}`,
        });
        item.textContent = modelId;
        item.title = modelId;
        item.addEventListener('click', async (ev) => {
          ev.stopPropagation();
          closeModelDropdown();
          try { await api.switchModel(taskId, provider.provider_type, modelId); } catch {}
        });
        modelDropdown.appendChild(item);
      }
    }

    inputArea.appendChild(modelDropdown);
  });

  document.addEventListener('click', closeModelDropdown);

  // Permission mode pill
  const modePill = el('button', { class: 'chat-mode-pill', title: 'Switch permission mode' });
  let modeDropdownOpen = false;
  let modeDropdown = null;

  const MODES = [
    { value: 'Chat',       label: 'Chat',        desc: 'Read-only — no file writes or commands' },
    { value: 'ManualEdit', label: 'Manual Edit',  desc: 'Approve each write and command' },
    { value: 'AutoEdit',   label: 'Auto Edit',    desc: 'Writes auto-allowed, commands need approval' },
    { value: 'FullAuto',   label: 'Full Auto',    desc: 'Everything runs without approval' },
  ];

  function getCurrentMode() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return 'ManualEdit';
    const task = agentStore.getState('tasks')[taskId];
    return task?.permissionLevel || 'ManualEdit';
  }

  function updateModePill() {
    const current = getCurrentMode();
    const mode = MODES.find((m) => m.value === current) || MODES[1];
    modePill.innerHTML = '';
    const dot = el('span', { class: `chat-mode-pill__dot chat-mode-pill__dot--${current.toLowerCase()}` });
    modePill.appendChild(dot);
    modePill.appendChild(el('span', {}, mode.label));
    modePill.appendChild(icon('M19 9l-7 7-7-7', 10));
  }

  function closeModeDropdown() {
    if (modeDropdown) {
      modeDropdown.remove();
      modeDropdown = null;
      modeDropdownOpen = false;
    }
  }

  modePill.addEventListener('click', (e) => {
    e.stopPropagation();
    if (modeDropdownOpen) { closeModeDropdown(); return; }

    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;

    modeDropdownOpen = true;
    modeDropdown = el('div', { class: 'chat-mode-dropdown' });
    const current = getCurrentMode();

    for (const mode of MODES) {
      const item = el('div', { class: `chat-mode-dropdown__item${mode.value === current ? ' chat-mode-dropdown__item--active' : ''}` });
      const labelRow = el('div', { class: 'chat-mode-dropdown__label' });
      const dot = el('span', { class: `chat-mode-pill__dot chat-mode-pill__dot--${mode.value.toLowerCase()}` });
      labelRow.appendChild(dot);
      labelRow.appendChild(el('span', {}, mode.label));
      item.appendChild(labelRow);
      item.appendChild(el('div', { class: 'chat-mode-dropdown__desc' }, mode.desc));
      item.addEventListener('click', async (ev) => {
        ev.stopPropagation();
        closeModeDropdown();
        const ok = await setTaskPermissions(taskId, mode.value);
        if (ok) updateModePill();
      });
      modeDropdown.appendChild(item);
    }

    // Position above the pill
    inputArea.appendChild(modeDropdown);
  });

  document.addEventListener('click', closeModeDropdown);

  const sendBtn = el('button', { class: 'chat-send-btn', title: 'Send' });
  sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 16));

  // Attached files state
  let attachedFiles = []; // Array of { name, type, base64? }

  // Slash command picker state
  let slashPickerItems = [];    // all loaded items: { type, name, description, body? }
  let slashPickerFiltered = []; // filtered by current query
  let slashPickerIndex = 0;     // keyboard-selected index
  let slashPickerOpen = false;
  let slashPickerLoaded = false;

  // Attachment pills container (above textarea)
  const attachmentPills = el('div', { class: 'chat-attachments' });
  attachmentPills.style.display = 'none';

  function renderAttachmentPills() {
    attachmentPills.innerHTML = '';
    if (attachedFiles.length === 0) {
      attachmentPills.style.display = 'none';
      return;
    }
    attachmentPills.style.display = 'flex';
    for (let i = 0; i < attachedFiles.length; i++) {
      const f = attachedFiles[i];
      const pill = el('div', { class: 'chat-attachment-pill' });
      if (f.base64 && f.type.startsWith('image/')) {
        const img = el('img', { class: 'chat-attachment-pill__thumb', src: `data:${f.type};base64,${f.base64}` });
        pill.appendChild(img);
      }
      pill.appendChild(el('span', { class: 'chat-attachment-pill__name' }, f.name));
      const removeBtn = el('button', { class: 'chat-attachment-pill__remove' }, '×');
      const idx = i;
      removeBtn.addEventListener('click', () => {
        attachedFiles.splice(idx, 1);
        renderAttachmentPills();
      });
      pill.appendChild(removeBtn);
      attachmentPills.appendChild(pill);
    }
  }

  function readFileAsBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = (e) => resolve(e.target.result.split(',')[1]);
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }

  // Attach file button
  const attachBtn = el('button', { class: 'chat-attach-btn', title: 'Attach file' });
  attachBtn.appendChild(icon('M15.172 7l-6.586 6.586a2 2 0 102.828 2.828l6.414-6.586a4 4 0 00-5.656-5.656l-6.415 6.585a6 6 0 108.486 8.486L20.5 13', 14));

  const fileInput = el('input', { type: 'file', style: 'display:none', multiple: true });
  attachBtn.addEventListener('click', () => fileInput.click());

  fileInput.addEventListener('change', async () => {
    for (const file of fileInput.files) {
      if (file.type.startsWith('image/')) {
        const base64 = await readFileAsBase64(file);
        attachedFiles.push({ name: file.name, type: file.type, base64 });
      } else {
        attachedFiles.push({ name: file.name, type: file.type });
      }
    }
    fileInput.value = '';
    renderAttachmentPills();
  });

  textarea.addEventListener('paste', async (e) => {
    for (const item of e.clipboardData.items) {
      if (item.type.startsWith('image/')) {
        const file = item.getAsFile();
        if (file) {
          const base64 = await readFileAsBase64(file);
          attachedFiles.push({ name: `pasted-image.${file.type.split('/')[1] || 'png'}`, type: file.type, base64 });
          renderAttachmentPills();
        }
      }
    }
  });

  // Slash picker overlay
  const slashPicker = el('div', { class: 'slash-picker slash-picker--hidden' });
  inputArea.appendChild(slashPicker);

  async function loadSlashItems() {
    if (slashPickerLoaded) return;
    slashPickerLoaded = true;
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;
    const tasks = agentStore.getState('tasks');
    const task = tasks[taskId];
    const projectId = task?.project_id || task?.projectId;
    if (!projectId) return;

    const results = [];

    // Load skills
    try {
      const skills = await api.listSkills(projectId);
      for (const s of (skills || [])) {
        results.push({ type: 'skill', name: s.name, description: s.description });
      }
    } catch {}

    // Load workflows
    try {
      const workflows = await api.listWorkflows(projectId);
      for (const w of (workflows || [])) {
        results.push({ type: 'workflow', name: w.name, description: w.description });
      }
    } catch {}

    // Load MCP servers
    try {
      const servers = await api.listMcpServers();
      for (const s of (servers || [])) {
        results.push({ type: 'mcp', name: s.name, description: s.description || `MCP: ${s.name}` });
      }
    } catch {}

    slashPickerItems = results;
  }

  function getSlashContext(ta) {
    const value = ta.value;
    const cursor = ta.selectionStart;
    const before = value.slice(0, cursor);
    // Match a `/` that starts at position 0 or after whitespace/newline
    const match = before.match(/(^|\s)(\/\S*)$/);
    if (!match) return null;
    const slashStart = before.length - match[2].length;
    const query = match[2].slice(1); // text after the `/`
    return { slashStart, slashEnd: cursor, query };
  }

  function filterSlashItems(query) {
    if (!query) return slashPickerItems.slice(0, 12);
    const q = query.toLowerCase();
    return slashPickerItems
      .filter(item =>
        item.name.toLowerCase().includes(q) ||
        (item.description || '').toLowerCase().includes(q)
      )
      .sort((a, b) => {
        // Prefer prefix matches
        const aPfx = a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const bPfx = b.name.toLowerCase().startsWith(q) ? 0 : 1;
        return aPfx - bPfx;
      })
      .slice(0, 10);
  }

  function renderSlashPicker() {
    slashPicker.innerHTML = '';
    if (!slashPickerOpen || slashPickerFiltered.length === 0) {
      slashPicker.classList.add('slash-picker--hidden');
      return;
    }
    slashPicker.classList.remove('slash-picker--hidden');

    for (let i = 0; i < slashPickerFiltered.length; i++) {
      const item = slashPickerFiltered[i];
      const row = el('div', {
        class: `slash-picker__item${i === slashPickerIndex ? ' slash-picker__item--active' : ''}`,
      });

      const typeBadge = el('span', { class: `slash-picker__badge slash-picker__badge--${item.type}` });
      typeBadge.textContent = item.type === 'skill' ? 'Skill' : item.type === 'workflow' ? 'Workflow' : 'MCP';
      row.appendChild(typeBadge);

      const nameEl = el('span', { class: 'slash-picker__name' }, item.name);
      row.appendChild(nameEl);

      if (item.description) {
        const descEl = el('span', { class: 'slash-picker__desc' }, item.description);
        row.appendChild(descEl);
      }

      row.addEventListener('mousedown', (e) => {
        e.preventDefault(); // prevent textarea blur
        selectSlashItem(item);
      });

      slashPicker.appendChild(row);
    }
  }

  function insertSlashToken(ctx, token) {
    const value = textarea.value;
    const newValue = value.slice(0, ctx.slashStart) + token + value.slice(ctx.slashEnd);
    textarea.value = newValue;
    textarea.selectionStart = textarea.selectionEnd = ctx.slashStart + token.length;
  }

  async function selectSlashItem(item) {
    const ctx = getSlashContext(textarea);
    if (!ctx) { closeSlashPicker(); return; }

    closeSlashPicker();

    if (item.type === 'workflow') {
      const taskId = agentStore.getState('activeTaskId');
      const task = agentStore.getState('tasks')[taskId];
      const projectId = task?.project_id || task?.projectId;
      try {
        const body = await api.getWorkflowBody(projectId, item.name);
        const value = textarea.value;
        const newValue = value.slice(0, ctx.slashStart) + body + value.slice(ctx.slashEnd);
        textarea.value = newValue;
        textarea.selectionStart = textarea.selectionEnd = ctx.slashStart + body.length;
      } catch {
        insertSlashToken(ctx, `/${item.name}`);
      }
    } else if (item.type === 'skill') {
      insertSlashToken(ctx, `@${item.name}`);
    } else {
      // MCP
      insertSlashToken(ctx, `@${item.name}`);
    }
    textarea.focus();
  }

  function openSlashPicker(query) {
    slashPickerOpen = true;
    slashPickerFiltered = filterSlashItems(query);
    slashPickerIndex = 0;
    renderSlashPicker();
  }

  function closeSlashPicker() {
    slashPickerOpen = false;
    slashPicker.classList.add('slash-picker--hidden');
  }

  sendBtn.addEventListener('click', () => {
    const taskId = agentStore.getState('activeTaskId');
    const text = textarea.value.trim();
    if (!taskId || (!text && attachedFiles.length === 0)) return;

    if (attachedFiles.length > 0) {
      const imageNames = attachedFiles.filter(f => f.base64).map(f => f.name).join(', ');
      const fullText = imageNames ? `${text}\n\n[Attached images: ${imageNames}]` : text;
      sendMessage(taskId, fullText || text);
    } else {
      sendMessage(taskId, text);
    }

    textarea.value = '';
    attachedFiles = [];
    renderAttachmentPills();
  });

  textarea.addEventListener('keydown', (e) => {
    // Handle picker navigation when open
    if (slashPickerOpen) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        slashPickerIndex = Math.min(slashPickerIndex + 1, slashPickerFiltered.length - 1);
        renderSlashPicker();
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        slashPickerIndex = Math.max(slashPickerIndex - 1, 0);
        renderSlashPicker();
        return;
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        const item = slashPickerFiltered[slashPickerIndex];
        if (item) selectSlashItem(item);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closeSlashPicker();
        return;
      }
    }

    // Normal enter to send
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendBtn.click();
    }
  });

  textarea.addEventListener('input', async () => {
    const ctx = getSlashContext(textarea);
    if (ctx) {
      if (!slashPickerLoaded) await loadSlashItems();
      openSlashPicker(ctx.query);
    } else {
      if (slashPickerOpen) closeSlashPicker();
    }
  });

  textarea.addEventListener('blur', () => {
    setTimeout(() => closeSlashPicker(), 150);
  });

  inputToolbar.appendChild(modelBtn);
  inputToolbar.appendChild(modePill);
  inputToolbar.appendChild(attachBtn);
  inputArea.appendChild(fileInput);
  inputToolbar.appendChild(sendBtn);

  inputArea.appendChild(attachmentPills);
  inputArea.appendChild(textarea);
  inputArea.appendChild(inputToolbar);

  container.appendChild(headerBar);
  container.appendChild(messagesArea);
  container.appendChild(approvalArea);
  container.appendChild(subagentsPanel);
  container.appendChild(budgetBanner);
  container.appendChild(inputArea);

  // Listen for workflow-trigger events and insert the body into the chat input
  document.addEventListener('workflow-trigger', (e) => {
    const body = e.detail?.body;
    if (!body) return;
    if (textarea.value.trim()) {
      textarea.value = body + '\n\n' + textarea.value;
    } else {
      textarea.value = body;
    }
    textarea.focus();
  });

  // Track loaded checkpoints
  let checkpoints = [];

  async function loadCheckpoints(taskId) {
    try {
      checkpoints = (await api.listCheckpoints(taskId)) || [];
    } catch {
      checkpoints = [];
    }
  }

  function hasFileChanges(msg) {
    if (!msg.content) return false;
    return msg.content.some(
      (b) => b.type === 'tool_use' && (b.name === 'write_file' || b.name === 'create_file')
    );
  }

  function findCheckpointForMessage(msgIndex) {
    // Find the checkpoint whose message_index is <= msgIndex
    // Checkpoints are created at user message time, so find the closest one at or before this index
    for (let i = checkpoints.length - 1; i >= 0; i--) {
      if (checkpoints[i].message_index <= msgIndex) {
        return checkpoints[i];
      }
    }
    return null;
  }

  async function handleRevert(checkpoint) {
    // Preview first
    let changes;
    try {
      changes = await api.previewCheckpoint(checkpoint.id);
    } catch (e) {
      console.error('Failed to preview checkpoint:', e);
      return;
    }

    if (!changes || changes.length === 0) return;

    // Build confirmation message
    const fileList = changes
      .map((c) => `${c.change_type === 'delete' ? 'Delete' : 'Restore'}: ${c.file_path}`)
      .join('\n');
    const confirmed = window.confirm(
      `Revert to checkpoint? The following changes will be made:\n\n${fileList}`
    );

    if (!confirmed) return;

    try {
      await api.revertToCheckpoint(checkpoint.id);
    } catch (e) {
      console.error('Failed to revert:', e);
    }
  }

  function render() {
    updateModePill();
    updateModelBtn();
    renderApprovalArea();
    messagesArea.innerHTML = '';
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      messagesArea.appendChild(el('div', { class: 'chat-empty' }, 'Select a task to start chatting'));
      return;
    }

    const tasks = agentStore.getState('tasks');
    const task = tasks[taskId];
    if (!task) return;

    // Load checkpoints asynchronously
    loadCheckpoints(taskId).then(() => renderMessages(task));
  }

  function renderMessages(task) {
    // Capture scroll state before clearing so we can restore it
    const prevDistFromBottom =
      messagesArea.scrollHeight - messagesArea.scrollTop - messagesArea.clientHeight;
    const wasAtBottom = prevDistFromBottom <= 80;

    messagesArea.innerHTML = '';

    const taskId = agentStore.getState('activeTaskId');
    const isRunning = task.status === 'Running';
    const isFailed = task.status === 'Failed';

    // Pre-build tool_use_id → result block map from all tool messages
    const resultMap = buildResultMap(task.messages);

    let lastUserMsgIdx = -1;
    for (let i = task.messages.length - 1; i >= 0; i--) {
      if (task.messages[i].role === 'user') { lastUserMsgIdx = i; break; }
    }

    for (let i = 0; i < task.messages.length; i++) {
      const msg = task.messages[i];

      // Tool messages are rendered inline inside tool call cards — skip them here
      if (msg.role === 'tool') continue;

      // Task complete card
      if (msg.role === 'task_complete') {
        const block = msg.content[0];
        messagesArea.appendChild(renderCompletionCard(block.summary, block.notes, block.diff));
        continue;
      }

      // Model switch separator
      if (msg.content?.length === 1 && msg.content[0].type === 'model_switch') {
        messagesArea.appendChild(renderModelSwitchSeparator(msg.content[0].to_model));
        continue;
      }

      // ── User message ───────────────────────────────────────
      if (msg.role === 'user') {
        const msgEl = el('div', { class: 'chat-message chat-message--user' });
        for (const block of msg.content) {
          if (block.type === 'text' && block.text) {
            const textEl = el('div', { class: 'chat-message__text' });
            textEl.innerHTML = formatText(block.text);
            msgEl.appendChild(textEl);
          }
        }
        // Hover action bar — copy always, stop/retry when relevant
        const actions = el('div', { class: 'chat-message__actions chat-message__actions--user' });
        const copyBtn = el('button', { class: 'chat-message__action-btn', title: 'Copy' });
        copyBtn.appendChild(icon('M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z', 13));
        copyBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          navigator.clipboard.writeText(extractMessageText(msg)).catch(() => {});
          copyBtn.title = 'Copied!';
          setTimeout(() => { copyBtn.title = 'Copy'; }, 1500);
        });
        actions.appendChild(copyBtn);
        if (i === lastUserMsgIdx && isRunning) {
          const stopBtn = el('button', { class: 'chat-message__action-btn chat-message__stop-btn', title: 'Stop task' });
          stopBtn.appendChild(icon('M21 12a9 9 0 11-18 0 9 9 0 0118 0zM9 10a1 1 0 000 2h6a1 1 0 000-2H9z', 13));
          stopBtn.appendChild(el('span', {}, 'Stop'));
          stopBtn.addEventListener('click', async (e) => {
            e.stopPropagation(); stopBtn.disabled = true;
            try { await api.abortTask(taskId); } catch {}
          });
          actions.appendChild(stopBtn);
        }
        if (i === lastUserMsgIdx && isFailed) {
          const retryBtn = el('button', { class: 'chat-message__action-btn chat-message__retry-btn', title: 'Retry' });
          retryBtn.appendChild(icon('M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15', 13));
          retryBtn.appendChild(el('span', {}, 'Retry'));
          retryBtn.addEventListener('click', async (e) => {
            e.stopPropagation();
            const text = extractMessageText(msg);
            if (text && taskId) {
              const { sendMessage } = await import('../../state/agent.js');
              sendMessage(taskId, text);
            }
          });
          actions.appendChild(retryBtn);
        }
        msgEl.appendChild(actions);
        messagesArea.appendChild(msgEl);
        continue;
      }

      // ── Assistant message ──────────────────────────────────
      if (msg.role === 'assistant') {
        const isLastMsg = i === task.messages.length - 1;
        const isStreaming = task.isStreaming && isLastMsg;
        const allEmpty = msg.content.every(b => b.type === 'text' && !b.text);

        if (isStreaming && allEmpty) {
          // Animated thinking text — no tokens yet
          messagesArea.appendChild(renderThinkingIndicator());
          continue;
        }

        // Each assistant message may interleave text blocks and tool_use blocks.
        // Text blocks accumulate into a text bubble; tool_use blocks become
        // standalone expandable cards. We flush the text bubble before each card.
        let currentTextEl = null;

        const flushText = () => {
          if (currentTextEl) { messagesArea.appendChild(currentTextEl); currentTextEl = null; }
        };
        const ensureTextEl = () => {
          if (!currentTextEl) {
            currentTextEl = el('div', { class: 'chat-message chat-message--assistant' });
            currentTextEl.appendChild(el('div', { class: 'chat-message__role' }, 'Assistant'));
          }
          return currentTextEl;
        };

        // Is thinking currently the active streaming block?
        const lastBlock = msg.content[msg.content.length - 1];
        const isThinkingStreaming = isStreaming && lastBlock?.type === 'thinking';

        for (const block of msg.content) {
          if (block.type === 'thinking') {
            flushText();
            const isThisBlockStreaming = isThinkingStreaming && block === lastBlock;
            messagesArea.appendChild(renderThinkingBlock(block, isThisBlockStreaming));
          } else if (block.type === 'text' && block.text) {
            const wrapper = ensureTextEl();
            const textEl = el('div', {
              class: `chat-message__text${isStreaming && block === lastBlock ? ' chat-message__text--streaming' : ''}`,
            });
            textEl.innerHTML = formatText(block.text);
            wrapper.appendChild(textEl);
          } else if (block.type === 'tool_use') {
            flushText();
            messagesArea.appendChild(renderToolCallCard(block, resultMap.get(block.id)));
          }
        }

        // Checkpoint marker — attach to the pending text bubble or as standalone
        if (hasFileChanges(msg)) {
          const cp = findCheckpointForMessage(i);
          if (cp && cp.file_count > 0) {
            const cpEl = renderCheckpointMarker(cp, taskId);
            if (currentTextEl) { currentTextEl.appendChild(cpEl); }
            else { messagesArea.appendChild(cpEl); }
          }
        }

        flushText();
      }
    }

    // Auto-scroll: snap to bottom only if the user was already there,
    // otherwise preserve their scroll position relative to the bottom.
    if (wasAtBottom) {
      messagesArea.scrollTop = messagesArea.scrollHeight;
    } else {
      messagesArea.scrollTop =
        messagesArea.scrollHeight - messagesArea.clientHeight - prevDistFromBottom;
    }
  }

  function extractMessageText(msg) {
    return msg.content
      .filter((b) => b.type === 'text')
      .map((b) => b.text)
      .join('\n')
      .trim();
  }

  // Countdown timers: requestId -> intervalId
  const countdownTimers = {};

  function renderApprovalArea() {
    // Cancel any running timers for requests no longer in the list
    const taskId = agentStore.getState('activeTaskId');
    const allRequests = agentStore.getState('permissionRequests');
    const requests = taskId ? (allRequests[taskId] || []) : [];

    approvalArea.innerHTML = '';

    for (const req of requests) {
      let opIcon;
      let widgetClass = 'chat-approval-widget';
      if (req.operation === 'run_command') {
        opIcon = icon('M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z', 14);
      } else if (req.operation.startsWith('sensitive_file')) {
        opIcon = icon('M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z', 14);
        widgetClass = req.operation === 'sensitive_file_tier2'
          ? 'chat-approval-widget chat-approval-widget--sensitive'
          : 'chat-approval-widget chat-approval-widget--gitignored';
      } else {
        opIcon = icon('M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', 14);
      }

      const widget = el('div', { class: widgetClass });

      // Operation icon + description
      const info = el('div', { class: 'chat-approval-widget__info' });
      info.appendChild(opIcon);

      const descEl = el('div', { class: 'chat-approval-widget__desc' });
      if (req.operation.startsWith('sensitive_file')) {
        const tierLabel = req.operation === 'sensitive_file_tier2' ? 'Sensitive' : 'Gitignored';
        const badge = el('span', { class: `chat-approval-widget__tier-badge chat-approval-widget__tier-badge--${tierLabel.toLowerCase()}` }, tierLabel);
        descEl.insertBefore(badge, descEl.firstChild);
      }
      descEl.appendChild(el('span', { class: 'chat-approval-widget__label' }, req.description));
      if (req.preview) {
        descEl.appendChild(el('span', { class: 'chat-approval-widget__preview' }, req.preview));
      }
      info.appendChild(descEl);
      widget.appendChild(info);

      // Countdown + buttons
      const actions = el('div', { class: 'chat-approval-widget__actions' });

      const countdownEl = el('span', { class: 'chat-approval-widget__countdown' }, '60s');

      const denyBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--deny' }, 'Deny');
      const allowBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--allow' }, 'Allow');

      let remaining = 60;

      function startCountdown() {
        if (countdownTimers[req.request_id]) return;
        countdownTimers[req.request_id] = setInterval(() => {
          remaining--;
          countdownEl.textContent = `${remaining}s`;
          if (remaining <= 0) {
            clearInterval(countdownTimers[req.request_id]);
            delete countdownTimers[req.request_id];
            respondToPermission(taskId, req.request_id, false);
          }
        }, 1000);
      }

      startCountdown();

      denyBtn.addEventListener('click', () => {
        clearInterval(countdownTimers[req.request_id]);
        delete countdownTimers[req.request_id];
        respondToPermission(taskId, req.request_id, false);
      });

      allowBtn.addEventListener('click', () => {
        clearInterval(countdownTimers[req.request_id]);
        delete countdownTimers[req.request_id];
        respondToPermission(taskId, req.request_id, true);
      });

      actions.appendChild(countdownEl);
      actions.appendChild(denyBtn);
      actions.appendChild(allowBtn);
      widget.appendChild(actions);
      approvalArea.appendChild(widget);
    }
  }

  agentStore.subscribe('tasks', () => { render(); updateCostDisplay(); updateHeaderBar(); renderBudgetBanner(); });
  agentStore.subscribe('activeTaskId', () => { render(); updateCostDisplay(); updateHeaderBar(); renderBudgetBanner(); renderSubagentsPanel(); });
  agentStore.subscribe('permissionRequests', renderApprovalArea);
  agentStore.subscribe('turnBudgetWarnings', renderBudgetBanner);
  agentStore.subscribe('subagents', renderSubagentsPanel);
  render();
  updateCostDisplay();
  updateHeaderBar();
  renderBudgetBanner();
  renderSubagentsPanel();

  return container;
}

// ── Thinking indicator ────────────────────────────────────────────────────────

const THINKING_WORDS = [
  'Thinking', 'Working', 'Analyzing', 'Reasoning', 'Processing',
  'Exploring', 'Reviewing', 'Searching', 'Planning', 'Considering',
];

function renderThinkingIndicator() {
  const wrapper = el('div', { class: 'chat-thinking-indicator' });

  const wordEl = el('span', { class: 'chat-thinking-indicator__word' });
  wordEl.textContent = THINKING_WORDS[Math.floor(Math.random() * THINKING_WORDS.length)];

  const dotsEl = el('span', { class: 'chat-thinking-indicator__dots' }, '...');
  wrapper.appendChild(wordEl);
  wrapper.appendChild(dotsEl);

  // Cycle through words every 2.5 s
  let idx = THINKING_WORDS.indexOf(wordEl.textContent);
  const timer = setInterval(() => {
    idx = (idx + 1) % THINKING_WORDS.length;
    wordEl.classList.add('chat-thinking-indicator__word--fade');
    setTimeout(() => {
      wordEl.textContent = THINKING_WORDS[idx];
      wordEl.classList.remove('chat-thinking-indicator__word--fade');
    }, 250);
  }, 2500);

  // Clean up timer when element is removed from DOM
  const observer = new MutationObserver(() => {
    if (!document.body.contains(wrapper)) { clearInterval(timer); observer.disconnect(); }
  });
  observer.observe(document.body, { childList: true, subtree: true });

  return wrapper;
}

// ── Tool call card (unified tool_use + tool_result) ──────────────────────────

/**
 * Build a map of tool_use_id → tool_result block from all tool messages.
 */
function buildResultMap(messages) {
  const map = new Map();
  for (const msg of messages) {
    if (msg.role === 'tool') {
      for (const block of (msg.content || [])) {
        if (block.type === 'tool_result' && block.tool_use_id) {
          map.set(block.tool_use_id, block);
        }
      }
    }
  }
  return map;
}

const TOOL_META = {
  read_file:      { label: 'Read file',      iconPath: 'M15 12a3 3 0 11-6 0 3 3 0 016 0zM2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z', color: 'blue' },
  list_directory: { label: 'List directory', iconPath: 'M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z', color: 'blue' },
  grep_search:    { label: 'Search',         iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'blue' },
  run_command:    { label: 'Run command',    iconPath: 'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z', color: 'orange' },
  edit_file:      { label: 'Edit file',      iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  apply_patch:    { label: 'Edit file',      iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  write_file:     { label: 'Write file',     iconPath: 'M12 5v14M5 12h14', color: 'green' },
  create_file:    { label: 'Create file',    iconPath: 'M12 5v14M5 12h14', color: 'green' },
};
const TOOL_META_DEFAULT = { label: null, iconPath: 'M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z', color: 'gray' };

function getToolSummary(name, input) {
  const path = input.path || input.file_path || input.directory || '';
  switch (name) {
    case 'read_file': {
      let s = path;
      if (input.start_line && input.end_line) s += `:${input.start_line}-${input.end_line}`;
      else if (input.start_line) s += `:${input.start_line}+`;
      return s;
    }
    case 'list_directory': return path;
    case 'grep_search': {
      const pat = input.pattern || input.query || '';
      return pat ? `"${pat}"${path ? '  ' + path : ''}` : path;
    }
    case 'run_command': {
      const cmd = input.command || input.cmd || '';
      return cmd.length > 72 ? cmd.slice(0, 69) + '…' : cmd;
    }
    case 'edit_file': case 'apply_patch': case 'write_file': case 'create_file':
      return path;
    default: return '';
  }
}

function formatToolInput(name, input) {
  const entries = Object.entries(input);
  if (entries.length === 0) return '(no input)';

  // For file ops: show metadata fields first, then large content fields separately
  const bulkKeys = ['content', 'new_content', 'diff', 'patch'];
  const meta = entries.filter(([k]) => !bulkKeys.includes(k));
  const bulk = entries.filter(([k]) => bulkKeys.includes(k));

  if (meta.length === 0 && bulk.length === 0) return JSON.stringify(input, null, 2);

  let out = '';
  for (const [k, v] of meta) {
    out += `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}\n`;
  }
  for (const [k, v] of bulk) {
    const str = typeof v === 'string' ? v : JSON.stringify(v, null, 2);
    const lines = str.split('\n');
    const preview = lines.length > 30 ? lines.slice(0, 30).join('\n') + '\n…' : str;
    out += `\n${k}:\n${preview}\n`;
  }
  return out.trim();
}

/**
 * Render a collapsible thinking block.
 * While streaming (isStreaming=true) shows an animated "Thinking…" header.
 * Once done, collapses by default; click header to expand/collapse.
 */
function renderThinkingBlock(block, isStreaming) {
  const card = el('div', { class: `thinking-block${isStreaming ? ' thinking-block--streaming' : ''}` });

  const header = el('button', { class: 'thinking-block__header' });

  // Brain icon
  const brainIcon = el('span', { class: 'thinking-block__icon' });
  brainIcon.appendChild(icon('M9.5 2a6.5 6.5 0 0 1 6.48 7.13A4.5 4.5 0 0 1 17 18H7a5 5 0 0 1-2.1-9.52A6.5 6.5 0 0 1 9.5 2z', 13));
  header.appendChild(brainIcon);

  if (isStreaming) {
    const shimmer = el('span', { class: 'thinking-block__label thinking-block__label--shimmer' }, 'Thinking');
    const dots = el('span', { class: 'thinking-block__dots' }, '...');
    header.appendChild(shimmer);
    header.appendChild(dots);
  } else {
    header.appendChild(el('span', { class: 'thinking-block__label' }, 'Thinking'));
    const chevron = el('span', { class: 'thinking-block__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    header.appendChild(chevron);

    // Token count hint
    if (block.thinking) {
      const words = block.thinking.split(/\s+/).length;
      header.appendChild(el('span', { class: 'thinking-block__meta' }, `~${words} words`));
    }

    // Expandable body
    const body = el('div', { class: 'thinking-block__body thinking-block__body--hidden' });
    const pre = el('pre', { class: 'thinking-block__content' });
    pre.textContent = block.thinking || '';
    body.appendChild(pre);
    card.appendChild(body);

    let open = false;
    header.addEventListener('click', () => {
      open = !open;
      body.classList.toggle('thinking-block__body--hidden', !open);
      const ch = header.querySelector('.thinking-block__chevron');
      if (ch) ch.style.transform = open ? 'rotate(180deg)' : '';
    });
  }

  card.appendChild(header);
  return card;
}

/**
 * Render an expandable tool call card combining tool_use + its tool_result.
 * @param {object} block  - The tool_use content block
 * @param {object|undefined} result - The matching tool_result block (undefined if still pending)
 */
function renderToolCallCard(block, result) {
  const { name, input = {}, id } = block;
  const meta = TOOL_META[name] || { ...TOOL_META_DEFAULT, label: name };
  const label = meta.label || name;
  const summary = getToolSummary(name, input);
  const isPending = !result;
  const isError = result?.is_error;

  const card = el('div', { class: `tool-call${isError ? ' tool-call--error' : ''}` });

  // ── Header (always visible, click to toggle) ──────────────
  const header = el('button', { class: 'tool-call__header' });

  // Colored icon badge
  const iconWrap = el('span', { class: `tool-call__icon tool-call__icon--${meta.color}` });
  iconWrap.appendChild(icon(meta.iconPath, 13));
  header.appendChild(iconWrap);

  // Tool label
  header.appendChild(el('span', { class: 'tool-call__name' }, label));

  // One-line summary (path / command / pattern)
  if (summary) {
    header.appendChild(el('span', { class: 'tool-call__summary' }, summary));
  }

  // Status: spinner | ✓ | ✗
  const statusEl = el('span', { class: 'tool-call__status' });
  if (isPending) {
    statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    const checkPath = isError
      ? 'M18 6L6 18M6 6l12 12'
      : 'M5 13l4 4L19 7';
    statusEl.appendChild(icon(checkPath, 12));
    statusEl.classList.add(isError ? 'tool-call__status--error' : 'tool-call__status--ok');
  }
  header.appendChild(statusEl);

  // Chevron
  const chevron = el('span', { class: 'tool-call__chevron' });
  chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
  header.appendChild(chevron);

  card.appendChild(header);

  // ── Expandable body ───────────────────────────────────────
  const body = el('div', { class: 'tool-call__body tool-call__body--hidden' });

  // Input section
  const inputSection = el('div', { class: 'tool-call__section' });
  inputSection.appendChild(el('div', { class: 'tool-call__section-label' }, 'Input'));
  const inputPre = el('pre', { class: 'tool-call__code' });
  inputPre.textContent = formatToolInput(name, input);
  inputSection.appendChild(inputPre);
  body.appendChild(inputSection);

  // Output section
  if (result && result.content != null) {
    const content = String(result.content);
    const lines = content.split('\n');
    const PREVIEW = 15;

    const outputSection = el('div', {
      class: `tool-call__section tool-call__section--output${isError ? ' tool-call__section--error' : ''}`,
    });
    outputSection.appendChild(el('div', { class: 'tool-call__section-label' }, isError ? 'Error' : 'Output'));

    const outputPre = el('pre', { class: 'tool-call__code' });
    if (lines.length > PREVIEW) {
      outputPre.textContent = lines.slice(0, PREVIEW).join('\n') + '\n…';
      let expanded = false;
      const showMore = el('button', { class: 'tool-call__show-more' }, `Show all (${lines.length} lines)`);
      showMore.addEventListener('click', (e) => {
        e.stopPropagation();
        expanded = !expanded;
        outputPre.textContent = expanded ? content : lines.slice(0, PREVIEW).join('\n') + '\n…';
        showMore.textContent = expanded ? 'Show less' : `Show all (${lines.length} lines)`;
      });
      outputSection.appendChild(outputPre);
      outputSection.appendChild(showMore);
    } else {
      outputPre.textContent = content;
      outputSection.appendChild(outputPre);
    }
    body.appendChild(outputSection);
  }

  card.appendChild(body);

  // Toggle expand / collapse
  let open = false;
  header.addEventListener('click', () => {
    open = !open;
    body.classList.toggle('tool-call__body--hidden', !open);
    chevron.style.transform = open ? 'rotate(180deg)' : '';
  });

  return card;
}

// ─────────────────────────────────────────────────────────────────────────────

function renderModelSwitchSeparator(toModel) {
  const sep = el('div', { class: 'chat-model-switch' });
  sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
  sep.appendChild(el('span', { class: 'chat-model-switch__label' }, `Model: ${abbreviateModel(toModel)}`));
  sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
  return sep;
}

function renderCheckpointMarker(cp, taskId) {
  const marker = el('div', { class: 'chat-checkpoint' });

  const info = el('div', { class: 'chat-checkpoint__info' });
  info.appendChild(icon('M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z', 14));
  info.appendChild(el('span', {}, `Checkpoint — ${cp.file_count} file${cp.file_count !== 1 ? 's' : ''} changed`));
  marker.appendChild(info);

  const actions = el('div', { class: 'chat-checkpoint__actions' });

  // Diff button — lazy loads and shows inline diff card
  const diffBtn = el('button', { class: 'chat-checkpoint__diff-btn' }, 'View diff');
  let diffCard = null;
  let diffLoaded = false;

  diffBtn.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (diffCard) {
      diffCard.remove();
      diffCard = null;
      diffBtn.textContent = 'View diff';
      return;
    }
    if (!diffLoaded) {
      diffBtn.textContent = 'Loading…';
      diffBtn.disabled = true;
      try {
        const diff = await api.getCheckpointDiff(taskId, cp.id);
        diffLoaded = true;
        diffBtn.textContent = 'Hide diff';
        diffBtn.disabled = false;
        if (diff && diff.files && diff.files.length > 0) {
          diffCard = renderCompletionCard(null, null, diff);
          diffCard.classList.add('chat-checkpoint__diff-view');
          marker.insertAdjacentElement('afterend', diffCard);
        } else {
          diffBtn.textContent = 'No changes';
        }
      } catch {
        diffBtn.textContent = 'View diff';
        diffBtn.disabled = false;
      }
    } else {
      diffBtn.textContent = 'Hide diff';
      diffCard = renderCompletionCard(null, null, null);
    }
  });
  actions.appendChild(diffBtn);

  // Revert button
  const revertBtn = el('button', { class: 'chat-checkpoint__revert' }, 'Revert');
  revertBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    handleRevert(cp);
  });
  actions.appendChild(revertBtn);

  marker.appendChild(actions);
  return marker;
}

function renderCompletionCard(summary, notes, diff) {
  const card = el('div', { class: 'task-complete-card' });

  // Header — only shown when used as a task completion card (summary present)
  if (summary) {
    const header = el('div', { class: 'task-complete-card__header' });
    header.appendChild(icon('M5 13l4 4L19 7', 16));
    header.appendChild(el('span', {}, 'Task complete'));
    card.appendChild(header);

    const summaryEl = el('div', { class: 'task-complete-card__summary' }, summary);
    card.appendChild(summaryEl);
  }

  // Notes (optional)
  if (notes) {
    const notesEl = el('div', { class: 'task-complete-card__notes' }, `Notes: ${notes}`);
    card.appendChild(notesEl);
  }

  // File changes section
  if (diff && diff.files && diff.files.length > 0) {
    const filesSection = el('div', { class: 'task-complete-card__files' });

    // Collapsible toggle
    const toggle = el('div', { class: 'task-complete-card__files-toggle' });
    const arrowIcon = icon('M19 9l-7 7-7-7', 14);
    toggle.appendChild(arrowIcon);
    toggle.appendChild(
      el('span', {}, `${diff.files.length} file${diff.files.length !== 1 ? 's' : ''} changed`)
    );
    const stats = el('span', { class: 'task-complete-card__stats' });
    if (diff.total_insertions > 0) stats.appendChild(el('span', { class: 'task-complete-card__insertions' }, `+${diff.total_insertions}`));
    if (diff.total_deletions > 0) stats.appendChild(el('span', { class: 'task-complete-card__deletions' }, `-${diff.total_deletions}`));
    toggle.appendChild(stats);
    filesSection.appendChild(toggle);

    // File list (collapsed by default)
    const fileList = el('div', { class: 'task-complete-card__file-list task-complete-card__file-list--collapsed' });

    for (const file of diff.files) {
      const fileRow = el('div', { class: 'task-complete-card__file-row' });

      // Status icon
      const statusClass =
        file.status === 'Created' ? 'task-complete-card__file-status--created' :
        file.status === 'Deleted' ? 'task-complete-card__file-status--deleted' :
        'task-complete-card__file-status--modified';
      const statusIcon = el('span', { class: `task-complete-card__file-status ${statusClass}` },
        file.status === 'Created' ? '+' : file.status === 'Deleted' ? '−' : '~'
      );
      fileRow.appendChild(statusIcon);

      // Path
      fileRow.appendChild(el('span', { class: 'task-complete-card__file-path' }, file.path));

      // Counts
      const counts = el('span', { class: 'task-complete-card__file-counts' });
      if (file.insertions > 0) counts.appendChild(el('span', { class: 'task-complete-card__insertions' }, `+${file.insertions}`));
      if (file.deletions > 0) counts.appendChild(el('span', { class: 'task-complete-card__deletions' }, `-${file.deletions}`));
      fileRow.appendChild(counts);

      // Mini bar chart
      const maxChanges = Math.max(...diff.files.map((f) => f.insertions + f.deletions), 1);
      const ratio = (file.insertions + file.deletions) / maxChanges;
      const bar = el('div', { class: 'task-complete-card__bar' });
      const fill = el('div', {
        class: 'task-complete-card__bar-fill',
        style: `width: ${Math.round(ratio * 100)}%`,
      });
      bar.appendChild(fill);
      fileRow.appendChild(bar);

      // Diff expand button (shown inline on click)
      if (file.unified_diff) {
        const diffBtn = el('button', { class: 'task-complete-card__diff-btn' }, 'diff');
        let diffExpanded = false;
        let diffEl = null;

        diffBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          diffExpanded = !diffExpanded;
          if (diffExpanded) {
            diffEl = el('pre', { class: 'task-complete-card__diff-content' }, file.unified_diff);
            fileRow.insertAdjacentElement('afterend', diffEl);
            diffBtn.textContent = 'hide';
          } else {
            if (diffEl) { diffEl.remove(); diffEl = null; }
            diffBtn.textContent = 'diff';
          }
        });
        fileRow.appendChild(diffBtn);
      }

      fileList.appendChild(fileRow);
    }

    filesSection.appendChild(fileList);

    // Toggle expand/collapse on click
    let expanded = false;
    toggle.style.cursor = 'pointer';
    toggle.addEventListener('click', () => {
      expanded = !expanded;
      fileList.classList.toggle('task-complete-card__file-list--collapsed', !expanded);
      arrowIcon.style.transform = expanded ? 'rotate(180deg)' : '';
    });

    card.appendChild(filesSection);
  }

  return card;
}

function formatText(text) {
  return marked.parse(text, { breaks: true, gfm: true });
}
