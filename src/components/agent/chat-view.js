import { el, icon, iconMulti } from '../../utils/dom.js';
import { agentStore, sendMessage, setActiveTask, setTaskPermissions, setTaskSensitiveAccess, respondToPermission, respondToAgentQuestion, retryFromCheckpoint, setPendingProjectId, setPendingModelChoice, setPendingPermissionLevel, setPendingSensitiveAccess, setPendingThinking, createTask, deleteTaskAction, GLOBAL_PROJECT_ID } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { terminalStore } from '../../state/terminal.js';
import { openDiffView } from '../../state/editor.js';
import * as api from '../../lib/tauri-api.js';
import { loadProviderConfigs, saveProviderConfigs, refreshAllProviderModels, pricingFor } from '../settings/ai-settings.js';
import { getCustomModel } from '../../state/custom-models.js';
import { openCustomModelModal } from '../settings/custom-model-modal.js';
import { renderMarkdown } from '../../lib/markdown.js';
import { processMessages } from '../../utils/message-pipeline.js';
import { formatRelativeTime } from '../../utils/format-time.js';
import { showConfirmDialog, showAlertDialog } from '../confirm-dialog.js';
import { attachCodeCopyButtons } from './chat-view/code-copy.js';
import { openImageLightbox } from './chat-view/image-lightbox.js';
import {
  TOOL_META,
  TOOL_META_DEFAULT,
  getToolSummary,
  formatToolOutput,
  formatToolInput,
} from './chat-view/tool-meta.js';

// Prompt the user to register any model not present in the built-in registry
// and not yet saved as a custom entry. Returns `true` if the selection may
// proceed, `false` if the user dismissed the registration modal. Also
// re-applies the selected model's spec (custom or zeroed) onto the provider
// config so the backend's context-window and max-output-tokens stay per-model-
// accurate after every switch.
async function pickModel(providerId, modelId) {
  if (!providerId || !modelId) return true;
  const providerType = providerId.startsWith('Compatible:') ? 'Compatible' : providerId;

  if (!pricingFor(modelId) && !getCustomModel(modelId)) {
    const ok = await new Promise((resolve) => {
      openCustomModelModal({
        modelId,
        providerType,
        onSaved: () => resolve(true),
        onCancelled: () => resolve(false),
      });
    });
    if (!ok) return false;
  }

  const configs = loadProviderConfigs();
  const cfg = configs[providerId];
  if (!cfg || !cfg.hasKey) return true;

  // Registry-known models → zero out any prior custom overrides so the Rust
  // registry values (context window, pricing) govern. Custom-registered models
  // → push their spec so condensing & max-output calcs use the right numbers.
  const custom = getCustomModel(modelId);
  const maxOut = custom?.maxOutputTokens  || 0;
  const inCost = custom?.inputCost        || 0;
  const outCost = custom?.outputCost      || 0;
  const cIn    = custom?.cachedInputCost  || 0;
  const cOut   = custom?.cachedOutputCost || 0;
  const ctxW   = custom?.contextWindow    || 0;
  const think  = cfg.customThinkingBudget || 0;

  try {
    await api.setAiProvider(
      providerType, '__STORED__', modelId, cfg.baseUrl || null, null,
      maxOut, inCost, outCost, cIn, cOut, ctxW, think, cfg.name || null,
    );
  } catch (e) { console.warn('[pickModel] setAiProvider failed:', e); }

  cfg.model = modelId;
  cfg.customMaxOutputTokens   = maxOut;
  cfg.customInputCost         = inCost;
  cfg.customOutputCost        = outCost;
  cfg.customCachedInputCost   = cIn;
  cfg.customCachedOutputCost  = cOut;
  cfg.customContextWindow     = ctxW;
  configs[providerId] = cfg;
  saveProviderConfigs(configs);
  return true;
}

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

// Persistent expand/collapse state — survives DOM rebuilds.
// Keys: "thinking-{msgIdx}", "tool-{tool_use_id}", "group-{firstToolUseId}"
const expandedState = new Map();

// Returns thinking capability info for the given model, or null if not supported.
function getThinkingCapability(model) {
  if (!model) return null;
  if (model.includes('claude-opus-4')) return { type: 'effort', levels: ['low', 'medium', 'high', 'max'] };
  if (model.includes('claude-sonnet-4') || model.includes('claude-haiku-4')) return { type: 'effort', levels: ['low', 'medium', 'high'] };
  // OpenAI GPT-5 family. Levels differ per sub-family:
  //   - gpt-5.x-codex (5.2-codex, 5.3-codex…): low/medium/high/xhigh
  //   - gpt-5-codex (original):                minimal/low/medium/high
  //   - gpt-5.4:                               low/medium/high/xhigh
  //   - gpt-5.1/5.2/5.3 (non-codex):           low/medium/high
  //   - gpt-5 / gpt-5-mini/-nano/-pro / chatgpt-5: minimal/low/medium/high
  // Only GPT-5 and above — older reasoning models (o1/o3/o4) are intentionally excluded.
  if (/^gpt-5\.\d+-codex/.test(model))               return { type: 'effort', levels: ['low', 'medium', 'high', 'xhigh'] };
  if (model === 'gpt-5-codex' || model.startsWith('gpt-5-codex')) return { type: 'effort', levels: ['minimal', 'low', 'medium', 'high'] };
  if (/^gpt-5\.4/.test(model))                       return { type: 'effort', levels: ['low', 'medium', 'high', 'xhigh'] };
  if (/^gpt-5\.\d+/.test(model))                     return { type: 'effort', levels: ['low', 'medium', 'high'] };
  if (/^(gpt-5|chatgpt-5)/.test(model))              return { type: 'effort', levels: ['minimal', 'low', 'medium', 'high'] };
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

  // Always-visible compact status line — context %, turns, last-request tokens.
  // Full breakdown is in the progressWrapper tooltip; this is the at-a-glance
  // version so the user doesn't have to expand the header to see usage.
  const statusLine = el('div', { class: 'chat-header-status' });
  headerRight.appendChild(statusLine);

  // Price box with border-as-progress (conic-gradient approach)
  const progressWrapper = el('div', { class: 'chat-header-progress', title: 'Context window used' });
  const progressInner = el('div', { class: 'chat-header-progress__inner' });
  const progressCostLabel = el('span', { class: 'chat-header-progress__label' });
  progressInner.appendChild(progressCostLabel);
  progressWrapper.appendChild(progressInner);
  headerRight.appendChild(progressWrapper);

  // Close chat — clears the active task so the chat view returns to the
  // empty "What would you like to do?" state. The task itself is unchanged
  // and can be reopened from the sidebar history.
  const headerCloseBtn = el('button', { class: 'chat-header-bar__close', title: 'Close chat' });
  headerCloseBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 13));
  headerCloseBtn.addEventListener('click', (e) => {
    e.stopPropagation(); // don't toggle the expand/collapse header
    setActiveTask(null);
  });
  headerRight.appendChild(headerCloseBtn);

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
    if (!taskId) { progressCostLabel.textContent = ''; headerStatsRow.innerHTML = ''; statusLine.textContent = ''; return; }
    const task = agentStore.getState('tasks')[taskId];
    const cost = task?.cost;
    if (!cost) { progressCostLabel.textContent = ''; headerStatsRow.innerHTML = ''; statusLine.textContent = ''; return; }

    // Aggregate subagent costs into the totals (still used for $ and tooltip).
    const sub = getSubagentCostTotals(taskId);
    const totalInput = (cost.total_input_tokens || 0) + sub.inputTokens;
    const totalOutput = (cost.total_output_tokens || 0) + sub.outputTokens;
    const usd = (cost.estimated_cost_usd || 0) + sub.usd;
    const cacheRead = (cost.total_cache_read_tokens || 0) + sub.cacheTokens;

    // Headline numbers are CUMULATIVE across the whole task so intermediate
    // assistant messages never make the display "reset". Per-user-turn and
    // per-request numbers live elsewhere (pill under each user bubble,
    // context bar for the last request).
    // NB: totalInput already includes sub.inputTokens (line above) — don't
    // add it again here or sub-agent input gets double-counted in the title.
    const sentTotal = totalInput + (cost.total_cache_read_tokens || 0) + (cost.total_cache_write_tokens || 0);
    const recvTotal = totalOutput;

    const costStr = usd > 0
      ? usd < 0.001 ? '<$0.001' : `$${usd.toFixed(3)}`
      : '$0';

    // Progress bar label = cost
    progressCostLabel.textContent = costStr;

    // Hover tooltip on progress bar — cumulative across the whole task.
    progressWrapper.title = [
      `Total ↑ Sent: ${sentTotal.toLocaleString()} (in=${totalInput.toLocaleString()}, cache_read=${(cost.total_cache_read_tokens || 0).toLocaleString()}, cache_write=${(cost.total_cache_write_tokens || 0).toLocaleString()})`,
      `Total ↓ Received: ${recvTotal.toLocaleString()}`,
      cacheRead > 0 ? `Cache read: ${cacheRead.toLocaleString()}` : null,
      `Turns: ${cost.turn_count ?? 0}`,
      sub.usd > 0 ? `Sub-agent cost: $${sub.usd.toFixed(4)}` : null,
      `Est. cost: $${usd.toFixed(4)}`,
    ].filter(Boolean).join('\n');

    // Compact always-visible status line:  42% ctx  ·  23 turns  ·  ↑300 ↓120
    const ctxPctText = statusLine.dataset.ctxPct || '';
    const turnsText = `${cost.turn_count ?? 0} turn${(cost.turn_count ?? 0) === 1 ? '' : 's'}`;
    const hasTotals = sentTotal || recvTotal;
    statusLine.innerHTML = '';
    const sep = () => el('span', { class: 'status-line__sep' }, '  ·  ');
    if (ctxPctText) statusLine.appendChild(el('span', { class: 'status-line__ctx' }, ctxPctText));
    if (ctxPctText) statusLine.appendChild(sep());
    statusLine.appendChild(el('span', { class: 'status-line__turns' }, turnsText));
    if (hasTotals) {
      statusLine.appendChild(sep());
      statusLine.appendChild(el('span', { class: 'status-line__sent' }, `↑${formatTokens(sentTotal)}`));
      statusLine.appendChild(el('span', { class: 'status-line__gap' }, ' '));
      statusLine.appendChild(el('span', { class: 'status-line__recv' }, `↓${formatTokens(recvTotal)}`));
    }

    // Expanded stats row — cumulative totals for the whole task.
    headerStatsRow.innerHTML = '';
    const statsItems = [
      { icon: '↑', value: formatTokens(sentTotal), cls: 'sent' },
      { icon: '↓', value: formatTokens(recvTotal), cls: 'recv' },
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

  // Changed-files panel (above input, expands upward)
  const changedFilesPanel = el('div', { class: 'chat-changed-files' });

  // Input area
  const inputArea = el('div', { class: 'chat-input-area' });
  const textarea = el('textarea', {
    class: 'chat-input',
    placeholder: 'Send a message...',
  });

  function autoResizeTextarea() {
    textarea.style.height = 'auto';
    textarea.style.height = textarea.scrollHeight + 'px';
  }

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
    if (!taskId) {
      // On the welcome screen, show the pending choice so the button label
      // reflects what the next new chat will use.
      return agentStore.getState('pendingModelChoice')?.modelId || '';
    }
    const task = agentStore.getState('tasks')[taskId];
    return task?.model || task?.info?.model || '';
  }

  function getCurrentProviderType() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      return agentStore.getState('pendingModelChoice')?.providerId || '';
    }
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

    // Before opening the dropdown, refresh the persisted model lists so newly-
    // released models show up without requiring a trip to settings. Backend
    // holds a 5-min TTL cache so this is a no-op after the first call.
    console.log('[chat-view model dropdown] calling refreshAllProviderModels...');
    try {
      const changed = await refreshAllProviderModels(true); // force-refresh in debug mode
      console.log('[chat-view model dropdown] refresh returned, changed:', Array.from(changed || []));
    } catch (e) {
      console.warn('[chat-view model dropdown] refresh threw:', e);
    }

    // Build model list from locally-cached provider configs (all connected providers)
    const configs = loadProviderConfigs();
    console.log('[chat-view model dropdown] configs after refresh:', Object.fromEntries(
      Object.entries(configs).map(([k, v]) => [k, { model: v.model, modelCount: v.models?.length || 0, models: v.models }])
    ));
    const providerEntries = Object.entries(configs)
      .filter(([, cfg]) => cfg.hasKey && cfg.models?.length);

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
        const groupLabel = providerId.startsWith('Compatible:')
          ? `OpenAI-Compatible — ${cfg.name || providerId.slice('Compatible:'.length)}`
          : providerId;
        const groupHeader = el('div', { class: 'chat-model-dropdown__group' }, groupLabel);
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
              if (!(await pickModel(providerId, modelId))) return;
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
          try {
            if (!(await pickModel(provider.provider_type, modelId))) return;
            await api.switchModel(taskId, provider.provider_type, modelId);
          } catch {}
        });
        modelDropdown.appendChild(item);
      }
    }

    const rect = modelBtn.getBoundingClientRect();
    const availableHeight = Math.max(160, rect.top - 12);
    modelDropdown.style.cssText =
      `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;left:${rect.left}px;`
      + `max-height:${availableHeight}px;`;
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
    if (!taskId) {
      // Welcome screen: reflect the preselected mode so the popover shows
      // the active level and Send applies it to the new task.
      return agentStore.getState('pendingPermissionLevel') || 'ManualEdit';
    }
    const task = agentStore.getState('tasks')[taskId];
    return task?.permissionLevel || 'ManualEdit';
  }

  function getCurrentSensitiveAccess() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return !!agentStore.getState('pendingSensitiveAccess');
    const task = agentStore.getState('tasks')[taskId];
    return task?.sensitiveAccess === true;
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
    const current   = getCurrentMode();
    const taskObj   = agentStore.getState('tasks')[taskId];
    const sensOn    = taskObj?.sensitiveAccess === true;
    const inEdit    = current === 'ManualEdit' || current === 'AutoEdit';
    const autoOn    = current === 'AutoEdit';
    const inFull    = current === 'FullAuto';

    function makeInlineToggle(on, onClick) {
      const btn = el('button', { class: `chat-call-config-toggle${on ? ' chat-call-config-toggle--on' : ''}` });
      btn.appendChild(el('span', { class: 'chat-call-config-toggle__thumb' }));
      btn.addEventListener('click', (ev) => { ev.stopPropagation(); onClick(); });
      return btn;
    }

    // ── Chat ──
    const chatItem = el('div', { class: `chat-mode-dropdown__item${current === 'Chat' ? ' chat-mode-dropdown__item--active' : ''}` });
    const chatDot  = el('span', { class: 'chat-mode-pill__dot chat-mode-pill__dot--chat' });
    chatItem.appendChild(chatDot);
    chatItem.appendChild(el('span', { class: 'chat-mode-dropdown__label-text' }, 'Chat'));
    chatItem.addEventListener('click', async (ev) => {
      ev.stopPropagation(); closeModeDropdown();
      const ok = await setTaskPermissions(taskId, 'Chat');
      if (ok) updateModePill();
    });
    modeDropdown.appendChild(chatItem);

    function makePillInfoBtn(tooltip) {
      const btn = el('button', { class: 'chat-call-config-info', 'data-tip': tooltip });
      btn.appendChild(iconMulti([
        'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z',
        'M12 16v-4M12 8h.01',
      ], 13));
      btn.addEventListener('click', (ev) => ev.stopPropagation());
      return btn;
    }

    // ── Edit ──
    const editItem = el('div', { class: `chat-mode-dropdown__item${inEdit ? ' chat-mode-dropdown__item--active' : ''}` });
    const editLeft = el('span', { class: 'chat-mode-dropdown__item-left' });
    editLeft.appendChild(el('span', { class: `chat-mode-pill__dot chat-mode-pill__dot--${autoOn ? 'autoedit' : 'manualedit'}` }));
    editLeft.appendChild(el('span', { class: 'chat-mode-dropdown__label-text' }, 'Edit'));
    editLeft.appendChild(makePillInfoBtn(autoOn
      ? 'Auto Edit — writes applied automatically; commands still need approval'
      : 'Manual Edit — every file write and command requires your approval'));
    editItem.appendChild(editLeft);
    editItem.appendChild(makeInlineToggle(autoOn, async () => {
      const ok = await setTaskPermissions(taskId, autoOn ? 'ManualEdit' : 'AutoEdit');
      if (ok) { updateModePill(); closeModeDropdown(); }
    }));
    editItem.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (inEdit) return;
      closeModeDropdown();
      const ok = await setTaskPermissions(taskId, 'ManualEdit');
      if (ok) updateModePill();
    });
    modeDropdown.appendChild(editItem);

    // ── Full Auto ──
    const fullItem = el('div', { class: `chat-mode-dropdown__item${inFull ? ' chat-mode-dropdown__item--active' : ''}` });
    const fullLeft = el('span', { class: 'chat-mode-dropdown__item-left' });
    fullLeft.appendChild(el('span', { class: 'chat-mode-pill__dot chat-mode-pill__dot--fullauto' }));
    fullLeft.appendChild(el('span', { class: 'chat-mode-dropdown__label-text' }, 'Full Auto'));
    fullLeft.appendChild(makePillInfoBtn(sensOn && inFull
      ? 'Full Auto · Sensitive — all files including .env and credentials are accessible'
      : 'Full Auto — everything runs without approval; sensitive files still require confirmation'));
    fullItem.appendChild(fullLeft);
    fullItem.appendChild(makeInlineToggle(sensOn && inFull, async () => {
      if (!inFull) {
        await setTaskPermissions(taskId, 'FullAuto');
        await setTaskSensitiveAccess(taskId, true);
      } else {
        await setTaskSensitiveAccess(taskId, !sensOn);
      }
      updateModePill(); closeModeDropdown();
    }));
    fullItem.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (inFull) return;
      closeModeDropdown();
      const ok = await setTaskPermissions(taskId, 'FullAuto');
      if (ok) updateModePill();
    });
    modeDropdown.appendChild(fullItem);

    const rect = modePill.getBoundingClientRect();
    const availableHeight = Math.max(160, rect.top - 12);
    modeDropdown.style.cssText =
      `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;right:${window.innerWidth - rect.right}px;`
      + `max-height:${availableHeight}px;overflow-y:auto;`;
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
    const isWaiting = task?.status === 'WaitingForInput';
    // Update textarea placeholder based on state
    textarea.placeholder = isWaiting ? 'Type your response...' : 'Send a message...';
    if (isRunning === sendBtnIsStop) return;
    sendBtnIsStop = isRunning;
    sendBtn.innerHTML = '';
    if (isRunning) {
      sendBtn.classList.add('chat-send-btn--stop');
      sendBtn.title = 'Stop task';
      const ns = 'http://www.w3.org/2000/svg';
      const svg = document.createElementNS(ns, 'svg');
      svg.setAttribute('width', '16');
      svg.setAttribute('height', '16');
      svg.setAttribute('viewBox', '0 0 24 24');
      svg.setAttribute('fill', 'none');
      const ring = document.createElementNS(ns, 'circle');
      ring.setAttribute('cx', '12');
      ring.setAttribute('cy', '12');
      ring.setAttribute('r', '9');
      ring.setAttribute('stroke', 'currentColor');
      ring.setAttribute('stroke-width', '2.5');
      ring.setAttribute('stroke-linecap', 'round');
      ring.setAttribute('stroke-dasharray', '42 14');
      ring.setAttribute('class', 'stop-ring');
      const rect = document.createElementNS(ns, 'rect');
      rect.setAttribute('x', '8');
      rect.setAttribute('y', '8');
      rect.setAttribute('width', '8');
      rect.setAttribute('height', '8');
      rect.setAttribute('rx', '1');
      rect.setAttribute('fill', 'currentColor');
      svg.appendChild(ring);
      svg.appendChild(rect);
      sendBtn.appendChild(svg);
    } else {
      sendBtn.classList.remove('chat-send-btn--stop');
      sendBtn.title = 'Send';
      sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 15));
    }
  }

  function getContextWindow(model) {
    if (!model) return 200000;
    // Claude [1m] variant suffix (e.g. claude-opus-4-7[1m]) = 1M-context mode.
    // Check this before the plain "claude" branch so we don't cap at 200K.
    if (model.includes('claude') && /\[1m\]/i.test(model)) return 1000000;
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
    // Context usage = size of the NEXT provider request, not the cumulative
    // tokens spent across the whole task. The numerator must be tokens
    // currently "in context" (input + cache reads/writes) for the last
    // request — summing turn-after-turn totals produces a meaningless ratio
    // that saturates at 100% after a handful of turns.
    const last = (agentStore.getState('lastRequestUsage') || {})[taskId];
    if (!last || !(last.input || last.cacheRead || last.cacheWrite)) {
      progressWrapper.style.setProperty('--progress', '0');
      progressWrapper.classList.remove('chat-header-progress--warn', 'chat-header-progress--high');
      // Clear previous ctx text so a just-started task doesn't inherit stale %.
      delete statusLine.dataset.ctxPct;
      return;
    }
    const used = (last.input || 0) + (last.cacheRead || 0) + (last.cacheWrite || 0);
    const max = getContextWindow(getCurrentModel());
    const pct = Math.min(100, (used / max) * 100);
    progressWrapper.style.setProperty('--progress', `${pct}`);
    // Publish context % to the status line. updateCostDisplay reads this
    // from the dataset on its next pass.
    statusLine.dataset.ctxPct = `${Math.round(pct)}% ctx`;
    // Refresh the status line text now so the % updates even when only
    // context changes (e.g. model switch) without a cost update.
    updateCostDisplay();

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

  // Attached chips — inserted via the slash/at picker and expanded into the
  // final message body on send. For files/terminals the chip only carries a
  // reference (path or session_id/pid); the agent reads content via its own
  // tools if it needs to. Keeps outbound context clean.
  //   { type: 'skill'|'workflow'|'mcp', name, body? }
  //   { type: 'file',     name, path }
  //   { type: 'terminal', name, sessionId, pid, label, cwd }
  let attachedTags = [];

  // Picker state — handles both `/` (skills/workflows/mcp) and `@` (files/terminals).
  let slashPickerItems = [];    // all loaded items for the active trigger
  let slashPickerFiltered = []; // filtered by current query
  let slashPickerIndex = 0;     // keyboard-selected index
  let slashPickerOpen = false;
  let slashPickerTrigger = '/'; // '/' or '@' — which character opened the picker
  // Cache of `@` file lists keyed by project root path (string → string[]).
  // Invalidated on-demand when the user triggers `@` in a different project.
  const mentionFilesCache = new Map();

  // Attachment pills container (above textarea)
  const attachmentPills = el('div', { class: 'chat-attachments' });
  attachmentPills.style.display = 'none';

  // Skill / workflow / mcp tag chips (above textarea, below attachments)
  const tagChips = el('div', { class: 'chat-tags' });
  tagChips.style.display = 'none';

  function tagIconPath(type) {
    // Small distinguishing icons for each type
    if (type === 'skill')    return 'M13 10V3L4 14h7v7l9-11h-7z';
    if (type === 'workflow') return 'M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM15 6h-3a6 6 0 0 0-6 6v3';
    if (type === 'file')     return 'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM13 2v7h7';
    if (type === 'terminal') return 'M4 17l6-6-6-6M12 19h8';
    return 'M5 12H3m16 0h-2M12 5V3m0 16v-2m-4.95-1.05-1.414 1.414M18.364 5.636l-1.414 1.414M18.364 18.364l-1.414-1.414M6.05 6.05 4.636 4.636M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z';
  }

  function renderTagChips() {
    tagChips.innerHTML = '';
    if (attachedTags.length === 0) {
      tagChips.style.display = 'none';
      return;
    }
    tagChips.style.display = 'flex';
    for (let i = 0; i < attachedTags.length; i++) {
      const t = attachedTags[i];
      // File chips display the basename but hover-title shows the full path.
      // Terminal chips display "label [pid]" and hover shows cwd.
      let displayName = t.name;
      let titleText = t.description || t.name;
      if (t.type === 'file' && t.path) {
        const parts = t.path.split('/');
        displayName = parts[parts.length - 1] || t.path;
        titleText = t.path;
      } else if (t.type === 'terminal') {
        displayName = t.pid != null ? `${t.label} [${t.pid}]` : t.label;
        titleText = t.cwd ? `${displayName} — ${t.cwd}` : displayName;
      }
      const chip = el('div', { class: `chat-tag chat-tag--${t.type}`, title: titleText });
      chip.appendChild(icon(tagIconPath(t.type), 12));
      chip.appendChild(el('span', { class: 'chat-tag__name' }, displayName));
      const removeBtn = el('button', { class: 'chat-tag__remove', title: 'Remove' }, '×');
      const idx = i;
      removeBtn.addEventListener('click', () => {
        attachedTags.splice(idx, 1);
        renderTagChips();
      });
      chip.appendChild(removeBtn);
      tagChips.appendChild(chip);
    }
  }

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
      const isImage = f.base64 && f.type.startsWith('image/');
      const src = isImage ? `data:${f.type};base64,${f.base64}` : null;
      if (isImage) {
        pill.appendChild(el('img', { class: 'chat-attachment-pill__thumb', src }));
      }
      pill.appendChild(el('span', { class: 'chat-attachment-pill__name' }, f.name));
      const removeBtn = el('button', { class: 'chat-attachment-pill__remove' }, '×');
      const idx = i;
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        attachedFiles.splice(idx, 1);
        renderAttachmentPills();
      });
      pill.appendChild(removeBtn);
      if (isImage) {
        pill.addEventListener('click', () => openImageLightbox(src));
      } else {
        pill.style.cursor = 'default';
      }
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

  // Agent Configuration button — brain icon, opens popover with model, permissions, thinking effort
  const callConfigBtn = el('button', { class: 'chat-think-btn', title: 'Agent configuration' });
  callConfigBtn.appendChild(iconMulti([
    'M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.46 2.5 2.5 0 0 1-1.04-1.54A2.5 2.5 0 0 1 4 15.5a2.5 2.5 0 0 1 0-7 2.5 2.5 0 0 1 1-2A2.5 2.5 0 0 1 9.5 2Z',
    'M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.46 2.5 2.5 0 0 0 1.04-1.54A2.5 2.5 0 0 0 20 15.5a2.5 2.5 0 0 0 0-7 2.5 2.5 0 0 0-1-2A2.5 2.5 0 0 0 14.5 2Z',
  ], 14));

  // Project picker — inline pill in the input toolbar. Shows the current
  // project name; clicking (when no task is active) opens a small list
  // popover with all projects + Global. When a task is active it's
  // read-only since a chat's project is fixed for its lifetime.
  const projectBtn = el('button', { class: 'chat-project-pill', title: 'Project' });
  const projectBtnLabel = el('span', { class: 'chat-project-pill__label' });
  const projectBtnChevron = el('span', { class: 'chat-project-pill__chevron' });
  projectBtnChevron.appendChild(icon('M6 9l6 6 6-6', 12));
  projectBtn.appendChild(projectBtnLabel);
  projectBtn.appendChild(projectBtnChevron);

  let projectPickerPopover = null;
  function closeProjectPicker() {
    if (projectPickerPopover) { projectPickerPopover.remove(); projectPickerPopover = null; }
  }
  function openProjectPicker() {
    closeProjectPicker();
    const pop = el('div', { class: 'chat-project-picker' });
    const currentId = getCurrentProjectId();
    const projects = workspaceStore.getState('projects');

    const globalItem = el('div', {
      class: `chat-project-picker__item${currentId === GLOBAL_PROJECT_ID ? ' chat-project-picker__item--active' : ''}`,
    });
    globalItem.textContent = 'Global';
    globalItem.title = 'Orchestrator: read across all projects, spawn sub-tasks.';
    globalItem.addEventListener('click', (ev) => {
      ev.stopPropagation();
      setPendingProjectId(GLOBAL_PROJECT_ID);
      closeProjectPicker();
    });
    pop.appendChild(globalItem);

    if (projects.length > 0) {
      pop.appendChild(el('div', { class: 'chat-project-picker__group' }, 'Projects'));
      for (const project of projects) {
        const item = el('div', {
          class: `chat-project-picker__item${String(currentId) === String(project.id) ? ' chat-project-picker__item--active' : ''}`,
        });
        item.textContent = project.name;
        item.title = project.root_path || project.name;
        item.addEventListener('click', (ev) => {
          ev.stopPropagation();
          setPendingProjectId(project.id);
          closeProjectPicker();
        });
        pop.appendChild(item);
      }
    }

    const rect = projectBtn.getBoundingClientRect();
    pop.style.cssText = `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;left:${rect.left}px;`;
    document.body.appendChild(pop);
    projectPickerPopover = pop;
  }
  projectBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (projectPickerPopover) { closeProjectPicker(); return; }
    if (agentStore.getState('activeTaskId')) return; // read-only when a task is active
    openProjectPicker();
  });
  document.addEventListener('click', closeProjectPicker);

  function updateProjectBtn() {
    const currentId = getCurrentProjectId();
    projectBtnLabel.textContent = projectLabelFor(currentId);
    const readonly = !!agentStore.getState('activeTaskId');
    projectBtn.classList.toggle('chat-project-pill--readonly', readonly);
  }

  let callConfigOpen = false;
  let callConfigPopover = null;
  let callConfigModelListOpen = false;

  function closeCallConfig() {
    if (callConfigPopover) { callConfigPopover.remove(); callConfigPopover = null; callConfigOpen = false; }
    callConfigModelListOpen = false;
  }

  // Return the label shown for a project id, including the Global pseudo-project.
  function projectLabelFor(projectId) {
    if (!projectId) return 'Select project';
    if (projectId === GLOBAL_PROJECT_ID) return 'Global';
    const projects = workspaceStore.getState('projects');
    const p = projects.find(pr => String(pr.id) === String(projectId));
    return p?.name || 'Select project';
  }

  // Resolve the "current" project scope for the popover: the active task's
  // project wins when one exists, otherwise the welcome-screen pending pick.
  function getCurrentProjectId() {
    const taskId = agentStore.getState('activeTaskId');
    const task = taskId ? agentStore.getState('tasks')[taskId] : null;
    if (task) return task.project_id || task.projectId || null;
    return agentStore.getState('pendingProjectId');
  }

  function rebuildCallConfigContent() {
    if (!callConfigPopover) return;
    callConfigPopover.innerHTML = '';

    const taskId = agentStore.getState('activeTaskId');

    // ── Model selector row (collapsible) ─────────────────
    // Project lives in its own toolbar pill now — keep this popover
    // focused on model + permissions + thinking effort.
    const currentModel = getCurrentModel();
    const modelHeader = el('div', { class: 'chat-call-config-model-header' });
    const modelHeaderLeft = el('span', { class: 'chat-call-config-model-header__left' });
    modelHeaderLeft.appendChild(el('span', { class: 'chat-call-config-model-header__label' }, 'Model'));
    modelHeaderLeft.appendChild(el('span', { class: 'chat-call-config-model-header__value' }, currentModel || 'Select model'));
    modelHeader.appendChild(modelHeaderLeft);
    const chevron = icon(callConfigModelListOpen ? 'M5 15l7-7 7 7' : 'M19 9l-7 7-7-7', 12);
    modelHeader.appendChild(chevron);
    modelHeader.addEventListener('click', (ev) => {
      ev.stopPropagation();
      callConfigModelListOpen = !callConfigModelListOpen;
      rebuildCallConfigContent();
    });
    callConfigPopover.appendChild(modelHeader);

    if (callConfigModelListOpen) {
      const list = el('div', { class: 'chat-call-config-model-list' });
      const configs = loadProviderConfigs();
      const providerEntries = Object.entries(configs)
        .filter(([, cfg]) => cfg.hasKey && cfg.models?.length);

      if (providerEntries.length > 0) {
        for (const [providerId, cfg] of providerEntries) {
          const groupLabel = providerId.startsWith('Compatible:')
            ? `OpenAI-Compatible — ${cfg.name || providerId.slice('Compatible:'.length)}`
            : providerId;
          list.appendChild(el('div', { class: 'chat-call-config-model-list__group' }, groupLabel));
          for (const modelId of cfg.models) {
            const item = el('div', {
              class: `chat-call-config-model-list__item${modelId === currentModel ? ' chat-call-config-model-list__item--active' : ''}`,
            });
            item.textContent = modelId;
            item.title = modelId;
            item.addEventListener('click', async (ev) => {
              ev.stopPropagation();
              if (!(await pickModel(providerId, modelId))) return;
              if (taskId) {
                try {
                  saveThinkingForModel(currentModel);
                  await api.switchModel(taskId, providerId, modelId);
                  restoreThinkingForModel(modelId);
                } catch (err) {
                  console.error('Failed to switch model:', err);
                }
              } else {
                // Welcome screen: no task to switch — store the pick and
                // apply it after createTask in the send handler.
                setPendingModelChoice({ providerId, modelId });
              }
              callConfigModelListOpen = false;
              updateCallConfigBtn();
              rebuildCallConfigContent();
            });
            list.appendChild(item);
          }
        }
      } else if (aiConfig?.providers?.length) {
        for (const provider of aiConfig.providers.filter((p) => p.enabled)) {
          list.appendChild(el('div', { class: 'chat-call-config-model-list__group' }, provider.provider_type));
          const modelId = provider.default_model;
          if (!modelId) continue;
          const item = el('div', {
            class: `chat-call-config-model-list__item${modelId === currentModel ? ' chat-call-config-model-list__item--active' : ''}`,
          });
          item.textContent = modelId;
          item.title = modelId;
          item.addEventListener('click', async (ev) => {
            ev.stopPropagation();
            if (!(await pickModel(provider.provider_type, modelId))) return;
            if (taskId) {
              try { await api.switchModel(taskId, provider.provider_type, modelId); } catch {}
            } else {
              setPendingModelChoice({ providerId: provider.provider_type, modelId });
            }
            callConfigModelListOpen = false;
            updateCallConfigBtn();
            rebuildCallConfigContent();
          });
          list.appendChild(item);
        }
      } else {
        list.appendChild(el('div', { class: 'chat-call-config-model-list__empty' }, 'No providers configured'));
      }
      callConfigPopover.appendChild(list);
    }

    callConfigPopover.appendChild(el('div', { class: 'chat-call-config-divider' }));

    // ── Permission modes — 3 rows: Chat, Edit (Manual↔Auto), Full Auto (+Sensitive) ──
    const current = getCurrentMode();
    const sensitiveOn    = getCurrentSensitiveAccess();
    const editGroupActive = current === 'ManualEdit' || current === 'AutoEdit';
    const autoEditOn      = current === 'AutoEdit';
    const fullAutoActive  = current === 'FullAuto';

    // On the welcome screen there is no task yet — store the choice on
    // agentStore; the send handler applies it right after createTask.
    // With an active task, route through the task-scoped APIs as usual.
    async function applyPermissionLevel(level) {
      if (!taskId) { setPendingPermissionLevel(level); return true; }
      return await setTaskPermissions(taskId, level);
    }
    async function applySensitive(allowed) {
      if (!taskId) { setPendingSensitiveAccess(allowed); return true; }
      return await setTaskSensitiveAccess(taskId, allowed);
    }

    function makeToggle(on, onClick) {
      const btn = el('button', { class: `chat-call-config-toggle${on ? ' chat-call-config-toggle--on' : ''}` });
      btn.appendChild(el('span', { class: 'chat-call-config-toggle__thumb' }));
      btn.addEventListener('click', (ev) => { ev.stopPropagation(); onClick(); });
      return btn;
    }

    // Proper circle-info SVG icon (Lucide style)
    function makeInfoBtn(tooltip) {
      const btn = el('button', { class: 'chat-call-config-info', 'data-tip': tooltip });
      btn.appendChild(iconMulti([
        'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z',
        'M12 16v-4M12 8h.01',
      ], 13));
      btn.addEventListener('click', (ev) => ev.stopPropagation());
      return btn;
    }

    // Row layout: [mode-icon] [label + info-btn (flex:1)] [toggle]
    function makeRow(iconPath, label, isActive, infoTip, toggleEl) {
      const row = el('div', { class: `chat-call-config-item${isActive ? ' chat-call-config-item--active' : ''}` });
      const ic = el('span', { class: 'chat-call-config-item__icon' });
      ic.appendChild(icon(iconPath, 14));
      row.appendChild(ic);
      // Left group: label + info icon, takes all available space
      const left = el('span', { class: 'chat-call-config-item__left' });
      left.appendChild(el('span', { class: 'chat-call-config-item__title' }, label));
      if (infoTip) left.appendChild(makeInfoBtn(infoTip));
      row.appendChild(left);
      // Toggle on the far right
      if (toggleEl) row.appendChild(toggleEl);
      return row;
    }

    // ── Chat ──
    const chatRow = makeRow(
      'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z',
      'Chat', current === 'Chat', null, null,
    );
    chatRow.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (current === 'Chat') return;
      const ok = await applyPermissionLevel('Chat');
      if (ok) { updateCallConfigBtn(); rebuildCallConfigContent(); }
    });
    callConfigPopover.appendChild(chatRow);

    // ── Edit (Manual / Auto) ──
    const editTip = autoEditOn
      ? 'Auto Edit — writes applied automatically; commands still need approval'
      : 'Manual Edit — every file write and command requires your approval';
    const editRow = makeRow(
      'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z',
      'Edit', editGroupActive, editTip,
      makeToggle(autoEditOn, async () => {
        const ok = await applyPermissionLevel(autoEditOn ? 'ManualEdit' : 'AutoEdit');
        if (ok) { updateCallConfigBtn(); rebuildCallConfigContent(); }
      }),
    );
    editRow.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (editGroupActive) return;
      const ok = await applyPermissionLevel('ManualEdit');
      if (ok) { updateCallConfigBtn(); rebuildCallConfigContent(); }
    });
    callConfigPopover.appendChild(editRow);

    // ── Full Auto (+Sensitive) ──
    const fullTip = sensitiveOn && fullAutoActive
      ? 'Full Auto · Sensitive — all files including .env and credentials are accessible'
      : 'Full Auto — everything runs without approval; sensitive files still require confirmation';
    const fullAutoRow = makeRow(
      'M13 10V3L4 14h7v7l9-11h-7z',
      'Full Auto', fullAutoActive, fullTip,
      makeToggle(sensitiveOn && fullAutoActive, async () => {
        if (!fullAutoActive) {
          const ok = await applyPermissionLevel('FullAuto');
          if (!ok) return;
          await applySensitive(true);
        } else {
          await applySensitive(!sensitiveOn);
        }
        updateCallConfigBtn();
        rebuildCallConfigContent();
      }),
    );
    fullAutoRow.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (fullAutoActive) return;
      const ok = await applyPermissionLevel('FullAuto');
      if (ok) { updateCallConfigBtn(); rebuildCallConfigContent(); }
    });
    callConfigPopover.appendChild(fullAutoRow);

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
          // Always persist so the last choice survives app restarts —
          // thinking effort isn't stored per-task in the DB (unlike model
          // and permission), so without this the client state is lost
          // whenever you close the app.
          setPendingThinking({
            enabled: thinkingEnabled,
            effort: thinkingEffort,
            budget: thinkingBudget,
          });
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
        // Always persist (same reasoning as the effort-button path above).
        setPendingThinking({
          enabled: thinkingEnabled,
          effort: thinkingEffort,
          budget: thinkingBudget,
        });
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
    // Cap height to the space available above the trigger so the popover
    // can't spill off the top of the viewport. The inner model list still
    // has its own max-height/scroll; this is the outer safety net for when
    // header + mode toggles + effort row push the total past what fits.
    const availableHeight = Math.max(200, rect.top - 12);
    callConfigPopover.style.cssText =
      `position:fixed;bottom:${window.innerHeight - rect.top + 4}px;right:${window.innerWidth - rect.right}px;`
      + `max-height:${availableHeight}px;overflow-y:auto;`;
    document.body.appendChild(callConfigPopover);

    // Refresh persisted model lists in the background so newly-released models
    // appear without forcing the user to re-enter their API key. Once done,
    // re-render the popover if it's still open.
    console.log('[callConfig] opened — kicking off refreshAllProviderModels(force=true)');
    refreshAllProviderModels(true).then((changed) => {
      console.log('[callConfig] refresh returned, changed keys:', Array.from(changed || []));
      if (!callConfigOpen) return;
      if (changed && changed.size > 0) {
        console.log('[callConfig] re-rendering popover with fresh models');
        rebuildCallConfigContent();
      }
    }).catch((e) => {
      console.warn('[callConfig] refresh threw:', e);
    });
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

  // Thinking state — seeded from the persisted welcome-screen choice so
  // it survives app restarts. The agent-config popover mutates these
  // directly and also pushes to `agentStore.pendingThinking` when no
  // task is active so the next new chat starts on the same effort.
  const _persistedThinking = agentStore.getState('pendingThinking');
  let thinkingEnabled = _persistedThinking?.enabled ?? false;
  let thinkingEffort = _persistedThinking?.effort || 'medium';
  let thinkingBudget = _persistedThinking?.budget ?? 8000;

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

  // Apply project defaults for thinking effort when a new task is created
  let appliedDefaultsForTask = null; // track which task we already applied defaults to
  function applyProjectDefaults() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId || taskId === appliedDefaultsForTask) return;
    const task = agentStore.getState('tasks')[taskId];
    if (!task?.projectDefaults) return;
    appliedDefaultsForTask = taskId;

    const effort = task.projectDefaults.thinking_effort;
    if (effort && effort !== 'off') {
      const cap = getThinkingCapability(getCurrentModel());
      if (cap) {
        thinkingEnabled = true;
        if (cap.type === 'effort') {
          thinkingEffort = effort;
        } else if (cap.type === 'budget') {
          const budgetMap = { low: 2000, medium: 10000, high: 20000, max: 32000 };
          thinkingBudget = budgetMap[effort] || 10000;
        }
        updateThinkBtn();
        updateCallConfigBtn();
      }
    } else if (effort === 'off') {
      thinkingEnabled = false;
      updateThinkBtn();
      updateCallConfigBtn();
    }
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

  // Resolve the project root for the currently-active task. Used as the cache
  // key and root for the `@` file walker. Returns null if we can't figure it
  // out — in that case the `@` picker just won't list any files.
  function getActiveProjectRoot() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return null;
    const task = agentStore.getState('tasks')[taskId];
    if (!task) return null;
    const pid = task.project_id || task.projectId;
    if (pid == null) return null;
    const projects = workspaceStore.getState('projects') || [];
    const project = projects.find((p) => String(p.id) === String(pid));
    return project?.root_path || null;
  }

  async function loadSlashCommands() {
    const results = [];
    try {
      const skills = await api.listSkills();
      for (const s of (skills || [])) {
        results.push({ type: 'skill', name: s.name, description: s.description });
      }
    } catch {}
    try {
      const workflows = await api.listWorkflows();
      for (const w of (workflows || [])) {
        results.push({ type: 'workflow', name: w.name, description: w.description });
      }
    } catch {}
    try {
      const servers = await api.listMcpServers();
      for (const s of (servers || [])) {
        results.push({ type: 'mcp', name: s.name, description: s.description || `MCP: ${s.name}` });
      }
    } catch {}
    return results;
  }

  async function loadMentionItems() {
    const results = [];

    // Terminals first — they're live and usually the shorter list. Includes
    // both user-opened and agent-spawned sessions so you can reference any.
    const sessions = terminalStore.getState('sessions') || [];
    for (const s of sessions) {
      const pidPart = s.pid != null ? ` [${s.pid}]` : '';
      results.push({
        type: 'terminal',
        name: `${s.label}${pidPart}`,
        description: s.cwd || '',
        sessionId: s.id,
        pid: s.pid ?? null,
        label: s.label,
        cwd: s.cwd || '',
      });
    }

    // Files for the active project, cached per-root so repeated opens are fast.
    const root = getActiveProjectRoot();
    if (root) {
      let files = mentionFilesCache.get(root);
      if (!files) {
        try {
          files = await api.listProjectFiles(root, 5000);
        } catch {
          files = [];
        }
        mentionFilesCache.set(root, files || []);
      }
      for (const path of files) {
        // `name` is the basename (what the user likely types); `path` is the
        // full relative path used for disambiguation and the final reference.
        const parts = path.split('/');
        const base = parts[parts.length - 1] || path;
        results.push({ type: 'file', name: base, description: path, path });
      }
    }

    return results;
  }

  async function loadSlashItems(trigger) {
    slashPickerItems = trigger === '@' ? await loadMentionItems() : await loadSlashCommands();
  }

  function getSlashContext(ta) {
    const value = ta.value;
    const cursor = ta.selectionStart;
    const before = value.slice(0, cursor);
    // Match `/` or `@` at position 0 or after whitespace/newline.
    const match = before.match(/(^|\s)([/@])(\S*)$/);
    if (!match) return null;
    const token = match[2] + match[3];
    const slashStart = before.length - token.length;
    return { slashStart, slashEnd: cursor, query: match[3], trigger: match[2] };
  }

  function filterSlashItems(query) {
    if (!query) return slashPickerItems.slice(0, 12);
    const q = query.toLowerCase();
    // Files match on both basename and full path; terminals/commands match name.
    const scored = [];
    for (const item of slashPickerItems) {
      const name = item.name.toLowerCase();
      const path = (item.path || '').toLowerCase();
      const namePfx = name.startsWith(q) ? 0 : 1;
      const pathPfx = path && path.startsWith(q) ? 0 : 2;
      if (name.includes(q)) {
        scored.push({ item, rank: namePfx });
      } else if (path && path.includes(q)) {
        scored.push({ item, rank: pathPfx });
      }
    }
    scored.sort((a, b) => a.rank - b.rank);
    return scored.slice(0, 12).map(s => s.item);
  }

  function badgeLabel(type) {
    if (type === 'skill') return 'Skill';
    if (type === 'workflow') return 'Workflow';
    if (type === 'mcp') return 'MCP';
    if (type === 'file') return 'File';
    if (type === 'terminal') return 'Terminal';
    return type;
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
      typeBadge.textContent = badgeLabel(item.type);
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

    // Strip the "/query" or "@query" token from the textarea — the selection
    // is captured as a compact chip instead of inlined text.
    const value = textarea.value;
    textarea.value = value.slice(0, ctx.slashStart) + value.slice(ctx.slashEnd);
    textarea.selectionStart = textarea.selectionEnd = ctx.slashStart;

    // Dedup — identity differs per type:
    //   file     → path       (two files with the same basename aren't duplicates)
    //   terminal → sessionId  (pid can theoretically recycle; session id is unique per run)
    //   other    → name
    const already = attachedTags.some(t => {
      if (t.type !== item.type) return false;
      if (item.type === 'file') return t.path === item.path;
      if (item.type === 'terminal') return t.sessionId === item.sessionId;
      return t.name === item.name;
    });
    if (already) { textarea.focus(); return; }

    const tag = {
      type: item.type,
      name: item.name,
      description: item.description || '',
    };

    if (item.type === 'file') {
      tag.path = item.path;
    } else if (item.type === 'terminal') {
      tag.sessionId = item.sessionId;
      tag.pid = item.pid;
      tag.label = item.label;
      tag.cwd = item.cwd;
    } else if (item.type === 'workflow') {
      // Fetch the body up front so we can inline it on send.
      try {
        tag.body = await api.getWorkflowBody(item.name);
      } catch {
        tag.body = null;
      }
    }

    attachedTags.push(tag);
    renderTagChips();
    textarea.focus();
  }

  function openSlashPicker(query, trigger) {
    slashPickerOpen = true;
    slashPickerTrigger = trigger || '/';
    slashPickerFiltered = filterSlashItems(query);
    slashPickerIndex = 0;
    renderSlashPicker();
  }

  function closeSlashPicker() {
    slashPickerOpen = false;
    slashPicker.classList.add('slash-picker--hidden');
  }

  sendBtn.addEventListener('click', async () => {
    let taskId = agentStore.getState('activeTaskId');

    if (sendBtnIsStop) {
      if (!taskId) return;
      sendBtn.disabled = true;
      try { await api.abortTask(taskId); } finally { sendBtn.disabled = false; }
      return;
    }

    const text = textarea.value.trim();
    if (!text && attachedFiles.length === 0 && attachedTags.length === 0) return;

    // Welcome-screen send: no active task yet. Auto-create one under the
    // picked project. Global now has its own backing row in the DB so no
    // first-project fallback is needed.
    if (!taskId) {
      const pending = agentStore.getState('pendingProjectId');
      let createArgs;
      if (pending === GLOBAL_PROJECT_ID) {
        // Backend registers a "__global__" project row at startup; the
        // root_path is internal to app data and ignored for orchestrator-
        // only tools, but we still pass a non-empty string so the command
        // signature stays the same.
        createArgs = [GLOBAL_PROJECT_ID, 'Global', '', 'New Global Task'];
      } else {
        const projects = workspaceStore.getState('projects');
        const proj = projects.find(p => String(p.id) === String(pending));
        if (!proj) {
          await showAlertDialog('No project selected', 'Pick a project in Agent configuration first.');
          return;
        }
        createArgs = [proj.id, proj.name, proj.root_path, 'New Task'];
      }
      const info = await createTask(...createArgs);
      if (!info) return;
      taskId = info.id;

      // Apply the welcome-screen model pick (if any) before the first
      // message is dispatched, so this chat starts on the model the user
      // selected rather than the provider default.
      const pendingModel = agentStore.getState('pendingModelChoice');
      if (pendingModel && pendingModel.providerId && pendingModel.modelId) {
        try {
          await api.switchModel(taskId, pendingModel.providerId, pendingModel.modelId);
        } catch (err) {
          console.error('Failed to apply pending model:', err);
        }
      }

      // Apply the welcome-screen permission choice. ManualEdit is the
      // default task permission so only push a different choice.
      const pendingPerm = agentStore.getState('pendingPermissionLevel');
      if (pendingPerm && pendingPerm !== 'ManualEdit') {
        try { await setTaskPermissions(taskId, pendingPerm); } catch {}
      }
      const pendingSens = agentStore.getState('pendingSensitiveAccess');
      if (pendingSens) {
        try { await setTaskSensitiveAccess(taskId, true); } catch {}
      }

      // Re-apply the welcome-screen thinking choice. createTask triggers
      // the activeTaskId subscription which runs applyProjectDefaults() —
      // that overwrites the client-side thinking vars with stale saved
      // defaults from settings_json. Restore the user's pending choice
      // here so the first message actually uses what they picked.
      const pendingThinking = agentStore.getState('pendingThinking');
      if (pendingThinking) {
        thinkingEnabled = !!pendingThinking.enabled;
        if (typeof pendingThinking.effort === 'string') thinkingEffort = pendingThinking.effort;
        if (typeof pendingThinking.budget === 'number') thinkingBudget = pendingThinking.budget;
      }
    }

    // If the model is waiting for a question response, route via respondToAgentQuestion
    const currentTask = agentStore.getState('tasks')[taskId];
    if (currentTask?.pendingQuestion) {
      if (!text) return;
      textarea.value = '';
      textarea.style.height = '';
      await respondToAgentQuestion(taskId, currentTask.pendingQuestion.request_id, text);
      return;
    }

    // Resolve thinking budget from UI config
    const thinkConfig = getThinkingConfig();
    let thinkBudget = undefined;
    if (thinkConfig) {
      if (thinkConfig.type === 'budget') thinkBudget = thinkConfig.value;
      else if (thinkConfig.type === 'effort') {
        // Map effort levels to token budgets. The OpenAI provider in the
        // backend re-derives `reasoning_effort` from this budget.
        const effortMap = {
          minimal: 500, low: 2000, medium: 10000, high: 20000, xhigh: 40000, max: 32000,
          LOW: 2000, HIGH: 20000,
        };
        thinkBudget = effortMap[thinkConfig.value] || 10000;
      }
    }

    // Persist the current model / permission / thinking as this project's
    // defaults. Runs on every message so the "most recent choice" sticks
    // — previously this was one-shot, which meant the first chat's
    // thinking effort became permanent for the project and any later
    // change on the welcome screen was silently overwritten by the stale
    // value when creating the next chat.
    const task = agentStore.getState('tasks')[taskId];
    const projectId = task?.project_id || task?.projectId;
    if (projectId) {
      const effort = thinkingEnabled ? thinkingEffort : 'off';
      const mode = task?.permissionLevel || 'ManualEdit';
      const defaults = {
        model: task?.model || null,
        provider_type: task?.provider_type || null,
        permission_level: mode,
        thinking_effort: effort,
      };
      api.saveProjectDefaults(projectId, defaults).catch(() => {});
      if (task) task.projectDefaults = defaults;
    }

    const images = attachedFiles
      .filter(f => f.base64 && f.type.startsWith('image/'))
      .map(f => ({ media_type: f.type, data: f.base64 }));

    // Expand attached tags into the final message body.
    //   - Workflow tags → prepend the workflow body as an explicit section.
    //   - Skill tags    → add a trailing instruction so the agent invokes the
    //                     named skill (it will call `read_skill` to load it).
    //   - MCP tags      → add a short hint so the agent prefers that server.
    //   - File tags     → pass the path only; the agent uses `read_file`
    //                     on demand, keeping context clean.
    //   - Terminal tags → pass the session_id (+ pid/label for display); the
    //                     agent uses `read_terminal_output(session_id)` if it
    //                     needs the buffer.
    const workflowParts = attachedTags
      .filter(t => t.type === 'workflow' && t.body)
      .map(t => `## Workflow: ${t.name}\n\n${t.body}`);

    const skillHints = attachedTags
      .filter(t => t.type === 'skill')
      .map(t => `Use the skill \`${t.name}\` for this task.`);

    const mcpHints = attachedTags
      .filter(t => t.type === 'mcp')
      .map(t => `Use the \`${t.name}\` MCP server for this task.`);

    const fileRefs = attachedTags
      .filter(t => t.type === 'file' && t.path)
      .map(t => `- ${t.path}`);

    const terminalRefs = attachedTags
      .filter(t => t.type === 'terminal' && t.sessionId != null)
      .map(t => {
        const bits = [`session_id=${t.sessionId}`];
        if (t.pid != null)       bits.push(`pid=${t.pid}`);
        if (t.label)             bits.push(`label="${t.label}"`);
        return `- ${bits.join(', ')}`;
      });

    const finalParts = [];
    if (workflowParts.length) finalParts.push(workflowParts.join('\n\n'));
    if (skillHints.length)    finalParts.push(skillHints.join(' '));
    if (mcpHints.length)      finalParts.push(mcpHints.join(' '));
    if (fileRefs.length) {
      finalParts.push(
        `Referenced files (paths only — call \`read_file\` if you need contents):\n${fileRefs.join('\n')}`,
      );
    }
    if (terminalRefs.length) {
      finalParts.push(
        `Referenced terminals (use \`read_terminal_output\` with the session_id to fetch buffer):\n${terminalRefs.join('\n')}`,
      );
    }
    if (text)                 finalParts.push(text);
    const finalText = finalParts.join('\n\n');

    sendMessage(taskId, finalText, thinkBudget, images.length ? images : undefined);

    textarea.value = '';
    textarea.style.height = '';
    attachedFiles = [];
    attachedTags = [];
    renderAttachmentPills();
    renderTagChips();
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
      if (e.key === 'Enter' || e.key === 'Tab') {
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
    autoResizeTextarea();
    const ctx = getSlashContext(textarea);
    if (ctx) {
      // Refresh items on open OR on a trigger change (e.g. user deleted `/foo`
      // and started typing `@bar`). While the picker stays on the same
      // trigger, just refilter against the cached list.
      if (!slashPickerOpen || slashPickerTrigger !== ctx.trigger) {
        await loadSlashItems(ctx.trigger);
      }
      openSlashPicker(ctx.query, ctx.trigger);
    } else {
      if (slashPickerOpen) closeSlashPicker();
    }
  });

  textarea.addEventListener('blur', () => {
    setTimeout(() => closeSlashPicker(), 150);
  });

  // Toolbar left: agent-config (brain) + inline project pill. Upload
  // button was removed — images can still be added via clipboard paste.
  const toolbarLeft = el('div', { class: 'chat-toolbar-left' });
  toolbarLeft.appendChild(callConfigBtn);
  toolbarLeft.appendChild(projectBtn);

  // Toolbar right: send
  const toolbarRight = el('div', { class: 'chat-toolbar-right' });
  toolbarRight.appendChild(sendBtn);

  inputToolbar.appendChild(toolbarLeft);
  inputToolbar.appendChild(toolbarRight);

  // Input wrapper: bordered box containing textarea on top + toolbar on bottom
  const inputWrapper = el('div', { class: 'chat-input-wrapper' });
  inputWrapper.appendChild(textarea);
  inputWrapper.appendChild(inputToolbar);

  inputArea.appendChild(attachmentPills);
  inputArea.appendChild(tagChips);
  inputArea.appendChild(inputWrapper);

  // ── Task tab bar for parallel tasks ──────────────────────────────────────
  const taskTabBar = el('div', { class: 'chat-task-tabs' });

  // Task tab bar is permanently hidden — task switching is handled by the
  // agent panel task list on the left sidebar. Parallel tasks each show only
  // when selected; no tab strip appears in the chat view.
  function renderTaskTabs() {
    taskTabBar.style.display = 'none';
  }

  container.appendChild(headerBar);
  container.appendChild(taskTabBar);
  container.appendChild(stickyCard);
  container.appendChild(messagesArea);
  container.appendChild(approvalArea);
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
    const confirmed = await showConfirmDialog(
      'Revert to checkpoint',
      `The following changes will be made:\n\n${fileList}`,
      { confirmLabel: 'Revert' },
    );

    if (!confirmed) return;

    try {
      await api.revertToCheckpoint(checkpoint.id);
    } catch (e) {
      console.error('Failed to revert:', e);
    }
  }

  // ── Revert modal (themed confirmation for the user-message revert button) ──
  let revertModal = null;

  function closeRevertModal() {
    if (revertModal) { revertModal.remove(); revertModal = null; }
  }

  /**
   * Show a themed confirmation modal for reverting to before a user message.
   * Offers two paths depending on whether a checkpoint (with file changes)
   * exists for this message:
   *   - Revert chat only  → truncate messages, keep file changes
   *   - Revert chat + files → also undo file edits made since the checkpoint
   *
   * The file list is fetched from `previewCheckpoint` so the user can see
   * exactly what will be undone before committing.
   */
  async function openRevertModal(msg, msgIndex) {
    closeRevertModal();

    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;

    // The `checkpoints` array is populated async on every render, so it can
    // be stale the first time the user clicks revert right after a turn
    // finishes — the newest checkpoint may not be in it yet and we'd pick a
    // too-early one, causing `preview_checkpoint` to cascade across an extra
    // turn. Force a fresh load here so the selection is always correct.
    await loadCheckpoints(taskId);

    const checkpoint = findCheckpointForMessage(msgIndex);
    let changes = [];
    if (checkpoint) {
      try {
        changes = (await api.previewCheckpoint(checkpoint.id)) || [];
      } catch {
        changes = [];
      }
    }
    const hasFileChanges = changes.length > 0;

    const backdrop = el('div', { class: 'chat-revert-backdrop' });
    backdrop.addEventListener('click', closeRevertModal);

    const card = el('div', { class: 'chat-revert-card' });
    card.addEventListener('click', (e) => e.stopPropagation());

    card.appendChild(el('div', { class: 'chat-revert-card__title' }, 'Revert to before this message?'));

    const msgText = extractMessageText(msg);
    const preview = msgText.length > 100 ? msgText.slice(0, 100) + '…' : msgText;
    card.appendChild(el('div', { class: 'chat-revert-card__sub' }, `"${preview}"`));

    if (hasFileChanges) {
      const filesWrap = el('div', { class: 'chat-revert-card__files' });
      const header = el('div', { class: 'chat-revert-card__files-header' },
        `${changes.length} file change${changes.length === 1 ? '' : 's'} since this message`);
      filesWrap.appendChild(header);
      const visible = changes.slice(0, 8);
      for (const c of visible) {
        const row = el('div', { class: 'chat-revert-card__file-row' });
        const badge = el('span', { class: `chat-revert-card__file-badge chat-revert-card__file-badge--${c.change_type}` });
        badge.textContent = c.change_type === 'delete' ? 'Delete' : 'Restore';
        row.appendChild(badge);
        row.appendChild(el('span', { class: 'chat-revert-card__file-path', title: c.file_path }, c.file_path));
        filesWrap.appendChild(row);
      }
      if (changes.length > visible.length) {
        filesWrap.appendChild(el('div', { class: 'chat-revert-card__files-more' },
          `+ ${changes.length - visible.length} more`));
      }
      card.appendChild(filesWrap);
    }

    const actionsEl = el('div', { class: 'chat-revert-card__actions' });

    // Option 1: chat-only — always available. Primary when there's no
    // checkpoint to revert files from (i.e. this is the only action that
    // actually does anything).
    const chatOnlyBtn = el('button', {
      class: `chat-revert-card__btn${checkpoint ? '' : ' chat-revert-card__btn--primary'}`,
    });
    chatOnlyBtn.appendChild(icon('M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z', 15));
    const chatLabel = el('span');
    chatLabel.innerHTML = '<strong>Revert chat only</strong><em>Remove messages after this point, keep file changes</em>';
    chatOnlyBtn.appendChild(chatLabel);
    chatOnlyBtn.addEventListener('click', async () => {
      closeRevertModal();
      await retryFromCheckpoint(taskId, msgIndex, null, false);
    });
    actionsEl.appendChild(chatOnlyBtn);

    // Option 2: chat + files — shown whenever a checkpoint exists for this
    // message. The file-list preview may be empty for a specific checkpoint
    // (snapshots are recorded per-turn before writes), but the option itself
    // should still be available so the user can pick the semantics they want.
    if (checkpoint) {
      const fullBtn = el('button', { class: 'chat-revert-card__btn chat-revert-card__btn--primary' });
      fullBtn.appendChild(icon('M3 10h10a5 5 0 010 10H9m-6-10l4-4m-4 4l4 4', 15));
      const fullLabel = el('span');
      fullLabel.innerHTML = hasFileChanges
        ? '<strong>Revert chat and files</strong><em>Also undo the file edits listed above</em>'
        : '<strong>Revert chat and files</strong><em>Restore files to the snapshot taken before this message</em>';
      fullBtn.appendChild(fullLabel);
      fullBtn.addEventListener('click', async () => {
        closeRevertModal();
        await retryFromCheckpoint(taskId, msgIndex, checkpoint.id, true);
      });
      actionsEl.appendChild(fullBtn);
    }

    card.appendChild(actionsEl);

    const cancelBtn = el('button', { class: 'chat-revert-card__cancel' }, 'Cancel');
    cancelBtn.addEventListener('click', closeRevertModal);
    card.appendChild(cancelBtn);

    backdrop.appendChild(card);
    revertModal = backdrop;
    container.appendChild(revertModal);
  }

  // ── Welcome-screen history loading ──────────────────────────────────
  // On the welcome screen we show recent chats for the selected project.
  // The agent-panel only loads tasks for projects the user has expanded in
  // the sidebar, so kick off our own load for the picked project and merge
  // results into the shared `agentStore.tasks` so the lookup stays
  // consistent with the rest of the app.
  const welcomeHistoryLoading = new Set();
  async function loadWelcomeHistory(projectId) {
    if (!projectId || welcomeHistoryLoading.has(projectId)) return;
    welcomeHistoryLoading.add(projectId);
    try {
      const infos = await api.listTasks(projectId);
      if (!infos?.length) return;
      const stored = { ...agentStore.getState('tasks') };
      let changed = false;
      for (const info of infos) {
        if (!stored[info.id]) {
          stored[info.id] = { ...info, messages: [], isStreaming: false };
          changed = true;
        }
      }
      if (changed) agentStore.setState({ tasks: stored });
    } catch {} finally {
      welcomeHistoryLoading.delete(projectId);
    }
  }

  // Move the input area between the welcome card (center) and the normal
  // bottom position. Reparents rather than duplicates so all event handlers
  // stay attached.
  function placeInputArea(target) {
    if (inputArea.parentElement === target) return;
    target.appendChild(inputArea);
  }

  // Welcome-screen history expand/collapse state (mirrors the agent-panel
  // sidebar's VISIBLE_CHAT_LIMIT + "+ N more" pattern). Resets when the
  // user switches scope so a long Global list doesn't stay expanded when
  // they jump to a fresh project.
  const WELCOME_HISTORY_LIMIT = 5;
  let welcomeHistoryExpanded = false;
  let welcomeHistoryLastScope = null;

  function renderWelcome() {
    messagesArea.innerHTML = '';
    let projectId = agentStore.getState('pendingProjectId');
    const projects = workspaceStore.getState('projects');
    // First-run default: pick the first project so the welcome screen has
    // a sensible title and history list without forcing the user through
    // the agent-config popover.
    if (!projectId && projects.length > 0) {
      projectId = projects[0].id;
      setPendingProjectId(projectId);
    }
    const isGlobal = projectId === GLOBAL_PROJECT_ID;
    const project = isGlobal ? null : projects.find(p => String(p.id) === String(projectId));

    let title;
    if (isGlobal) title = 'What should we build?';
    else if (project) title = `What should we build in ${project.name}?`;
    else title = 'What would you like to do?';

    const emptyEl = el('div', { class: 'chat-empty' });
    const inner = el('div', { class: 'chat-empty__inner' });
    inner.appendChild(el('div', { class: 'chat-empty__prompt' }, title));

    // Input moves into the welcome card so the box appears directly under
    // the title, matching the reference screenshot.
    placeInputArea(inner);

    // History list for the selected scope. For a real project, kick off a
    // load so tasks not yet expanded in the sidebar still appear.
    if (projectId) loadWelcomeHistory(projectId);

    const tasks = agentStore.getState('tasks');
    const matchesScope = (t) => {
      const tp = t.project_id || t.projectId;
      if (isGlobal) return tp === GLOBAL_PROJECT_ID;
      if (!projectId) return false;
      return String(tp) === String(projectId);
    };
    const sorted = Object.values(tasks)
      .filter(matchesScope)
      .sort((a, b) => {
        const aMs = new Date(a.updated_at || a.updatedAt || a.created_at || a.createdAt || 0).getTime();
        const bMs = new Date(b.updated_at || b.updatedAt || b.created_at || b.createdAt || 0).getTime();
        return bMs - aMs;
      });

    // Reset the expanded flag when the selected scope changes so jumping
    // to a new project doesn't surface a previously-expanded long list.
    const scopeKey = isGlobal ? '__global__' : String(projectId || '');
    if (scopeKey !== welcomeHistoryLastScope) {
      welcomeHistoryExpanded = false;
      welcomeHistoryLastScope = scopeKey;
    }

    if (sorted.length > 0) {
      const histList = el('div', { class: 'chat-empty__history' });
      const visible = welcomeHistoryExpanded
        ? sorted
        : sorted.slice(0, WELCOME_HISTORY_LIMIT);

      for (const t of visible) {
        const row = el('button', { class: 'chat-empty__history-item' });
        row.appendChild(icon('M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z', 13));
        // Time label sits between the icon and the title. Prefer updated_at
        // (activity recency) then created_at; also check camelCase variants
        // in case the payload came from a different serializer path.
        const ts = t.updated_at || t.updatedAt || t.created_at || t.createdAt || '';
        const rel = formatRelativeTime(ts);
        if (rel) {
          const timeEl = el('span', { class: 'chat-empty__history-time', title: ts }, rel);
          row.appendChild(timeEl);
        }
        row.appendChild(el('span', { class: 'chat-empty__history-title' }, t.title || 'Untitled'));

        // Delete button, revealed on row hover. Uses the same action the
        // sidebar's per-task trash icon calls, so the backend path and
        // state cleanup are shared.
        const delBtn = el('span', {
          class: 'chat-empty__history-delete',
          role: 'button',
          'aria-label': 'Delete task',
          title: 'Delete task',
        });
        delBtn.appendChild(icon(
          'M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2',
          12,
        ));
        delBtn.addEventListener('click', async (e) => {
          e.stopPropagation();
          e.preventDefault();
          await deleteTaskAction(t.id);
          renderWelcome();
        });
        row.appendChild(delBtn);

        row.addEventListener('click', () => setActiveTask(t.id));
        histList.appendChild(row);
      }

      const hiddenCount = sorted.length - WELCOME_HISTORY_LIMIT;
      if (!welcomeHistoryExpanded && hiddenCount > 0) {
        const more = el('button', { class: 'chat-empty__history-expand' });
        more.textContent = `+ ${hiddenCount} more`;
        more.addEventListener('click', () => {
          welcomeHistoryExpanded = true;
          renderWelcome();
        });
        histList.appendChild(more);
      } else if (welcomeHistoryExpanded && sorted.length > WELCOME_HISTORY_LIMIT) {
        const less = el('button', { class: 'chat-empty__history-expand chat-empty__history-expand--collapse' });
        less.textContent = 'Show less';
        less.addEventListener('click', () => {
          welcomeHistoryExpanded = false;
          renderWelcome();
        });
        histList.appendChild(less);
      }

      inner.appendChild(histList);
    } else if (!projectId) {
      inner.appendChild(el('div', { class: 'chat-empty__hint-line' },
        'Pick a project or switch to Global in Agent configuration to get started.'));
    }

    emptyEl.appendChild(inner);
    messagesArea.appendChild(emptyEl);
  }

  function render() {
    updateModePill();
    updateCallConfigBtn();
    updateModelBtn();
    updateThinkBtn();
    updateContextBadge();
    updateSendBtn();
    updateProjectBtn();
    renderApprovalArea();
    const taskId = agentStore.getState('activeTaskId');
    container.classList.toggle('chat-view--welcome', !taskId);
    if (!taskId) {
      renderWelcome();
      return;
    }

    // Restore input to its normal bottom position when a task is active.
    placeInputArea(container);

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

    // Helper: is this node an "activity" (connected by the timeline line)?
    const isActivityNode = (n) => ['thinking', 'thinking-indicator', 'tool-use', 'collapsed-group', 'parallel-group', 'context-condense'].includes(n.type);

    // Render a single node into a DOM element (returns null to skip)
    const renderNodeEl = (node) => {
      switch (node.type) {
        case 'task-complete': {
          const b = node.content;
          if (b.diff && b.diff.files && b.diff.files.length > 0) {
            populateChangedFilesPanel(changedFilesPanel, b.diff, task);
          }
          // No summary (e.g. turn-limit, cancellation, model just stopped) →
          // nothing to show as a card; the existing assistant text bubble (if
          // any) will carry whatever was said.
          if (!b.summary) return null;

          const card = el('div', { class: 'chat-task-complete' });

          const header = el('div', { class: 'chat-task-complete__header' });
          const checkIcon = icon('M5 12l5 5L20 7', 13);
          header.appendChild(checkIcon);
          header.appendChild(el('span', { class: 'chat-task-complete__label' }, 'Task complete'));
          card.appendChild(header);

          const body = el('div', { class: 'chat-task-complete__body md' });
          try {
            body.innerHTML = renderMarkdown(b.summary);
          } catch {
            body.textContent = b.summary;
          }
          attachCodeCopyButtons(body);
          card.appendChild(body);

          // Copy summary button
          const actions = el('div', { class: 'chat-task-complete__actions' });
          const copyBtn = el('button', { class: 'chat-task-complete__copy', title: 'Copy summary' });
          copyBtn.appendChild(icon('M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z', 12));
          copyBtn.addEventListener('click', () => {
            navigator.clipboard.writeText(b.summary).catch(() => {});
            copyBtn.title = 'Copied!';
            setTimeout(() => { copyBtn.title = 'Copy summary'; }, 1500);
          });
          actions.appendChild(copyBtn);
          card.appendChild(actions);

          return card;
        }
        case 'context-condense': {
          return renderContextCondenseIndicator(node.content);
        }
        case 'model-switch': {
          const m = node.content.to_model, cur = task.model || task.info?.model || '', same = m === cur;
          return renderModelSwitchSeparator(m, same && thinkingEnabled ? thinkingEffort : null, same && thinkingEnabled ? thinkingBudget : null);
        }
        case 'user-message': {
          const msg = node.msg, i = node.msgIdx;
          const msgEl = el('div', { class: 'chat-message chat-message--user' });
          for (const b of msg.content) {
            if (b.type === 'text' && b.text) { const t = el('div', { class: 'chat-message__text' }); t.innerHTML = formatText(b.text); attachCodeCopyButtons(t); msgEl.appendChild(t); }
            else if (b.type === 'image' && b.data) { const img = el('img', { class: 'chat-message__image', src: `data:${b.media_type};base64,${b.data}` }); img.addEventListener('click', () => openImageLightbox(img.src)); msgEl.appendChild(img); }
          }
          // Per-turn cost pill — tokens + $ spent answering this specific message.
          const tu = msg.turnUsage;
          // [debug badge] Fires every time a user bubble re-renders. If the
          // pill visibly resets, compare this log to the accumulator log: if
          // state here shows nonzero but the pill disappears, it's a render
          // bug; if state itself is zeroed, the reset is upstream.
          console.log(
            `[debug badge] render user-msg idx=${i} turnUsage=${JSON.stringify(tu || null)}`
          );
          if (tu && (tu.input || tu.output || tu.cacheRead || tu.cacheWrite)) {
            const sent = (tu.input || 0) + (tu.cacheRead || 0) + (tu.cacheWrite || 0);
            const recv = tu.output || 0;
            const costTxt = tu.cost > 0
              ? (tu.cost < 0.001 ? '<$0.001' : `$${tu.cost.toFixed(3)}`)
              : '$0';
            const pill = el('div', { class: 'chat-message__turn-usage' });
            pill.title = [
              `Input: ${(tu.input || 0).toLocaleString()}`,
              `Output: ${(tu.output || 0).toLocaleString()}`,
              `Cache read: ${(tu.cacheRead || 0).toLocaleString()}`,
              `Cache write: ${(tu.cacheWrite || 0).toLocaleString()}`,
              `Cost: $${(tu.cost || 0).toFixed(4)}`,
            ].join('\n');
            pill.appendChild(el('span', { class: 'turn-usage__sent' }, `↑${formatTokens(sent)}`));
            pill.appendChild(el('span', { class: 'turn-usage__sep' }, ' · '));
            pill.appendChild(el('span', { class: 'turn-usage__recv' }, `↓${formatTokens(recv)}`));
            pill.appendChild(el('span', { class: 'turn-usage__sep' }, ' · '));
            pill.appendChild(el('span', { class: 'turn-usage__cost' }, costTxt));
            msgEl.appendChild(pill);
          }
          const actions = el('div', { class: 'chat-message__actions chat-message__actions--user' });
          const copyBtn = el('button', { class: 'chat-message__action-btn', title: 'Copy' });
          copyBtn.appendChild(icon('M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z', 13));
          copyBtn.addEventListener('click', (e) => { e.stopPropagation(); navigator.clipboard.writeText(extractMessageText(msg)).catch(() => {}); copyBtn.title = 'Copied!'; setTimeout(() => { copyBtn.title = 'Copy'; }, 1500); });
          actions.appendChild(copyBtn);
          // Revert button — rolls the task back to the state just before this
          // user message. Reverts file changes from the checkpoint AND
          // truncates chat history. Hidden while the task is running because
          // mid-turn reverts would leave inconsistent state. Checkpoint is
          // looked up at click time so we always see the latest array.
          if (!isRunning) {
            const revertBtn = el('button', { class: 'chat-message__action-btn chat-message__revert-btn', title: 'Revert to before this message' });
            revertBtn.appendChild(icon('M3 10h10a5 5 0 010 10H9m-6-10l4-4m-4 4l4 4', 13));
            revertBtn.addEventListener('click', (e) => {
              e.stopPropagation();
              openRevertModal(msg, i);
            });
            actions.appendChild(revertBtn);
          }
          msgEl.appendChild(actions);
          return msgEl;
        }
        case 'thinking-indicator': return renderThinkingIndicator();
        case 'thinking': {
          const msgContent = task.messages[node.msgIdx]?.content || [];
          const blockIndex = node.contentIdx;
          let lastAssistantIdx = -1;
          for (let mi = task.messages.length - 1; mi >= 0; mi--) { if (task.messages[mi].role === 'assistant') { lastAssistantIdx = mi; break; } }
          const isInLast = node.msgIdx === lastAssistantIdx;
          const isStr = task.isStreaming && isInLast;
          const isLastOrEmpty = blockIndex >= 0 && (blockIndex === msgContent.length - 1 || (blockIndex === msgContent.length - 2 && msgContent[msgContent.length - 1]?.type === 'text' && !msgContent[msgContent.length - 1]?.text));
          return renderThinkingBlock(node.block, isStr && isLastOrEmpty, `thinking-${node.blockIdx}`);
        }
        case 'assistant-text': {
          const s = task.isStreaming && node.isLastMsg;
          const w = el('div', { class: 'chat-message chat-message--assistant' });
          const last = node.blocks[node.blocks.length - 1];
          for (const b of node.blocks) {
            const isStreaming = s && b === last;
            const t = el('div', { class: `chat-message__text${isStreaming ? ' chat-message__text--streaming' : ''}` });
            t.innerHTML = formatText(b.text);
            // Don't add buttons to the actively-streaming block — it rebuilds every delta.
            // They're added once streaming finishes and renderMessages re-runs without the class.
            if (!isStreaming) attachCodeCopyButtons(t);
            w.appendChild(t);
          }
          return w;
        }
        case 'tool-use': {
          if (node.toolName === 'todo_write') return renderMinimalToolIndicator('todo_write', node.block, node.toolResult);
          if (node.toolName === 'spawn_subagent') return renderSubagentCard(node.block, node.toolResult);
          if (node.toolName === 'wait_for_subagents' || node.toolName === 'list_active_agents') return renderMinimalToolIndicator(node.toolName, node.block, node.toolResult);
          return renderToolCallCard(node.block, node.toolResult);
        }
        case 'collapsed-group': return renderCollapsedGroup(node);
        case 'parallel-group': return renderParallelGroup(node);
        case 'checkpoint-anchor': {
          return null;
        }
      }
      return null;
    };

    // Render nodes — group consecutive activity nodes into timeline sections.
    // "Transparent" node types (checkpoint-anchor, model-switch) render to null
    // most of the time and should NOT break an ongoing timeline when they do.
    const isTransparentNode = (n) => n.type === 'checkpoint-anchor' || n.type === 'model-switch';

    let timeline = null;
    const flushTimeline = () => { if (timeline) { messagesArea.appendChild(timeline); timeline = null; } };

    for (const node of nodes) {
      if (isActivityNode(node)) {
        if (!timeline) timeline = el('div', { class: 'activity-timeline' });
        const rendered = renderNodeEl(node);
        if (rendered) { const item = el('div', { class: 'activity-timeline__item' }); item.appendChild(rendered); timeline.appendChild(item); }
      } else if (isTransparentNode(node)) {
        // Render but don't flush the timeline — only break if it actually produces visible output
        const rendered = renderNodeEl(node);
        if (rendered) {
          // Checkpoint/model-switch rendered something visible — flush timeline, then append
          flushTimeline();
          messagesArea.appendChild(rendered);
        }
        // If null, just skip — timeline stays intact
      } else {
        flushTimeline();
        const rendered = renderNodeEl(node);
        if (rendered) messagesArea.appendChild(rendered);
      }
    }
    flushTimeline();

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
    // Resolve persistent expand state first so the body and chevron are
    // built in their final visual state — see renderToolCallCard for the
    // chevron-flicker rationale.
    const groupKey = `group-${group.children[0]?.toolUseId || group.children[0]?.msgIdx}`;
    const wasOpen = !!expandedState.get(groupKey);

    const container = el('div', { class: 'collapsed-group' });

    // Header row — always visible
    const header = el('button', { class: 'collapsed-group__header', type: 'button' });

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

    // Chevron — start in the final rotation so re-renders don't animate it
    const chevron = el('span', { class: 'collapsed-group__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    if (wasOpen) chevron.style.transform = 'rotate(180deg)';
    header.appendChild(chevron);

    container.appendChild(header);

    // Expandable body with individual tool cards
    const body = el('div', { class: `collapsed-group__body${wasOpen ? '' : ' collapsed-group__body--hidden'}` });
    for (const child of group.children) {
      if (child.toolName === 'spawn_subagent') {
        body.appendChild(renderSubagentCard(child.block, child.toolResult));
      } else {
        body.appendChild(renderToolCallCard(child.block, child.toolResult));
      }
    }
    container.appendChild(body);

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

  agentStore.subscribe('lastRequestUsage', () => {
    // Context % is driven off the LAST request's input/cache tokens — refresh
    // the progress ring (and its tooltip) whenever a new usage report lands.
    updateContextBadge();
    updateCostDisplay();
  });

  agentStore.subscribe('tasks', () => {
    updateCostDisplay();
    updateHeaderBar();
    renderStickyCard();
    renderTaskTabs();
    // Always update the send button — even in the streaming fast-path below.
    // This ensures the spinner/stop button reacts immediately when the task
    // completes, without waiting for a full debounced re-render.
    updateSendBtn();

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
        // While the model is streaming, render the assistant message as
        // plain text — assigning textContent is O(N) per chunk vs. marked +
        // DOMPurify which becomes O(N²) over a long reply (each chunk
        // re-parses the entire growing string from scratch). The full
        // markdown render fires once when streaming completes.
        if (lastBlock?.type === 'text') {
          const streamingEl = messagesArea.querySelector('.chat-message__text--streaming');
          if (streamingEl && lastBlock.text) {
            // Schedule the textContent update on rAF so a burst of token
            // events coalesces into one paint per frame instead of N.
            if (!streamingEl._rustic_pendingFrame) {
              streamingEl._rustic_pendingFrame = true;
              requestAnimationFrame(() => {
                streamingEl._rustic_pendingFrame = false;
                // `lastBlock` may have grown since this rAF was scheduled;
                // re-read the current text from the store.
                const liveTask = agentStore.getState('tasks')?.[taskId];
                const liveLast = liveTask?.messages?.[liveTask.messages.length - 1];
                const liveBlock = liveLast?.content?.[liveLast.content.length - 1];
                if (liveBlock?.type === 'text' && typeof liveBlock.text === 'string') {
                  streamingEl.textContent = liveBlock.text;
                  streamingEl.classList.add('chat-message__text--streaming-plain');
                  autoScrollIfNeeded();
                }
              });
            }
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
  agentStore.subscribe('activeTaskId', () => {
    render(); updateCostDisplay(); updateHeaderBar(); renderStickyCard(); renderTaskTabs();
    // Apply project defaults (thinking effort) when switching to a new task
    applyProjectDefaults();
  });
  // Welcome screen depends on the picked project + the project list.
  agentStore.subscribe('pendingProjectId', () => {
    if (!agentStore.getState('activeTaskId')) render();
  });
  workspaceStore.subscribe('projects', () => {
    if (!agentStore.getState('activeTaskId')) render();
  });
  agentStore.subscribe('permissionRequests', () => { renderApprovalArea(); renderTaskTabs(); });
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
  renderStickyCard();
  renderTaskTabs();

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
 * Tool results appear as role 'tool' during live execution, as role 'user'
 * when loaded from the database (the API sends client-side tool results with
 * User role), and inline under role 'assistant' for server-executed tools
 * like Anthropic's web_search / web_fetch (tool_use and tool_result are
 * emitted in the same assistant turn). All three cases must be pooled so
 * history replay can pair server-side tool cards with their results.
 */
function buildResultMap(messages) {
  const map = new Map();
  for (const msg of messages) {
    for (const block of (msg.content || [])) {
      if (block.type === 'tool_result' && block.tool_use_id) {
        map.set(block.tool_use_id, block);
      }
    }
  }
  return map;
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
  attachCodeCopyButtons(bodyEl);
  card.appendChild(bodyEl);

  // Response (only for questions that have been answered)
  if (isQuestion && hasResponse) {
    const responseEl = el('div', { class: 'chat-msg-card__response' });
    responseEl.appendChild(el('span', { class: 'chat-msg-card__response-label' }, 'Your response:'));
    // Strip the "User response: " prefix added by the backend tool output
    const responseText = String(result.content).replace(/^User response:\s*/i, '');
    responseEl.appendChild(el('span', {}, responseText));
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

// ── Context condense indicator ──────────────────────────────────────────────

function renderContextCondenseIndicator(content) {
  const isRunning = content.status === 'running';
  const indicator = el('div', { class: 'tool-indicator context-condense-indicator' });

  const iconEl = el('span', { class: 'tool-indicator__icon' });
  if (isRunning) {
    iconEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    iconEl.appendChild(icon('M5 13l4 4L19 7', 11));
    iconEl.classList.add('tool-indicator__icon--ok');
  }
  indicator.appendChild(iconEl);

  let labelText = 'Compacting context...';
  if (!isRunning) {
    labelText = `Context compacted: ${content.original_messages} → ${content.condensed_to} messages`;
  }
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
 * Render a subagent card: collapsible card with clean header row.
 * Collapsed: [icon] name [spinner/✓/✗] [chevron]
 * Expanded:  stats row with ↑tokens ↓tokens $cost words + input/output buttons
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
  const liveOutput = liveAgent?.output || (result ? String(result.content || '') : '');
  const livePrompt = liveAgent?.prompt || prompt;
  const liveSummary = liveAgent?.summary || '';

  const isRunning = status === 'running';
  const isFailed = status === 'failed';

  const statusClass = isRunning ? '' : isFailed ? ' subagent-card--failed' : ' subagent-card--completed';
  const card = el('div', { class: `subagent-card${statusClass}`, 'data-tool-use-id': id });

  // ── Header row: icon + name + status + chevron ──
  const headerRow = el('div', { class: 'subagent-card__header' });

  // Agent icon (purple)
  const iconWrap = el('span', { class: 'tool-call__icon tool-call__icon--purple' });
  iconWrap.appendChild(icon('M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', 13));
  headerRow.appendChild(iconWrap);

  // Agent name (truncated via CSS)
  headerRow.appendChild(el('span', { class: 'subagent-card__name' }, name));

  // Status: spinner | ✓ | ✗
  const statusEl = el('span', { class: 'tool-call__status' });
  if (isRunning) {
    statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    const checkPath = isFailed ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
    statusEl.appendChild(icon(checkPath, 12));
    statusEl.classList.add(isFailed ? 'tool-call__status--error' : 'tool-call__status--ok');
  }
  headerRow.appendChild(statusEl);

  // Chevron toggle
  const chevron = el('span', { class: 'subagent-card__chevron' });
  chevron.appendChild(icon('M6 9l6 6 6-6', 12));
  headerRow.appendChild(chevron);

  card.appendChild(headerRow);

  // ── Details panel (hidden by default) ──
  const details = el('div', { class: 'subagent-card__details' });

  const liveCost = liveAgent?.cost;
  const inputTokens = liveCost?.total_input_tokens || 0;
  const outputTokens = liveCost?.total_output_tokens || 0;
  const subCostUsd = liveCost?.estimated_cost_usd || 0;
  const wordCount = liveOutput ? liveOutput.trim().split(/\s+/).filter(Boolean).length : 0;

  // Stats row: [↑ tokens] [↓ tokens] $ cost  words
  const statsRow = el('div', { class: 'subagent-card__stats' });

  // ↑ Input button (clickable, opens input prompt)
  const inputBtn = el('button', { class: 'subagent-card__token-btn subagent-card__token-btn--sent', title: 'View input prompt' });
  inputBtn.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '↑'));
  inputBtn.appendChild(el('span', {}, inputTokens > 0 ? formatTokens(inputTokens) : '0'));
  inputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    openScratchInEditor(`[Input] ${name}`, livePrompt, 'markdown');
  });
  statsRow.appendChild(inputBtn);

  // ↓ Output button (clickable, opens streamed activity log — live only,
  // not persisted across reloads).
  const outputBtn = el('button', { class: 'subagent-card__token-btn subagent-card__token-btn--recv', title: 'View output' });
  outputBtn.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '↓'));
  outputBtn.appendChild(el('span', {}, outputTokens > 0 ? formatTokens(outputTokens) : '0'));
  outputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    const currentOutput = agentStore.getState('subagents')?.[taskId]?.[agentId]?.output || liveOutput;
    if (currentOutput) {
      openScratchInEditor(`[Output] ${name}`, currentOutput, 'markdown');
    }
  });
  statsRow.appendChild(outputBtn);

  // 📋 Final answer button — only visible once the sub-agent has produced a
  // summary. This is the piece that's persisted to DB, so the button keeps
  // working after reload even though the streamed activity log above does
  // not.
  if (liveSummary) {
    const answerBtn = el('button', { class: 'subagent-card__token-btn subagent-card__token-btn--answer', title: 'View final answer' });
    answerBtn.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '★'));
    answerBtn.appendChild(el('span', {}, 'Answer'));
    answerBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const latest = agentStore.getState('subagents')?.[taskId]?.[agentId]?.summary || liveSummary;
      openScratchInEditor(`[Answer] ${name}`, latest, 'markdown');
    });
    statsRow.appendChild(answerBtn);
  }

  // $ cost
  const costStat = el('span', { class: 'subagent-card__stat subagent-card__stat--cost' });
  costStat.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '$'));
  costStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, subCostUsd > 0 ? subCostUsd.toFixed(3) : '0'));
  statsRow.appendChild(costStat);

  // Word count
  const wordStat = el('span', { class: 'subagent-card__stat subagent-card__stat--words' });
  wordStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, wordCount > 0 ? `${wordCount} words` : '0 words'));
  statsRow.appendChild(wordStat);

  details.appendChild(statsRow);
  card.appendChild(details);

  // Toggle expand/collapse on header click
  headerRow.addEventListener('click', () => {
    card.classList.toggle('subagent-card--expanded');
  });

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

  // Compute persistent expand state up-front so the body and chevron are
  // built directly in their final visual state — without this, the body is
  // born hidden and the chevron starts at 0deg, then we flip them after
  // append. Because the chevron has a CSS `transform 0.15s` transition, that
  // post-append flip animates from 0→180deg every time renderMessages rebuilds
  // the chat (which it does on every tool_use / tool_result event during
  // streaming). The user perceives this as the dropdown "resetting" itself
  // even though the open flag is preserved in expandedState.
  const toolKey = `tool-${id}`;
  const wasOpen = !!expandedState.get(toolKey);

  const card = el('div', { class: 'tool-call', 'data-tool-use-id': id });

  // ── Header: icon + label + summary + status + chevron (thinking-block style) ──
  const header = el('button', { class: 'tool-call__header', type: 'button' });

  // Colored icon
  const iconWrap = el('span', { class: `tool-call__icon tool-call__icon--${meta.color}` });
  iconWrap.appendChild(icon(meta.iconPath, 13));
  header.appendChild(iconWrap);

  // Tool label
  header.appendChild(el('span', { class: 'tool-call__name' }, label));

  // One-line summary (path / command / pattern)
  if (summary) {
    header.appendChild(el('span', { class: 'tool-call__summary' }, summary));
  }

  // Status: spinner | ✓ | ✗  (right next to summary, not pushed to far right)
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

  // Chevron — start in the final rotation so it doesn't animate on re-render
  const chevron = el('span', { class: 'tool-call__chevron' });
  chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
  if (wasOpen) chevron.style.transform = 'rotate(180deg)';
  header.appendChild(chevron);

  card.appendChild(header);

  // ── Expandable body: clickable Input / Output buttons with preview ──
  const body = el('div', { class: `tool-call__body${wasOpen ? '' : ' tool-call__body--hidden'}` });

  const inputText = formatToolInput(name, input);

  // Input button — click to open in scratch editor
  const inputBtn = el('button', { class: 'tool-call__action-btn' });
  inputBtn.appendChild(el('span', { class: 'tool-call__action-label' }, 'Input'));
  const inputPreview = inputText.split('\n').slice(0, 3).join('\n');
  if (inputPreview.trim()) {
    const inputPre = el('pre', { class: 'tool-call__preview' });
    inputPre.textContent = inputPreview;
    inputBtn.appendChild(inputPre);
  }
  inputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    openScratchInEditor(`[Input] ${label}`, inputText, 'json');
  });
  body.appendChild(inputBtn);

  // Output button — click to open in scratch editor, show 3-line preview.
  // web_search / web_fetch results come back as JSON with encrypted fields;
  // format them before display so the user sees titles + URLs, not blobs.
  if (result && result.content != null) {
    const content = formatToolOutput(name, result.content);
    const outputBtn = el('button', {
      class: `tool-call__action-btn${isError ? ' tool-call__action-btn--error' : ''}`,
    });
    outputBtn.appendChild(el('span', { class: 'tool-call__action-label' }, isError ? 'Error' : 'Output'));
    const previewLines = content.split('\n').slice(0, 3).join('\n');
    if (previewLines.trim()) {
      const outputPre = el('pre', { class: 'tool-call__preview' });
      outputPre.textContent = previewLines;
      outputBtn.appendChild(outputPre);
    }
    outputBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      openScratchInEditor(`[Output] ${label}`, content, 'text');
    });
    body.appendChild(outputBtn);
  }

  card.appendChild(body);

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
        diffBtn.disabled = false;
        if (diff && diff.files && diff.files.length > 0) {
          diffBtn.textContent = 'Hide diff';
          diffCard = el('div', { class: 'chat-checkpoint__diff-view' });
          for (const file of diff.files) {
            const row = el('div', { class: 'chat-checkpoint__diff-row' });
            const st = file.status === 'Created' ? '+' : file.status === 'Deleted' ? '−' : '~';
            const stColor = file.status === 'Created' ? 'bright-green' : file.status === 'Deleted' ? 'bright-red' : 'bright-yellow';
            row.appendChild(el('span', { style: `color:var(--${stColor});font-weight:700;width:14px;text-align:center;flex-shrink:0` }, st));
            row.appendChild(el('span', { style: 'flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-family:var(--font-family-mono);font-size:11px;color:var(--fg2)' }, file.path));
            diffCard.appendChild(row);
          }
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

  const projectId = task.project_id || task.projectId;
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
      openDiffView({ projectId, filePath: absPath, unifiedDiff: file.unified_diff });
    });
    row.appendChild(openBtn);

    // Click row → open diff in editor
    row.addEventListener('click', () => {
      const absPath = projectRoot ? projectRoot + sep + file.path.replace(/[\\/]/g, sep) : file.path;
      openDiffView({ projectId, filePath: absPath, unifiedDiff: file.unified_diff });
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

function formatText(text) {
  return renderMarkdown(text);
}

// attachCodeCopyButtons, openImageLightbox extracted to ./chat-view/ — see imports.
