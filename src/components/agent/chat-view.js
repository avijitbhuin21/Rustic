import { el, icon, iconMulti } from '../../utils/dom.js';
import { agentStore, sendMessage, setTaskPermissions, respondToPermission } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { openDiffView } from '../../state/editor.js';
import * as api from '../../lib/tauri-api.js';
import { loadProviderConfigs } from '../settings/ai-settings.js';
import { marked } from 'marked';
import { processMessages } from '../../utils/message-pipeline.js';

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

// Persistent expand/collapse state — survives DOM rebuilds.
// Keys: "thinking-{msgIdx}", "tool-{tool_use_id}", "group-{firstToolUseId}"
const expandedState = new Map();

// Returns thinking capability info for the given model, or null if not supported.
function getThinkingCapability(model) {
  if (!model) return null;
  if (model.includes('claude-opus-4')) return { type: 'effort', levels: ['low', 'medium', 'high', 'max'] };
  if (model.includes('claude-sonnet-4') || model.includes('claude-haiku-4')) return { type: 'effort', levels: ['low', 'medium', 'high'] };
  if (/^o\d/.test(model)) return { type: 'effort', levels: ['low', 'medium', 'high'] };
  if (model.includes('gemini-2.5-pro')) return { type: 'budget', min: 128, max: 32768 };
  if (model.includes('gemini-2.5-flash-lite')) return { type: 'budget', min: 512, max: 24576 };
  if (model.includes('gemini-2.5-flash')) return { type: 'budget', min: 0, max: 24576 };
  if (model.includes('gemini-3')) return { type: 'effort', levels: ['LOW', 'HIGH'] };
  return null;
}

function formatTokens(n) {
  if (!n) return '0';
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

export function createChatView() {
  const container = el('div', { class: 'chat-view' });

  // Chat header bar — collapsed: [title ... progress+cost], expanded: stats+task
  const headerBar = el('div', { class: 'chat-header-bar' });
  let headerExpanded = false;

  // Collapsed row elements
  const headerCollapsedRow = el('div', { class: 'chat-header-bar__row chat-header-bar__row--collapsed' });
  const headerTitle = el('div', { class: 'chat-header-bar__title' });
  const headerRight = el('div', { class: 'chat-header-bar__right' });

  // Price box with border-as-progress (conic-gradient approach)
  const progressWrapper = el('div', { class: 'chat-header-progress', title: 'Context window used' });
  const progressInner = el('div', { class: 'chat-header-progress__inner' });
  const progressCostLabel = el('span', { class: 'chat-header-progress__label' });
  progressInner.appendChild(progressCostLabel);
  progressWrapper.appendChild(progressInner);
  headerRight.appendChild(progressWrapper);
  headerCollapsedRow.appendChild(headerTitle);
  headerCollapsedRow.appendChild(headerRight);

  // Expanded area elements
  const headerExpandedArea = el('div', { class: 'chat-header-bar__expanded chat-header-bar__expanded--hidden' });
  const headerStatsRow = el('div', { class: 'chat-header-bar__stats-row' });
  const headerFullTaskWrapper = el('div', { class: 'chat-header-bar__full-task-wrapper' });
  const headerFullTask = el('div', { class: 'chat-header-bar__full-task' });
  const headerCopyBtn = el('button', { class: 'chat-header-bar__copy-btn', title: 'Copy prompt' });
  headerCopyBtn.appendChild(icon('M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z', 13));
  headerCopyBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    navigator.clipboard.writeText(headerFullTask.textContent).catch(() => {});
    headerCopyBtn.title = 'Copied!';
    setTimeout(() => { headerCopyBtn.title = 'Copy prompt'; }, 1500);
  });
  headerFullTaskWrapper.appendChild(headerCopyBtn);
  headerFullTaskWrapper.appendChild(headerFullTask);
  headerExpandedArea.appendChild(headerStatsRow);
  headerExpandedArea.appendChild(headerFullTaskWrapper);

  headerBar.appendChild(headerCollapsedRow);
  headerBar.appendChild(headerExpandedArea);

  // Toggle expanded/collapsed on click
  function toggleHeader() {
    headerExpanded = !headerExpanded;
    headerExpandedArea.classList.toggle('chat-header-bar__expanded--hidden', !headerExpanded);
    headerCollapsedRow.classList.toggle('chat-header-bar__row--hidden', headerExpanded);
    headerBar.classList.toggle('chat-header-bar--expanded', headerExpanded);
    updateHeaderBar();
  }
  headerCollapsedRow.style.cursor = 'pointer';
  headerCollapsedRow.addEventListener('click', toggleHeader);
  headerExpandedArea.style.cursor = 'pointer';
  headerExpandedArea.addEventListener('click', toggleHeader);

  /** Sum up all subagent costs for a given task. */
  function getSubagentCostTotals(taskId) {
    const subagents = agentStore.getState('subagents')[taskId] || {};
    let inputTokens = 0, outputTokens = 0, cacheTokens = 0, usd = 0;
    for (const agent of Object.values(subagents)) {
      if (agent.cost) {
        inputTokens += agent.cost.total_input_tokens || 0;
        outputTokens += agent.cost.total_output_tokens || 0;
        cacheTokens += agent.cost.total_cache_read_tokens || 0;
        usd += agent.cost.estimated_cost_usd || 0;
      }
    }
    return { inputTokens, outputTokens, cacheTokens, usd };
  }

  function updateCostDisplay() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) { progressCostLabel.textContent = ''; headerStatsRow.innerHTML = ''; return; }
    const task = agentStore.getState('tasks')[taskId];
    const cost = task?.cost;
    if (!cost) { progressCostLabel.textContent = ''; headerStatsRow.innerHTML = ''; return; }

    // Aggregate subagent costs into the totals
    const sub = getSubagentCostTotals(taskId);
    const input = (cost.total_input_tokens || 0) + sub.inputTokens;
    const output = (cost.total_output_tokens || 0) + sub.outputTokens;
    const usd = (cost.estimated_cost_usd || 0) + sub.usd;
    const cacheRead = (cost.total_cache_read_tokens || 0) + sub.cacheTokens;

    const costStr = usd > 0
      ? usd < 0.001 ? '<$0.001' : `$${usd.toFixed(3)}`
      : '$0';

    // Progress bar label = cost
    progressCostLabel.textContent = costStr;

    // Hover tooltip on progress bar
    progressWrapper.title = [
      `↑ Sent: ${input.toLocaleString()}`,
      `↓ Received: ${output.toLocaleString()}`,
      cacheRead > 0 ? `Cache read: ${cacheRead.toLocaleString()}` : null,
      `Turns: ${cost.turn_count ?? 0}`,
      sub.usd > 0 ? `Sub-agent cost: $${sub.usd.toFixed(4)}` : null,
      `Est. cost: $${usd.toFixed(4)}`,
    ].filter(Boolean).join('\n');

    // Expanded stats row
    headerStatsRow.innerHTML = '';
    const statsItems = [
      { icon: '↑', value: formatTokens(input), cls: 'sent' },
      { icon: '↓', value: formatTokens(output), cls: 'recv' },
      { icon: '$', value: usd > 0 ? (usd < 0.001 ? '<0.001' : usd.toFixed(3)) : '0', cls: 'cost' },
    ];
    for (const s of statsItems) {
      const stat = el('span', { class: `chat-header-stat chat-header-stat--${s.cls}` });
      stat.appendChild(el('span', { class: 'chat-header-stat__icon' }, s.icon));
      stat.appendChild(el('span', { class: 'chat-header-stat__value' }, s.value));
      headerStatsRow.appendChild(stat);
    }
  }

  function updateHeaderBar() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) { headerTitle.textContent = ''; headerFullTask.textContent = ''; return; }
    const task = agentStore.getState('tasks')[taskId];
    headerTitle.textContent = task?.title || '';

    // Full task text for expanded view — skip injected [Project Memory] messages
    let questionText = '';
    for (const msg of (task?.messages || [])) {
      if (msg.role === 'user') {
        for (const block of (msg.content || [])) {
          if (block.type === 'text' && block.text && !block.text.startsWith('[Project Memory]')) {
            questionText = block.text;
            break;
          }
        }
        if (questionText) break;
      }
    }
    headerFullTask.textContent = questionText;
  }

  // Sticky card (todo list only) — sits between header and messages
  const stickyCard = el('div', { class: 'chat-sticky-card chat-sticky-card--hidden' });
  let stickyTodosCollapsed = true;

  function renderStickyCard() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) { stickyCard.classList.add('chat-sticky-card--hidden'); stickyCard.innerHTML = ''; return; }

    const task = agentStore.getState('tasks')[taskId];
    const todos = agentStore.getState('todos')[taskId] || [];
    if (!task) { stickyCard.classList.add('chat-sticky-card--hidden'); stickyCard.innerHTML = ''; return; }

    // Nothing to show if no todos
    if (todos.length === 0) {
      stickyCard.classList.add('chat-sticky-card--hidden');
      stickyCard.innerHTML = '';
      return;
    }

    stickyCard.innerHTML = '';
    stickyCard.classList.remove('chat-sticky-card--hidden');

    // ── Todo list section ──
    if (todos.length > 0) {
      const tSection = el('div', { class: 'sticky-card__section' });
      const tHeader = el('button', { class: 'sticky-card__header' });
      const completedCount = todos.filter(t => t.status === 'completed').length;
      tHeader.appendChild(icon('M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', 13));
      tHeader.appendChild(el('span', { class: 'sticky-card__title' }, 'Todo'));
      tHeader.appendChild(el('span', { class: 'sticky-card__counter' }, `${completedCount}/${todos.length}`));
      const tChevron = el('span', { class: 'sticky-card__chevron' });
      tChevron.appendChild(icon('M19 9l-7 7-7-7', 10));
      if (stickyTodosCollapsed) tChevron.style.transform = 'rotate(-90deg)';
      tHeader.appendChild(tChevron);
      tSection.appendChild(tHeader);

      const tBody = el('div', { class: `sticky-card__body${stickyTodosCollapsed ? ' sticky-card__body--hidden' : ''}` });

      // Sort: in_progress first, then completed, then pending
      const sorted = [...todos].sort((a, b) => {
        const order = { in_progress: 0, completed: 1, pending: 2 };
        return (order[a.status] ?? 3) - (order[b.status] ?? 3);
      });

      for (const item of sorted) {
        const row = el('div', { class: `sticky-card__todo sticky-card__todo--${item.status}` });
        const checkbox = el('span', { class: 'sticky-card__todo-check' });
        if (item.status === 'completed') {
          checkbox.appendChild(icon('M5 13l4 4L19 7', 11));
        } else if (item.status === 'in_progress') {
          checkbox.appendChild(el('span', { class: 'sticky-card__todo-spinner' }));
        } else {
          checkbox.appendChild(el('span', { class: 'sticky-card__todo-empty' }));
        }
        row.appendChild(checkbox);

        const label = el('span', { class: 'sticky-card__todo-label' }, item.content);
        row.appendChild(label);

        if (item.status === 'in_progress') {
          row.appendChild(el('span', { class: 'sticky-card__todo-badge sticky-card__todo-badge--active' }, 'Active'));
        }

        tBody.appendChild(row);
      }

      tSection.appendChild(tBody);

      tHeader.addEventListener('click', () => {
        stickyTodosCollapsed = !stickyTodosCollapsed;
        tBody.classList.toggle('sticky-card__body--hidden', stickyTodosCollapsed);
        tChevron.style.transform = stickyTodosCollapsed ? 'rotate(-90deg)' : '';
      });

      stickyCard.appendChild(tSection);
    }
  }

  // Messages area
  const messagesArea = el('div', { class: 'chat-messages' });

  // Approval requests area (shown between messages and input)
  const approvalArea = el('div', { class: 'chat-approval-area' });

  // Sub-agents panel (shown when active sub-agents exist)

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

  // Changed-files panel (above input, expands upward)
  const changedFilesPanel = el('div', { class: 'chat-changed-files' });

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
    const label = el('span', {}, model || 'Model');
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

    closeThinkPopover();
    closeModeDropdown();

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
              saveThinkingForModel(currentModel);
              await api.switchModel(taskId, providerId, modelId);
              restoreThinkingForModel(modelId);
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

    const rect = modelBtn.getBoundingClientRect();
    modelDropdown.style.cssText = `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;left:${rect.left}px;`;
    document.body.appendChild(modelDropdown);
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

    closeModelDropdown();
    closeThinkPopover();

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

    const rect = modePill.getBoundingClientRect();
    modeDropdown.style.cssText = `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;right:${window.innerWidth - rect.right}px;`;
    document.body.appendChild(modeDropdown);
  });

  document.addEventListener('click', closeModeDropdown);

  const sendBtn = el('button', { class: 'chat-send-btn', title: 'Send' });
  sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 15));

  let sendBtnIsStop = false;

  function updateSendBtn() {
    const taskId = agentStore.getState('activeTaskId');
    const task = taskId ? agentStore.getState('tasks')[taskId] : null;
    const isRunning = task?.status === 'Running';
    if (isRunning === sendBtnIsStop) return;
    sendBtnIsStop = isRunning;
    sendBtn.innerHTML = '';
    if (isRunning) {
      sendBtn.classList.add('chat-send-btn--stop');
      sendBtn.title = 'Stop task';
      sendBtn.appendChild(icon('M6 6h12v12H6z', 14));
    } else {
      sendBtn.classList.remove('chat-send-btn--stop');
      sendBtn.title = 'Send';
      sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 15));
    }
  }

  // Slash commands toolbar button — opens the picker with all items
  const slashBtn = el('button', { class: 'chat-slash-btn', title: 'Slash commands' }, '/');
  slashBtn.addEventListener('click', async () => {
    textarea.focus();
    if (!slashPickerLoaded) await loadSlashItems();
    openSlashPicker('');
  });

  function getContextWindow(model) {
    if (!model) return 200000;
    if (model.includes('gemini-2.5-pro') || model.includes('gemini-3')) return 1048576;
    if (model.includes('gemini-2.5')) return 1048576;
    if (model.includes('claude')) return 200000;
    if (model.includes('gpt-4o') || model.includes('gpt-4')) return 128000;
    if (/^o\d/.test(model)) return 200000;
    return 128000;
  }

  function updateContextBadge() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      progressWrapper.style.setProperty('--progress', '0');
      progressWrapper.classList.remove('chat-header-progress--warn', 'chat-header-progress--high');
      return;
    }
    const task = agentStore.getState('tasks')[taskId];
    const cost = task?.cost;
    if (!cost || !cost.total_input_tokens) {
      progressWrapper.style.setProperty('--progress', '0');
      progressWrapper.classList.remove('chat-header-progress--warn', 'chat-header-progress--high');
      return;
    }
    const used = (cost.total_input_tokens || 0) + (cost.total_output_tokens || 0);
    const max = getContextWindow(getCurrentModel());
    const pct = Math.min(100, (used / max) * 100);
    progressWrapper.style.setProperty('--progress', `${pct}`);

    // Update expanded stats: context percentage
    const contextStat = headerStatsRow.querySelector('.chat-header-stat--context');
    if (!contextStat && headerStatsRow.children.length > 0) {
      const stat = el('span', { class: 'chat-header-stat chat-header-stat--context' });
      stat.appendChild(el('span', { class: 'chat-header-stat__icon' }, '%'));
      stat.appendChild(el('span', { class: 'chat-header-stat__value' }, `${Math.round(pct)}%`));
      headerStatsRow.appendChild(stat);
    } else if (contextStat) {
      const val = contextStat.querySelector('.chat-header-stat__value');
      if (val) val.textContent = `${Math.round(pct)}%`;
    }

    progressWrapper.classList.toggle('chat-header-progress--warn', pct > 50 && pct <= 80);
    progressWrapper.classList.toggle('chat-header-progress--high', pct > 80);
  }

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

  // Attach file button (upload icon)
  const attachBtn = el('button', { class: 'chat-attach-btn', title: 'Attach file' });
  attachBtn.appendChild(icon('M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12', 14));

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

  // Call Configuration button — brain icon, opens popover with thinking effort + chat mode
  const callConfigBtn = el('button', { class: 'chat-think-btn', title: 'Call configuration' });
  callConfigBtn.appendChild(iconMulti([
    'M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.46 2.5 2.5 0 0 1-1.04-1.54A2.5 2.5 0 0 1 4 15.5a2.5 2.5 0 0 1 0-7 2.5 2.5 0 0 1 1-2A2.5 2.5 0 0 1 9.5 2Z',
    'M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.46 2.5 2.5 0 0 0 1.04-1.54A2.5 2.5 0 0 0 20 15.5a2.5 2.5 0 0 0 0-7 2.5 2.5 0 0 0-1-2A2.5 2.5 0 0 0 14.5 2Z',
  ], 14));

  let callConfigOpen = false;
  let callConfigPopover = null;

  function closeCallConfig() {
    if (callConfigPopover) { callConfigPopover.remove(); callConfigPopover = null; callConfigOpen = false; }
  }

  function rebuildCallConfigContent() {
    if (!callConfigPopover) return;
    callConfigPopover.innerHTML = '';

    const taskId = agentStore.getState('activeTaskId');

    // ── Permission modes ─────────────────────────────────
    const current = getCurrentMode();
    const MODE_ICONS = {
      Chat:       'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z',
      ManualEdit: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z',
      AutoEdit:   'M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4',
      FullAuto:   'M13 10V3L4 14h7v7l9-11h-7z',
    };

    for (const mode of MODES) {
      const isActive = mode.value === current;
      const item = el('div', {
        class: `chat-call-config-item${isActive ? ' chat-call-config-item--active' : ''}`,
      });
      const iconEl = el('span', { class: 'chat-call-config-item__icon' });
      iconEl.appendChild(icon(MODE_ICONS[mode.value] || MODE_ICONS.Chat, 16));
      item.appendChild(iconEl);

      const textCol = el('div', { class: 'chat-call-config-item__text' });
      textCol.appendChild(el('div', { class: 'chat-call-config-item__title' }, mode.label));
      textCol.appendChild(el('div', { class: 'chat-call-config-item__desc' }, mode.desc));
      item.appendChild(textCol);

      if (isActive) {
        const check = el('span', { class: 'chat-call-config-item__check' });
        check.appendChild(icon('M5 13l4 4L19 7', 14));
        item.appendChild(check);
      }

      item.addEventListener('click', async (ev) => {
        ev.stopPropagation();
        const ok = await setTaskPermissions(taskId, mode.value);
        if (ok) {
          updateCallConfigBtn();
          rebuildCallConfigContent(); // re-render in place, don't close
        }
      });
      callConfigPopover.appendChild(item);
    }

    // ── Divider ──────────────────────────────────────────
    callConfigPopover.appendChild(el('div', { class: 'chat-call-config-divider' }));

    // ── Thinking effort ──────────────────────────────────
    const cap = getThinkingCapability(getCurrentModel());
    if (cap && cap.type === 'effort') {
      const effortRow = el('div', { class: 'chat-call-config-effort' });
      const effortLabel = el('span', { class: 'chat-call-config-effort__label' });
      effortLabel.appendChild(icon('M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z', 16));
      effortLabel.appendChild(el('span', {}, `Effort (${thinkingEnabled ? thinkingEffort.charAt(0).toUpperCase() + thinkingEffort.slice(1).toLowerCase() : 'Off'})`));
      effortRow.appendChild(effortLabel);

      const toggleGroup = el('div', { class: 'chat-call-config-effort__toggles' });
      for (const level of cap.levels) {
        const isActive = thinkingEnabled && thinkingEffort === level;
        const btn = el('button', {
          class: `chat-call-config-effort__btn${isActive ? ' chat-call-config-effort__btn--active' : ''}`,
          title: level,
        });
        // Small colored dot
        btn.appendChild(el('span', { class: `chat-call-config-effort__dot${isActive ? ' chat-call-config-effort__dot--active' : ''}` }));
        btn.addEventListener('click', (e) => {
          e.stopPropagation();
          if (thinkingEnabled && thinkingEffort === level) {
            thinkingEnabled = false;
          } else {
            thinkingEnabled = true;
            thinkingEffort = level;
          }
          saveThinkingForModel(getCurrentModel());
          updateThinkBtn();
          updateCallConfigBtn();
          rebuildCallConfigContent(); // re-render, don't close
        });
        toggleGroup.appendChild(btn);
      }
      effortRow.appendChild(toggleGroup);
      callConfigPopover.appendChild(effortRow);
    } else if (cap && cap.type === 'budget') {
      const budgetRow = el('div', { class: 'chat-call-config-effort' });
      budgetRow.appendChild(el('span', { class: 'chat-call-config-effort__label' }, `Thinking budget: ${thinkingBudget}`));
      const slider = el('input', {
        type: 'range', class: 'chat-think-slider',
        min: String(cap.min), max: String(cap.max),
        step: String(Math.max(128, Math.floor((cap.max - cap.min) / 100))),
        value: String(thinkingBudget),
      });
      slider.addEventListener('input', (e) => {
        e.stopPropagation();
        thinkingBudget = parseInt(e.target.value, 10);
        thinkingEnabled = thinkingBudget > 0;
        saveThinkingForModel(getCurrentModel());
        updateThinkBtn();
        updateCallConfigBtn();
        rebuildCallConfigContent();
      });
      budgetRow.appendChild(slider);
      callConfigPopover.appendChild(budgetRow);
    }
  }

  function openCallConfig() {
    closeCallConfig();
    callConfigOpen = true;
    callConfigPopover = el('div', { class: 'chat-call-config-popover' });

    rebuildCallConfigContent();

    const rect = callConfigBtn.getBoundingClientRect();
    callConfigPopover.style.cssText = `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;right:${window.innerWidth - rect.right}px;`;
    document.body.appendChild(callConfigPopover);
  }

  callConfigBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (callConfigOpen) { closeCallConfig(); return; }
    closeModelDropdown();
    openCallConfig();
  });
  document.addEventListener('click', closeCallConfig);

  function updateCallConfigBtn() {
    const cap = getThinkingCapability(getCurrentModel());
    callConfigBtn.classList.toggle('chat-think-btn--active', thinkingEnabled && !!cap);
  }

  // Thinking state
  let thinkingEnabled = false;
  let thinkingEffort = 'medium';
  let thinkingBudget = 8000;

  // Persist thinking config per model name
  const thinkingPerModel = new Map();

  function saveThinkingForModel(model) {
    if (!model) return;
    thinkingPerModel.set(model, { enabled: thinkingEnabled, effort: thinkingEffort, budget: thinkingBudget });
  }

  function restoreThinkingForModel(model) {
    if (!model) return;
    const saved = thinkingPerModel.get(model);
    if (saved) {
      thinkingEnabled = saved.enabled;
      thinkingEffort = saved.effort;
      thinkingBudget = saved.budget;
    } else {
      // Reset to defaults for unknown models
      thinkingEnabled = false;
      thinkingEffort = 'medium';
      thinkingBudget = 8000;
    }
    updateThinkBtn();
    updateCallConfigBtn();
  }

  // Brain (thinking) button — kept for programmatic use but hidden from toolbar
  const thinkBtn = el('button', { class: 'chat-think-btn', title: 'Thinking effort' });
  thinkBtn.appendChild(iconMulti([
    'M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.46 2.5 2.5 0 0 1-1.04-1.54A2.5 2.5 0 0 1 4 15.5a2.5 2.5 0 0 1 0-7 2.5 2.5 0 0 1 1-2A2.5 2.5 0 0 1 9.5 2Z',
    'M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.46 2.5 2.5 0 0 0 1.04-1.54A2.5 2.5 0 0 0 20 15.5a2.5 2.5 0 0 0 0-7 2.5 2.5 0 0 0-1-2A2.5 2.5 0 0 0 14.5 2Z',
  ], 14));
  thinkBtn.style.display = 'none';

  function closeThinkPopover() {}

  function updateThinkBtn() {
    const cap = getThinkingCapability(getCurrentModel());
    thinkBtn.classList.toggle('chat-think-btn--active', thinkingEnabled);
    if (!cap) thinkingEnabled = false;
  }

  // Expose thinking config for use when sending messages
  function getThinkingConfig() {
    if (!thinkingEnabled) return null;
    const cap = getThinkingCapability(getCurrentModel());
    if (!cap) return null;
    if (cap.type === 'effort') return { type: 'effort', value: thinkingEffort };
    if (cap.type === 'budget') return { type: 'budget', value: thinkingBudget };
    return null;
  }

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

  sendBtn.addEventListener('click', async () => {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;

    if (sendBtnIsStop) {
      sendBtn.disabled = true;
      try { await api.abortTask(taskId); } finally { sendBtn.disabled = false; }
      return;
    }

    const text = textarea.value.trim();
    if (!text && attachedFiles.length === 0) return;

    // Resolve thinking budget from UI config
    const thinkConfig = getThinkingConfig();
    let thinkBudget = undefined;
    if (thinkConfig) {
      if (thinkConfig.type === 'budget') thinkBudget = thinkConfig.value;
      else if (thinkConfig.type === 'effort') {
        // Map effort levels to token budgets
        const effortMap = { low: 2000, medium: 10000, high: 20000, max: 32000, LOW: 2000, HIGH: 20000 };
        thinkBudget = effortMap[thinkConfig.value] || 10000;
      }
    }

    if (attachedFiles.length > 0) {
      const imageNames = attachedFiles.filter(f => f.base64).map(f => f.name).join(', ');
      const fullText = imageNames ? `${text}\n\n[Attached images: ${imageNames}]` : text;
      sendMessage(taskId, fullText || text, thinkBudget);
    } else {
      sendMessage(taskId, text, thinkBudget);
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

  // Toolbar left: attach | slash | model | callConfig(brain)
  const toolbarLeft = el('div', { class: 'chat-toolbar-left' });
  toolbarLeft.appendChild(attachBtn);
  toolbarLeft.appendChild(slashBtn);
  toolbarLeft.appendChild(modelBtn);
  toolbarLeft.appendChild(callConfigBtn);

  // Toolbar right: send
  const toolbarRight = el('div', { class: 'chat-toolbar-right' });
  toolbarRight.appendChild(sendBtn);

  inputToolbar.appendChild(toolbarLeft);
  inputToolbar.appendChild(toolbarRight);

  // Input wrapper: bordered box containing textarea on top + toolbar on bottom
  const inputWrapper = el('div', { class: 'chat-input-wrapper' });
  inputWrapper.appendChild(textarea);
  inputWrapper.appendChild(inputToolbar);

  inputArea.appendChild(fileInput);
  inputArea.appendChild(attachmentPills);
  inputArea.appendChild(inputWrapper);

  container.appendChild(headerBar);
  container.appendChild(stickyCard);
  container.appendChild(messagesArea);
  container.appendChild(approvalArea);
  container.appendChild(budgetBanner);
  container.appendChild(changedFilesPanel);
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
    updateCallConfigBtn();
    updateModelBtn();
    updateThinkBtn();
    updateContextBadge();
    updateSendBtn();
    renderApprovalArea();
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      messagesArea.innerHTML = '';
      const emptyEl = el('div', { class: 'chat-empty' });
      emptyEl.appendChild(el('div', { class: 'chat-empty__prompt' }, 'What would you like to do?'));
      const hints = el('div', { class: 'chat-empty__hints' });
      for (const hint of ['Fix a bug', 'Explain code', 'Refactor', 'Add a feature']) {
        const pill = el('button', { class: 'chat-empty__hint' }, hint);
        pill.addEventListener('click', () => {
          textarea.value = hint;
          textarea.focus();
        });
        hints.appendChild(pill);
      }
      emptyEl.appendChild(hints);
      messagesArea.appendChild(emptyEl);
      return;
    }

    const tasks = agentStore.getState('tasks');
    const task = tasks[taskId];
    if (!task) return;

    // Render messages immediately, then reload checkpoints in background.
    // This avoids the flash of empty content between clear and async populate.
    renderMessages(task);
    loadCheckpoints(taskId);
  }

  function renderMessages(task) {
    // Capture scroll state before clearing so we can restore it
    const prevDistFromBottom =
      messagesArea.scrollHeight - messagesArea.scrollTop - messagesArea.clientHeight;
    const wasAtBottom = prevDistFromBottom <= 80;

    messagesArea.innerHTML = '';
    changedFilesPanel.innerHTML = '';
    changedFilesPanel.classList.remove('chat-changed-files--visible');

    const taskId = agentStore.getState('activeTaskId');
    const isRunning = task.status === 'Running';
    const isFailed = task.status === 'Failed';

    // Pre-build tool_use_id → result block map from all tool messages
    const resultMap = buildResultMap(task.messages);

    // Find last user message index (for stop/retry buttons)
    let lastUserMsgIdx = -1;
    for (let i = task.messages.length - 1; i >= 0; i--) {
      if (task.messages[i].role === 'user') { lastUserMsgIdx = i; break; }
    }

    // ── Pipeline: normalize → collapse read/search → group parallel ──
    const nodes = processMessages(task.messages, resultMap);

    // Find the first user message index — it's shown in the sticky card, so skip it below
    let firstUserMsgIdx = -1;
    for (let i = 0; i < task.messages.length; i++) {
      if (task.messages[i].role === 'user') { firstUserMsgIdx = i; break; }
    }

    for (const node of nodes) {
      switch (node.type) {

        case 'task-complete': {
          const block = node.content;
          messagesArea.appendChild(renderCompletionCard(block.summary, block.notes, null));
          if (block.diff && block.diff.files && block.diff.files.length > 0) {
            populateChangedFilesPanel(changedFilesPanel, block.diff, task);
          }
          break;
        }

        case 'model-switch': {
          const switchToModel = node.content.to_model;
          const currentModel = task.model || task.info?.model || '';
          const isCurrentModel = switchToModel === currentModel;
          messagesArea.appendChild(renderModelSwitchSeparator(
            switchToModel,
            isCurrentModel && thinkingEnabled ? thinkingEffort : null,
            isCurrentModel && thinkingEnabled ? thinkingBudget : null,
          ));
          break;
        }

        case 'user-message': {
          const msg = node.msg;
          const i = node.msgIdx;
          // Skip first user message — shown in sticky card at top
          if (i === firstUserMsgIdx) break;
          const msgEl = el('div', { class: 'chat-message chat-message--user' });
          for (const block of msg.content) {
            if (block.type === 'text' && block.text) {
              const textEl = el('div', { class: 'chat-message__text' });
              textEl.innerHTML = formatText(block.text);
              msgEl.appendChild(textEl);
            }
          }
          // Hover action bar
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
          break;
        }

        case 'thinking-indicator': {
          messagesArea.appendChild(renderThinkingIndicator());
          break;
        }

        case 'thinking': {
          // A thinking block is "still streaming" if:
          // 1. The task is streaming and this is the last assistant message, AND
          // 2. This block is the last content block (or followed only by an empty text placeholder).
          const msgContent = task.messages[node.msgIdx]?.content || [];
          const blockIndex = node.contentIdx;
          // Find the last assistant message index (tool results may come after it)
          let lastAssistantIdx = -1;
          for (let mi = task.messages.length - 1; mi >= 0; mi--) {
            if (task.messages[mi].role === 'assistant') { lastAssistantIdx = mi; break; }
          }
          const isInLastAssistantMsg = node.msgIdx === lastAssistantIdx;
          const isStreaming = task.isStreaming && isInLastAssistantMsg;
          const isLastOrFollowedByEmptyText = blockIndex >= 0 && (
            blockIndex === msgContent.length - 1 ||
            (blockIndex === msgContent.length - 2 &&
             msgContent[msgContent.length - 1]?.type === 'text' &&
             !msgContent[msgContent.length - 1]?.text));
          const isThisBlockStreaming = isStreaming && isLastOrFollowedByEmptyText;
          const thinkingKey = `thinking-${node.blockIdx}`;
          messagesArea.appendChild(renderThinkingBlock(node.block, isThisBlockStreaming, thinkingKey));
          break;
        }

        case 'assistant-text': {
          const isStreaming = task.isStreaming && node.isLastMsg;
          const wrapper = el('div', { class: 'chat-message chat-message--assistant' });
          const lastBlock = node.blocks[node.blocks.length - 1];
          for (const block of node.blocks) {
            const textEl = el('div', {
              class: `chat-message__text${isStreaming && block === lastBlock ? ' chat-message__text--streaming' : ''}`,
            });
            textEl.innerHTML = formatText(block.text);
            wrapper.appendChild(textEl);
          }
          messagesArea.appendChild(wrapper);
          break;
        }

        case 'tool-use': {
          // Show todo_write as a minimal inline indicator (details in sticky card)
          if (node.toolName === 'todo_write') {
            messagesArea.appendChild(renderMinimalToolIndicator('todo_write', node.block, node.toolResult));
            break;
          }
          // Subagent tools get custom rendering
          if (node.toolName === 'spawn_subagent') {
            messagesArea.appendChild(renderSubagentCard(node.block, node.toolResult));
            break;
          }
          if (node.toolName === 'wait_for_subagents' || node.toolName === 'list_active_agents') {
            messagesArea.appendChild(renderMinimalToolIndicator(node.toolName, node.block, node.toolResult));
            break;
          }
          messagesArea.appendChild(renderToolCallCard(node.block, node.toolResult));
          break;
        }

        case 'collapsed-group': {
          messagesArea.appendChild(renderCollapsedGroup(node));
          break;
        }

        case 'parallel-group': {
          messagesArea.appendChild(renderParallelGroup(node));
          break;
        }

        case 'checkpoint-anchor': {
          if (hasFileChanges(node.msg)) {
            const cp = findCheckpointForMessage(node.msgIdx);
            if (cp && cp.file_count > 0) {
              messagesArea.appendChild(renderCheckpointMarker(cp, taskId));
            }
          }
          break;
        }
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

  // ── Collapsed read/search group ────────────────────────────
  function renderCollapsedGroup(group) {
    const container = el('div', { class: 'collapsed-group' });

    // Header row — always visible
    const header = el('button', { class: 'collapsed-group__header' });

    // Icon
    const iconWrap = el('span', { class: 'collapsed-group__icon' });
    iconWrap.appendChild(icon('M15 12a3 3 0 11-6 0 3 3 0 016 0zM2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z', 13));
    header.appendChild(iconWrap);

    // Summary text
    header.appendChild(el('span', { class: 'collapsed-group__summary' }, group.summary));

    // Status badge
    const statusEl = el('span', { class: 'collapsed-group__status' });
    if (group.allCompleted) {
      const checkPath = group.anyError ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
      statusEl.appendChild(icon(checkPath, 12));
      statusEl.classList.add(group.anyError ? 'collapsed-group__status--error' : 'collapsed-group__status--ok');
    } else {
      statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
    }
    header.appendChild(statusEl);

    // Chevron
    const chevron = el('span', { class: 'collapsed-group__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    header.appendChild(chevron);

    container.appendChild(header);

    // Expandable body with individual tool cards
    const body = el('div', { class: 'collapsed-group__body collapsed-group__body--hidden' });
    for (const child of group.children) {
      if (child.toolName === 'spawn_subagent') {
        body.appendChild(renderSubagentCard(child.block, child.toolResult));
      } else {
        body.appendChild(renderToolCallCard(child.block, child.toolResult));
      }
    }
    container.appendChild(body);

    // Toggle — restore persistent state
    const groupKey = `group-${group.children[0]?.toolUseId || group.children[0]?.msgIdx}`;
    const wasOpen = expandedState.get(groupKey);
    if (wasOpen) {
      body.classList.remove('collapsed-group__body--hidden');
      chevron.style.transform = 'rotate(180deg)';
    }
    header.addEventListener('click', () => {
      const isOpen = !body.classList.contains('collapsed-group__body--hidden');
      const newOpen = !isOpen;
      body.classList.toggle('collapsed-group__body--hidden', !newOpen);
      chevron.style.transform = newOpen ? 'rotate(180deg)' : '';
      expandedState.set(groupKey, newOpen);
    });

    return container;
  }

  // ── Parallel tool group ────────────────────────────────────
  function renderParallelGroup(group) {
    const container = el('div', { class: 'parallel-group' });

    // Render each child (could be tool-use or collapsed-group)
    for (const child of group.children) {
      if (child.type === 'collapsed-group') {
        container.appendChild(renderCollapsedGroup(child));
      } else if (child.type === 'tool-use') {
        if (child.toolName === 'spawn_subagent') {
          container.appendChild(renderSubagentCard(child.block, child.toolResult));
        } else {
          container.appendChild(renderToolCallCard(child.block, child.toolResult));
        }
      }
    }

    return container;
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

      // Buttons only — no countdown, wait indefinitely for user response
      const actions = el('div', { class: 'chat-approval-widget__actions' });

      const denyBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--deny' }, 'Deny');
      const allowBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--allow' }, 'Allow');

      denyBtn.addEventListener('click', () => {
        respondToPermission(taskId, req.request_id, false);
      });

      allowBtn.addEventListener('click', () => {
        respondToPermission(taskId, req.request_id, true);
      });

      actions.appendChild(denyBtn);
      actions.appendChild(allowBtn);
      widget.appendChild(actions);
      approvalArea.appendChild(widget);
    }
  }

  // ── Smart incremental updates ─────────────────────────────
  // Instead of rebuilding the entire DOM on every state change, we detect
  // what changed and apply targeted updates. Full re-render is a last resort.
  let renderRafId = null;

  function autoScrollIfNeeded() {
    const distFromBottom = messagesArea.scrollHeight - messagesArea.scrollTop - messagesArea.clientHeight;
    if (distFromBottom <= 80) {
      messagesArea.scrollTop = messagesArea.scrollHeight;
    }
  }

  function scheduleFullRender() {
    if (renderRafId) cancelAnimationFrame(renderRafId);
    renderRafId = requestAnimationFrame(() => { renderRafId = null; render(); });
  }

  agentStore.subscribe('tasks', () => {
    updateCostDisplay();
    updateHeaderBar();
    renderBudgetBanner();
    renderStickyCard();

    const taskId = agentStore.getState('activeTaskId');
    const task = taskId && agentStore.getState('tasks')[taskId];
    if (!task) { scheduleFullRender(); return; }

    // During streaming, the most frequent events are text deltas and thinking deltas.
    // We intercept these and do targeted DOM updates to avoid the full rebuild flicker.
    if (task.isStreaming) {
      const msgs = task.messages;
      const lastMsg = msgs[msgs.length - 1];
      if (lastMsg?.role === 'assistant') {
        const lastBlock = lastMsg.content[lastMsg.content.length - 1];

        // ── Fast-path: Text delta ──
        // Update only the streaming text element in-place.
        if (lastBlock?.type === 'text') {
          const streamingEl = messagesArea.querySelector('.chat-message__text--streaming');
          if (streamingEl && lastBlock.text) {
            streamingEl.innerHTML = formatText(lastBlock.text);
            autoScrollIfNeeded();
            return; // Skip full re-render
          }
        }

        // ── Fast-path: Thinking delta ──
        // The shimmer animation is already showing — update word count and content in-place.
        // We skip full re-render to prevent collapsing the thinking UI.
        if (lastBlock?.type === 'thinking') {
          const thinkingEl = messagesArea.querySelector('.thinking-block--streaming');
          if (thinkingEl) {
            // Update word count for the live timer display
            const thinkingKey = thinkingEl.getAttribute('data-thinking-key');
            if (thinkingKey && lastBlock.thinking) {
              thinkingWordCounts.set(thinkingKey, countWords(lastBlock.thinking));
            }
            // Update thinking content in expandable body
            const contentEl = thinkingEl.querySelector('.thinking-block__content--streaming');
            if (contentEl && lastBlock.thinking) {
              contentEl.textContent = lastBlock.thinking;
            }
            autoScrollIfNeeded();
            return; // Skip full re-render
          }
        }
      }
    }

    // All other state changes — debounced full re-render
    scheduleFullRender();
  });
  agentStore.subscribe('activeTaskId', () => { render(); updateCostDisplay(); updateHeaderBar(); renderBudgetBanner(); renderStickyCard(); });
  agentStore.subscribe('permissionRequests', renderApprovalArea);
  agentStore.subscribe('turnBudgetWarnings', renderBudgetBanner);
  agentStore.subscribe('todos', renderStickyCard);

  // Throttled re-render on subagent state changes (text deltas fire very frequently)
  let subagentRenderTimer = null;
  agentStore.subscribe('subagents', () => {
    if (subagentRenderTimer) return;
    subagentRenderTimer = setTimeout(() => {
      subagentRenderTimer = null;
      scheduleFullRender();
      updateCostDisplay(); // aggregate subagent costs into header stats
    }, 300);
  });

  render();
  updateCostDisplay();
  updateHeaderBar();
  renderBudgetBanner();
  renderStickyCard();

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
 * Build a map of tool_use_id → tool_result block from all messages.
 * Tool results appear as role 'tool' during live execution and as role 'user'
 * when loaded from the database (the API sends tool results with User role).
 */
function buildResultMap(messages) {
  const map = new Map();
  for (const msg of messages) {
    if (msg.role === 'tool' || msg.role === 'user') {
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
  chat_message:   { label: 'Message',        iconPath: 'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z', color: 'purple', special: 'chat_message' },
  spawn_subagent: { label: 'Subagent',       iconPath: 'M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', color: 'purple' },
  wait_for_subagents: { label: 'Wait for subagents', iconPath: 'M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z', color: 'gray' },
  list_active_agents: { label: 'List agents', iconPath: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', color: 'gray' },
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

// Track thinking start times for elapsed display
const thinkingStartTimes = new Map();
const thinkingWordCounts = new Map();

/**
 * Render a collapsible thinking block.
 * While streaming: shows "Thinking... Xs" with elapsed time.
 * Once done: shows "Thought for Xs", collapses by default.
 */
function countWords(text) {
  if (!text) return 0;
  return text.trim().split(/\s+/).filter(Boolean).length;
}

function renderThinkingBlock(block, isStreaming, stateKey) {
  const card = el('div', { class: `thinking-block${isStreaming ? ' thinking-block--streaming' : ''}` });
  if (stateKey) card.setAttribute('data-thinking-key', stateKey);

  const header = el('button', { class: 'thinking-block__header' });

  // Brain icon
  const brainIcon = el('span', { class: 'thinking-block__icon' });
  brainIcon.appendChild(icon('M9.5 2a6.5 6.5 0 0 1 6.48 7.13A4.5 4.5 0 0 1 17 18H7a5 5 0 0 1-2.1-9.52A6.5 6.5 0 0 1 9.5 2z', 13));
  header.appendChild(brainIcon);

  // Header first, then body — so body appears BELOW header
  card.appendChild(header);

  if (isStreaming) {
    // Track start time and word count
    if (!thinkingStartTimes.has(stateKey)) {
      thinkingStartTimes.set(stateKey, Date.now());
    }
    // Update word count from current thinking text
    const words = countWords(block.thinking);
    thinkingWordCounts.set(stateKey, words);

    const shimmer = el('span', { class: 'thinking-block__label thinking-block__label--shimmer' }, 'Thinking');
    const metaEl = el('span', { class: 'thinking-block__meta' });
    const startTime = thinkingStartTimes.get(stateKey);
    const updateMeta = () => {
      const secs = Math.round((Date.now() - startTime) / 1000);
      const wc = thinkingWordCounts.get(stateKey) || 0;
      metaEl.textContent = `${secs}s · ${wc} words`;
    };
    updateMeta();
    header.appendChild(shimmer);
    header.appendChild(metaEl);

    // Chevron for expand/collapse during streaming
    const chevron = el('span', { class: 'thinking-block__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    header.appendChild(chevron);

    // Expandable body — view thinking content while streaming
    const wasOpen = stateKey && expandedState.get(stateKey);
    const body = el('div', { class: `thinking-block__body${wasOpen ? '' : ' thinking-block__body--hidden'}` });
    const pre = el('pre', { class: 'thinking-block__content thinking-block__content--streaming' });
    pre.textContent = block.thinking || '';
    body.appendChild(pre);
    card.appendChild(body);

    if (wasOpen) chevron.style.transform = 'rotate(180deg)';

    header.addEventListener('click', () => {
      const isOpen = !body.classList.contains('thinking-block__body--hidden');
      const newOpen = !isOpen;
      body.classList.toggle('thinking-block__body--hidden', !newOpen);
      chevron.style.transform = newOpen ? 'rotate(180deg)' : '';
      if (stateKey) expandedState.set(stateKey, newOpen);
    });

    // Update every second
    const timer = setInterval(updateMeta, 1000);
    const observer = new MutationObserver(() => {
      if (!document.body.contains(card)) { clearInterval(timer); observer.disconnect(); }
    });
    observer.observe(document.body, { childList: true, subtree: true });
  } else {
    // Calculate duration: prefer stamped duration_secs (persisted), fall back to client-side timer
    let durationSecs = 0;
    if (block.duration_secs != null) {
      durationSecs = block.duration_secs;
    } else if (thinkingStartTimes.has(stateKey)) {
      durationSecs = Math.round((Date.now() - thinkingStartTimes.get(stateKey)) / 1000);
    }
    thinkingStartTimes.delete(stateKey);
    const words = countWords(block.thinking);
    thinkingWordCounts.delete(stateKey);

    // Format: "Thought for Xs" then separator then word count
    if (durationSecs < 1) durationSecs = 1;
    const durationStr = `Thought for ${durationSecs}s`;
    const labelEl = el('span', { class: 'thinking-block__label' });
    labelEl.textContent = durationStr;
    header.appendChild(labelEl);

    if (words > 0) {
      header.appendChild(el('span', { class: 'thinking-block__separator' }, '•'));
      header.appendChild(el('span', { class: 'thinking-block__word-count' }, `${words} words`));
    }

    const chevron = el('span', { class: 'thinking-block__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    header.appendChild(chevron);

    // Expandable body — restore persistent expand state, appended AFTER header
    const wasOpen = stateKey && expandedState.get(stateKey);
    const body = el('div', { class: `thinking-block__body${wasOpen ? '' : ' thinking-block__body--hidden'}` });
    const pre = el('pre', { class: 'thinking-block__content' });
    pre.textContent = block.thinking || '';
    body.appendChild(pre);
    card.appendChild(body);

    if (wasOpen) chevron.style.transform = 'rotate(180deg)';

    header.addEventListener('click', () => {
      const isOpen = !body.classList.contains('thinking-block__body--hidden');
      const newOpen = !isOpen;
      body.classList.toggle('thinking-block__body--hidden', !newOpen);
      chevron.style.transform = newOpen ? 'rotate(180deg)' : '';
      if (stateKey) expandedState.set(stateKey, newOpen);
    });
  }

  return card;
}

/**
 * Render a chat_message card.
 * For type "question": shows question prominently, waits for response.
 * For type "message": shows the message as a styled info card.
 */
function renderChatMessageCard(block, result) {
  const { input = {}, id } = block;
  const text = input.text || input.question || JSON.stringify(input);
  const msgType = input.type || 'message';
  const isPending = !result;
  const hasResponse = result && !result.is_error;

  const isQuestion = msgType === 'question';
  const cardClass = isQuestion ? 'chat-msg-card chat-msg-card--question' : 'chat-msg-card chat-msg-card--info';
  const card = el('div', { class: cardClass, 'data-tool-use-id': id });

  // Header
  const header = el('div', { class: 'chat-msg-card__header' });
  if (isQuestion) {
    header.appendChild(icon('M8.228 9c.549-1.165 2.03-2 3.772-2 2.21 0 4 1.343 4 3 0 1.4-1.278 2.575-3.006 2.907-.542.104-.994.54-.994 1.093m0 3h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z', 15));
    header.appendChild(el('span', {}, isPending ? 'Waiting for your response' : 'Question'));
  } else {
    header.appendChild(icon('M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z', 15));
    header.appendChild(el('span', {}, 'Agent'));
  }
  card.appendChild(header);

  // Message body (rendered as markdown)
  const bodyEl = el('div', { class: 'chat-msg-card__body' });
  bodyEl.innerHTML = formatText(text);
  card.appendChild(bodyEl);

  // Response (only for questions that have been answered)
  if (isQuestion && hasResponse) {
    const responseEl = el('div', { class: 'chat-msg-card__response' });
    responseEl.appendChild(el('span', { class: 'chat-msg-card__response-label' }, 'Your response:'));
    responseEl.appendChild(el('span', {}, String(result.content)));
    card.appendChild(responseEl);
  }

  return card;
}

/**
 * Render a minimal inline indicator for tool calls that are shown elsewhere (e.g. todo_write).
 */
function renderMinimalToolIndicator(toolName, block, result) {
  const isPending = !result;
  const isError = result?.is_error;
  const indicator = el('div', { class: 'tool-indicator' });

  const iconEl = el('span', { class: 'tool-indicator__icon' });
  // Checkmark or spinner
  if (isPending) {
    iconEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else if (isError) {
    iconEl.appendChild(icon('M18 6L6 18M6 6l12 12', 11));
    iconEl.classList.add('tool-indicator__icon--error');
  } else {
    iconEl.appendChild(icon('M5 13l4 4L19 7', 11));
    iconEl.classList.add('tool-indicator__icon--ok');
  }
  indicator.appendChild(iconEl);

  // Label
  const labels = {
    todo_write: 'Updated todo list',
    wait_for_subagents: 'Waiting for subagents',
    list_active_agents: 'Checked subagent status',
  };
  const labelText = labels[toolName] || `Used ${toolName}`;
  indicator.appendChild(el('span', { class: 'tool-indicator__label' }, labelText));

  return indicator;
}

// ── Subagent card ────────────────────────────────────────────────────────────

/**
 * Slugify a name the same way the backend does: lowercase, non-alphanum → hyphen, trim, cap 30.
 */
function slugifyAgentName(name) {
  if (!name) return '';
  let slug = name.toLowerCase().replace(/[^a-z0-9]/g, '-').replace(/^-+|-+$/g, '');
  if (slug.length > 30) slug = slug.slice(0, 30);
  return slug;
}

/**
 * Render a subagent card: single inline row.
 * Layout: [icon] name [↑ input] [↓ output] wordCount spinner/✓/✗
 */
function renderSubagentCard(block, result) {
  const { input = {}, id } = block;
  const name = input.name || input.description || 'subagent';
  const prompt = input.prompt || '';
  const agentId = slugifyAgentName(name);

  // Look up live subagent state
  const taskId = agentStore.getState('activeTaskId');
  const subagents = agentStore.getState('subagents');
  const liveAgent = subagents?.[taskId]?.[agentId];

  const status = liveAgent?.status || (result ? (result.is_error ? 'failed' : 'completed') : 'running');
  const liveOutput = liveAgent?.output || '';
  const livePrompt = liveAgent?.prompt || prompt;

  const isRunning = status === 'running';
  const isFailed = status === 'failed';

  const statusClass = isRunning ? '' : isFailed ? ' subagent-card--failed' : ' subagent-card--completed';
  const card = el('div', { class: `subagent-card${statusClass}`, 'data-tool-use-id': id });

  const row = el('div', { class: 'subagent-card__row' });

  // Agent icon (purple)
  const iconWrap = el('span', { class: 'tool-call__icon tool-call__icon--purple' });
  iconWrap.appendChild(icon('M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', 13));
  row.appendChild(iconWrap);

  // Agent name
  row.appendChild(el('span', { class: 'tool-call__name' }, name));

  // ↑ Input button (arrow up) with token count
  const inputBtn = el('button', { class: 'subagent-card__arrow-btn', title: 'View input prompt' });
  inputBtn.appendChild(icon('M5 15l7-7 7 7', 11));
  inputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    openScratchInEditor(`[Input] ${name}`, livePrompt, 'markdown');
  });
  row.appendChild(inputBtn);

  // ↑ Token count (sent/input tokens)
  const liveCost = liveAgent?.cost;
  const inputTokens = liveCost?.total_input_tokens || 0;
  const inputTokenEl = el('span', { class: 'subagent-card__tokens subagent-card__tokens--sent' }, inputTokens > 0 ? formatTokens(inputTokens) : '');
  row.appendChild(inputTokenEl);

  // ↓ Output button (arrow down) with token count
  const outputBtn = el('button', { class: 'subagent-card__arrow-btn', title: 'View output' });
  outputBtn.appendChild(icon('M19 9l-7 7-7-7', 11));
  outputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    const currentOutput = agentStore.getState('subagents')?.[taskId]?.[agentId]?.output || liveOutput;
    if (currentOutput) {
      openScratchInEditor(`[Output] ${name}`, currentOutput, 'markdown');
    }
  });
  row.appendChild(outputBtn);

  // ↓ Token count (received/output tokens)
  const outputTokens = liveCost?.total_output_tokens || 0;
  const outputTokenEl = el('span', { class: 'subagent-card__tokens subagent-card__tokens--recv' }, outputTokens > 0 ? formatTokens(outputTokens) : '');
  row.appendChild(outputTokenEl);

  // Word count (updates live as streaming arrives)
  const wordCount = liveOutput ? liveOutput.trim().split(/\s+/).filter(Boolean).length : 0;
  const wordEl = el('span', { class: 'subagent-card__words' }, wordCount > 0 ? `${wordCount} words` : '');

  // Cost display
  const subCostUsd = liveCost?.estimated_cost_usd || 0;
  const costEl = el('span', { class: 'subagent-card__cost' }, subCostUsd > 0 ? `$${subCostUsd.toFixed(3)}` : '');
  row.appendChild(wordEl);
  row.appendChild(costEl);

  // Status: spinner | ✓ | ✗  (right side)
  const statusEl = el('span', { class: 'tool-call__status' });
  if (isRunning) {
    statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    const checkPath = isFailed ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
    statusEl.appendChild(icon(checkPath, 12));
    statusEl.classList.add(isFailed ? 'tool-call__status--error' : 'tool-call__status--ok');
  }
  row.appendChild(statusEl);

  card.appendChild(row);
  return card;
}

/**
 * Open content as a scratch buffer in the editor (registers in editor state).
 */
async function openScratchInEditor(title, content, language) {
  try {
    const info = await api.openScratchBuffer(title, content, language);
    if (!info) return;
    // Register in editor store so the tab appears
    const { editorStore, setActiveBuffer } = await import('../../state/editor.js');
    const buffer = {
      id: info.id,
      filePath: info.file_path,
      fileName: info.file_name,
      projectName: '',
      lineCount: info.line_count,
      language: info.language,
      isModified: false,
      fileType: 'code',
      isPreview: false,
      isDualMode: false,
      viewMode: 'edit',
    };
    const newBuffers = { ...editorStore.getState('openBuffers'), [info.id]: buffer };
    editorStore.setState({ openBuffers: newBuffers });
    setActiveBuffer(info.id);
  } catch (e) {
    console.error('Failed to open scratch buffer:', e);
  }
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

  // ── Special rendering for chat_message ──────────────────────────
  if (name === 'chat_message') {
    return renderChatMessageCard(block, result);
  }

  const statusClass = isPending ? '' : isError ? ' tool-call--error' : ' tool-call--success';
  const card = el('div', { class: `tool-call${statusClass}`, 'data-tool-use-id': id });

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

  // Toggle expand / collapse — restore persistent state
  const toolKey = `tool-${id}`;
  const wasOpen = expandedState.get(toolKey);
  if (wasOpen) {
    body.classList.remove('tool-call__body--hidden');
    chevron.style.transform = 'rotate(180deg)';
  }
  header.addEventListener('click', () => {
    const isOpen = !body.classList.contains('tool-call__body--hidden');
    const newOpen = !isOpen;
    body.classList.toggle('tool-call__body--hidden', !newOpen);
    chevron.style.transform = newOpen ? 'rotate(180deg)' : '';
    expandedState.set(toolKey, newOpen);
  });

  return card;
}

// ─────────────────────────────────────────────────────────────────────────────

function renderModelSwitchSeparator(toModel, thinkEffort, thinkBudget) {
  const sep = el('div', { class: 'chat-model-switch' });
  sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
  let label = `Model: ${toModel}`;
  if (thinkEffort) label += ` · thinking: ${thinkEffort}`;
  else if (thinkBudget > 0) label += ` · thinking: ${thinkBudget} tokens`;
  sep.appendChild(el('span', { class: 'chat-model-switch__label' }, label));
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

function getTaskProjectRoot(task) {
  const projectId = task.project_id || task.projectId;
  if (!projectId) return null;
  const projects = workspaceStore.getState('projects');
  const project = projects.find((p) => String(p.id) === String(projectId));
  return project ? project.root_path : null;
}

function populateChangedFilesPanel(panel, diff, task) {
  panel.innerHTML = '';

  const projectRoot = getTaskProjectRoot(task) || '';
  const sep = projectRoot.includes('/') ? '/' : '\\';

  // Toggle header
  const toggle = el('div', { class: 'chat-changed-files__toggle' });
  const arrowIcon = icon('M19 9l-7 7-7-7', 14);
  arrowIcon.style.transition = 'transform 0.15s';
  toggle.appendChild(arrowIcon);
  toggle.appendChild(
    el('span', {}, `${diff.files.length} file${diff.files.length !== 1 ? 's' : ''} changed in this conversation`)
  );
  const stats = el('span', { class: 'chat-changed-files__stats' });
  if (diff.total_insertions > 0) stats.appendChild(el('span', { class: 'chat-changed-files__additions' }, `+${diff.total_insertions}`));
  if (diff.total_deletions > 0) stats.appendChild(el('span', { class: 'chat-changed-files__deletions' }, `\u2212${diff.total_deletions}`));
  toggle.appendChild(stats);
  panel.appendChild(toggle);

  // File list (collapsed by default)
  const fileList = el('div', { class: 'chat-changed-files__list chat-changed-files__list--collapsed' });

  const maxChanges = Math.max(...diff.files.map((f) => f.insertions + f.deletions), 1);

  for (const file of diff.files) {
    const row = el('div', { class: 'chat-changed-files__row' });

    // Status icon
    const statusClass =
      file.status === 'Created' ? 'chat-changed-files__status--created' :
      file.status === 'Deleted' ? 'chat-changed-files__status--deleted' :
      'chat-changed-files__status--modified';
    row.appendChild(
      el('span', { class: `chat-changed-files__status ${statusClass}` },
        file.status === 'Created' ? '+' : file.status === 'Deleted' ? '\u2212' : '~')
    );

    // File path
    const pathEl = el('span', { class: 'chat-changed-files__path' }, file.path);
    row.appendChild(pathEl);

    // Change counts
    const counts = el('span', { class: 'chat-changed-files__counts' });
    if (file.insertions > 0) counts.appendChild(el('span', { class: 'chat-changed-files__additions' }, `+${file.insertions}`));
    if (file.deletions > 0) counts.appendChild(el('span', { class: 'chat-changed-files__deletions' }, `\u2212${file.deletions}`));
    row.appendChild(counts);

    // Mini bar
    const ratio = (file.insertions + file.deletions) / maxChanges;
    const bar = el('div', { class: 'chat-changed-files__bar' });
    bar.appendChild(el('div', {
      class: 'chat-changed-files__bar-fill',
      style: `width: ${Math.round(ratio * 100)}%`,
    }));
    row.appendChild(bar);

    // Open-in-editor icon
    const openBtn = el('button', { class: 'chat-changed-files__open-btn', title: 'Open diff in editor' });
    openBtn.appendChild(icon('M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3', 12));
    openBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const absPath = projectRoot ? projectRoot + sep + file.path.replace(/[\\/]/g, sep) : file.path;
      openDiffView({ filePath: absPath, isStaged: false });
    });
    row.appendChild(openBtn);

    // Click row → open diff in editor
    row.addEventListener('click', () => {
      const absPath = projectRoot ? projectRoot + sep + file.path.replace(/[\\/]/g, sep) : file.path;
      openDiffView({ filePath: absPath, isStaged: false });
    });

    fileList.appendChild(row);
  }

  panel.appendChild(fileList);

  // Toggle expand/collapse
  let expanded = false;
  toggle.style.cursor = 'pointer';
  toggle.addEventListener('click', () => {
    expanded = !expanded;
    fileList.classList.toggle('chat-changed-files__list--collapsed', !expanded);
    arrowIcon.style.transform = expanded ? 'rotate(180deg)' : '';
  });

  panel.classList.add('chat-changed-files--visible');
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
