import { el, icon, iconMulti } from '../../utils/dom.js';
import { createCombobox } from '../../utils/combobox.js';
import { agentStore, sendMessage, setActiveTask, setTaskPermissions, setTaskSensitiveAccess, respondToPermission, respondToAgentQuestion, setPendingProjectId, setPendingModelChoice, setPendingPermissionLevel, setPendingSensitiveAccess, setPendingThinking, createTask, deleteTaskAction, retrySendMessage, queueMessage, clearQueuedMessage, GLOBAL_PROJECT_ID, getTaskProjectRoot } from '../../state/agent.js';
import { workspaceStore } from '../../state/workspace.js';
import { terminalStore } from '../../state/terminal.js';
import { openDiffView } from '../../state/editor.js';
import * as api from '../../lib/tauri-api.js';
import { loadProviderConfigs, saveProviderConfigs, refreshAllProviderModels, pricingFor, contextWindowFor, hasAnyConnectedProvider } from '../settings/ai-settings.js';
import { openSettings, setCategory as setSettingsCategory } from '../../state/settings.js';
import { getCustomModel } from '../../state/custom-models.js';
import { openCustomModelModal } from '../settings/custom-model-modal.js';
import { renderMarkdown } from '../../lib/markdown.js';
import { timeSync, logBigString, mark } from '../../lib/perf-debug.js';
import { processMessages } from '../../utils/message-pipeline.js';
import { formatRelativeTime } from '../../utils/format-time.js';
import { showConfirmDialog, showAlertDialog, showRevertDialog } from '../confirm-dialog.js';
import { attachCodeCopyButtons } from './chat-view/code-copy.js';
import { openImageLightbox } from './chat-view/image-lightbox.js';
import {
  TOOL_META,
  TOOL_META_DEFAULT,
  DIFF_TOOL_NAMES,
  getToolSummary,
  formatToolOutput,
  formatToolInput,
  formatEditDiffForOutput,
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

  // Harness providers (Claude Code, Codex) own their own model selection
  // through the CLI itself. Rustic doesn't need pricing or context-window
  // numbers for them — cost is billed against the user's subscription
  // (rendered as "subscription" in the cost pill, not USD), and the CLI
  // manages its own context-window budget. So skip both the registration
  // modal and the setAiProvider reconfigure call entirely.
  if (providerType === 'ClaudeCode' || providerType === 'Codex') return true;

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
  // User-saved custom override > frontend registry > 0 (defer to backend).
  // The registry covers cases where the backend's defaults are wrong or
  // missing — currently GPT-5.5's 1M context window and the cached-input
  // rate for Claude / Claude Code aliases.
  const registryPricing = pricingFor(modelId) || {};
  const maxOut = custom?.maxOutputTokens  || 0;
  const inCost = custom?.inputCost        || 0;
  const outCost = custom?.outputCost      || 0;
  const cIn    = custom?.cachedInputCost  || registryPricing.cachedInput  || 0;
  const cOut   = custom?.cachedOutputCost || registryPricing.cachedOutput || 0;
  const ctxW   = custom?.contextWindow    || contextWindowFor(modelId)   || 0;

  // Thinking budget is a per-task client setting (chat-view's agent-config
  // popover) — no longer a per-provider field. Pass null so the backend
  // falls back to its own registry default for this model.
  try {
    await api.setAiProvider(
      providerType, '__STORED__', modelId, cfg.baseUrl || null, null,
      maxOut, inCost, outCost, cIn, cOut, ctxW || null, null, cfg.name || null,
    );
  } catch (e) { console.warn('[pickModel] setAiProvider failed:', e); }

  cfg.model = modelId;
  cfg.customMaxOutputTokens   = maxOut;
  cfg.customInputCost         = inCost;
  cfg.customOutputCost        = outCost;
  cfg.customCachedInputCost   = cIn;
  cfg.customCachedOutputCost  = cOut;
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

// User picks for stale question prompts whose live request died with the
// worker thread (process restart, hard kill, etc.). The persisted tool_use
// block has no tool_result, so without this map the question keeps
// re-rendering as "pending" with clickable buttons even after the user
// picked an answer. Keyed by the question's tool_use_id; survives DOM
// rebuilds within the session.
const pickedChoiceState = new Map();

// Returns thinking capability info for the given model, or null if not supported.
function getThinkingCapability(model) {
  if (!model) return null;
  // Claude Code aliases (sonnet / opus / haiku) used by the subscription
  // harness. The CLI accepts `--effort <level>` with values
  // {low, medium, high, xhigh, max}; we expose the same tiers we surface for
  // the equivalent native Anthropic models so the UI is consistent. Match
  // these *before* the longer "claude-..." patterns so "opus" alone wins.
  if (model === 'opus')                  return { type: 'effort', levels: ['low', 'medium', 'high', 'max'] };
  if (model === 'sonnet' || model === 'haiku') return { type: 'effort', levels: ['low', 'medium', 'high'] };
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
    e.stopPropagation();
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

  // Persistent DOM for the cost-display widgets — built once, mutated in
  // place from `updateCostDisplay`. The previous version did
  // `headerStatsRow.innerHTML = ''` + rebuild on every `agent-request-usage`
  // event, which fires N times per multi-tool turn. The visible flash of
  // those nested spans being torn down and rebuilt was a major flicker
  // source even though the messagesArea cache was working correctly.
  let costDomBuilt = false;
  let statusLineCtx = null, statusLineCtxSep = null, statusLineTurns = null;
  let statusLineSentSep = null, statusLineSent = null, statusLineRecv = null;
  let headerStatSent = null, headerStatRecv = null, headerStatCost = null;
  let headerStatCostEl = null;
  function buildCostDom() {
    // Status line: [ctx]  ·  [turns]  ·  ↑sent ↓recv
    statusLine.replaceChildren();
    statusLineCtx = el('span', { class: 'status-line__ctx', style: 'display:none' });
    statusLineCtxSep = el('span', { class: 'status-line__sep', style: 'display:none' }, '  ·  ');
    statusLineTurns = el('span', { class: 'status-line__turns' });
    statusLineSentSep = el('span', { class: 'status-line__sep', style: 'display:none' }, '  ·  ');
    statusLineSent = el('span', { class: 'status-line__sent', style: 'display:none' });
    const gap = el('span', { class: 'status-line__gap' }, ' ');
    statusLineRecv = el('span', { class: 'status-line__recv', style: 'display:none' });
    statusLine.appendChild(statusLineCtx);
    statusLine.appendChild(statusLineCtxSep);
    statusLine.appendChild(statusLineTurns);
    statusLine.appendChild(statusLineSentSep);
    statusLine.appendChild(statusLineSent);
    statusLine.appendChild(gap);
    statusLine.appendChild(statusLineRecv);

    // Header stats row — three pills, always present, mutated in place.
    headerStatsRow.replaceChildren();
    function makeStat(cls, iconChar) {
      const stat = el('span', { class: `chat-header-stat chat-header-stat--${cls}` });
      stat.appendChild(el('span', { class: 'chat-header-stat__icon' }, iconChar));
      const v = el('span', { class: 'chat-header-stat__value' }, '');
      stat.appendChild(v);
      stat._iconEl = stat.firstChild;
      stat._valueEl = v;
      return stat;
    }
    headerStatSent = makeStat('sent', '↑');
    headerStatRecv = makeStat('recv', '↓');
    headerStatCost = makeStat('cost', '$');
    headerStatsRow.appendChild(headerStatSent);
    headerStatsRow.appendChild(headerStatRecv);
    headerStatsRow.appendChild(headerStatCost);
    headerStatCostEl = headerStatCost;
    costDomBuilt = true;
  }

  function setVisible(node, on) {
    if (!node) return;
    node.style.display = on ? '' : 'none';
  }

  function updateCostDisplay() {
    if (!costDomBuilt) buildCostDom();
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      progressCostLabel.textContent = '';
      setVisible(statusLineCtx, false); setVisible(statusLineCtxSep, false);
      statusLineTurns.textContent = '';
      setVisible(statusLineSentSep, false); setVisible(statusLineSent, false); setVisible(statusLineRecv, false);
      headerStatSent._valueEl.textContent = ''; headerStatRecv._valueEl.textContent = ''; headerStatCost._valueEl.textContent = '';
      return;
    }
    const task = agentStore.getState('tasks')[taskId];
    const cost = task?.cost;
    if (!cost) {
      progressCostLabel.textContent = '';
      setVisible(statusLineCtx, false); setVisible(statusLineCtxSep, false);
      statusLineTurns.textContent = '';
      setVisible(statusLineSentSep, false); setVisible(statusLineSent, false); setVisible(statusLineRecv, false);
      headerStatSent._valueEl.textContent = ''; headerStatRecv._valueEl.textContent = ''; headerStatCost._valueEl.textContent = '';
      return;
    }

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

    // Subscription-mode tasks (Claude Code today, Codex later) don't have a
    // meaningful USD figure — the user is paying a flat subscription, not
    // per-token. Showing "$0" would be technically true but misleading
    // ("did the model not use any tokens?"). Plan §B.7: render a subscription
    // marker in place of the dollar amount; token counters stay since they
    // remain useful for understanding context usage.
    const isSubscriptionTask = (task?.provider_type || task?.info?.provider_type || '') === 'ClaudeCode';

    const costStr = isSubscriptionTask
      ? 'subscription'
      : usd > 0
        ? usd < 0.001 ? '<$0.001' : `$${usd.toFixed(3)}`
        : '$0';

    // Progress bar label = cost (or subscription marker)
    progressCostLabel.textContent = costStr;

    // Hover tooltip on progress bar — cumulative across the whole task.
    progressWrapper.title = [
      `Total ↑ Sent: ${sentTotal.toLocaleString()} (in=${totalInput.toLocaleString()}, cache_read=${(cost.total_cache_read_tokens || 0).toLocaleString()}, cache_write=${(cost.total_cache_write_tokens || 0).toLocaleString()})`,
      `Total ↓ Received: ${recvTotal.toLocaleString()}`,
      cacheRead > 0 ? `Cache read: ${cacheRead.toLocaleString()}` : null,
      `Turns: ${cost.turn_count ?? 0}`,
      sub.usd > 0 ? `Sub-agent cost: $${sub.usd.toFixed(4)}` : null,
      isSubscriptionTask
        ? 'Billing: Claude subscription (no per-call USD).'
        : `Est. cost: $${usd.toFixed(4)}`,
    ].filter(Boolean).join('\n');

    // ── Status line: in-place text + visibility toggles ────────────────────
    const ctxPctText = statusLine.dataset.ctxPct || '';
    const turnsText = `${cost.turn_count ?? 0} turn${(cost.turn_count ?? 0) === 1 ? '' : 's'}`;
    const hasTotals = sentTotal || recvTotal;

    setVisible(statusLineCtx, !!ctxPctText);
    setVisible(statusLineCtxSep, !!ctxPctText);
    if (ctxPctText && statusLineCtx.textContent !== ctxPctText) statusLineCtx.textContent = ctxPctText;
    if (statusLineTurns.textContent !== turnsText) statusLineTurns.textContent = turnsText;

    setVisible(statusLineSentSep, !!hasTotals);
    setVisible(statusLineSent, !!hasTotals);
    setVisible(statusLineRecv, !!hasTotals);
    if (hasTotals) {
      const sentText = `↑${formatTokens(sentTotal)}`;
      const recvText = `↓${formatTokens(recvTotal)}`;
      if (statusLineSent.textContent !== sentText) statusLineSent.textContent = sentText;
      if (statusLineRecv.textContent !== recvText) statusLineRecv.textContent = recvText;
    }

    // ── Header stats row: in-place value updates ───────────────────────────
    const sentVal = formatTokens(sentTotal);
    const recvVal = formatTokens(recvTotal);
    const costVal = isSubscriptionTask
      ? 'subscription'
      : (usd > 0 ? (usd < 0.001 ? '<0.001' : usd.toFixed(3)) : '0');
    const costIcon = isSubscriptionTask ? '∞' : '$';

    if (headerStatSent._valueEl.textContent !== sentVal) headerStatSent._valueEl.textContent = sentVal;
    if (headerStatRecv._valueEl.textContent !== recvVal) headerStatRecv._valueEl.textContent = recvVal;
    if (headerStatCost._valueEl.textContent !== costVal) headerStatCost._valueEl.textContent = costVal;
    if (headerStatCost._iconEl.textContent !== costIcon) headerStatCost._iconEl.textContent = costIcon;
    headerStatCost.classList.toggle('chat-header-stat--cost-subscription', isSubscriptionTask);
    headerStatCost.title = isSubscriptionTask
      ? 'Tokens billed against your Claude subscription — no per-call USD cost.'
      : '';
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

  // Todo rows live inside the bottom tabs bar (see below) — `stickyBodyEl`
  // is the actual rows container, eagerly built so the reconcile path in
  // `renderStickyCard` can write into it without a lazy first-render dance.
  // todo content (string) → row element. Keyed by content because the
  // backend doesn't ship a stable id; same content + same status + same
  // position is treated as the same row.
  const stickyBodyEl = el('div', { class: 'sticky-card__body' });
  const stickyTodoRows = new Map();

  function buildTodoRow(item) {
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
    row._status = item.status;
    return row;
  }

  // In-place row update: only touches the parts of the row that changed
  // status. The spinner DOM survives if `in_progress` is still in_progress,
  // so its CSS animation never restarts. Returns the same row element.
  function updateTodoRow(row, item) {
    if (row._status === item.status) return row;
    row.className = `sticky-card__todo sticky-card__todo--${item.status}`;
    // Replace the checkbox content based on new status.
    const checkbox = row.firstChild;
    if (checkbox) checkbox.replaceChildren();
    if (item.status === 'completed') {
      checkbox?.appendChild(icon('M5 13l4 4L19 7', 11));
    } else if (item.status === 'in_progress') {
      checkbox?.appendChild(el('span', { class: 'sticky-card__todo-spinner' }));
    } else {
      checkbox?.appendChild(el('span', { class: 'sticky-card__todo-empty' }));
    }
    // Active badge — add or remove without rebuilding the whole row.
    const existingBadge = row.querySelector(':scope > .sticky-card__todo-badge');
    if (item.status === 'in_progress' && !existingBadge) {
      row.appendChild(el('span', { class: 'sticky-card__todo-badge sticky-card__todo-badge--active' }, 'Active'));
    } else if (item.status !== 'in_progress' && existingBadge) {
      existingBadge.remove();
    }
    row._status = item.status;
    return row;
  }

  function renderStickyCard() {
    const taskId = agentStore.getState('activeTaskId');
    const task = taskId && agentStore.getState('tasks')[taskId];
    const todos = (taskId && agentStore.getState('todos')[taskId]) || [];

    if (!taskId || !task || todos.length === 0) {
      tabsAvailable.todo = false;
      // Don't clear stickyBodyEl — keep persistent DOM so when todos come
      // back the spinners and rows reconcile rather than rebuild.
      updateTabsAreaUI();
      return;
    }

    tabsAvailable.todo = true;

    // Update counter on the tab button + (when this tab is expanded) the
    // panel header. Reading the same string from one source keeps them in
    // sync without an extra subscribe.
    const completedCount = todos.filter(t => t.status === 'completed').length;
    const counterText = `${completedCount}/${todos.length}`;
    if (todoTabBadge.textContent !== counterText) todoTabBadge.textContent = counterText;
    if (tabsActiveTab === 'todo' && tabsPanelCount.textContent !== counterText) {
      tabsPanelCount.textContent = counterText;
    }

    // Sort: in_progress first, then completed, then pending. Same order as
    // before, so the user-visible row order is preserved.
    const sorted = [...todos].sort((a, b) => {
      const order = { in_progress: 0, completed: 1, pending: 2 };
      return (order[a.status] ?? 3) - (order[b.status] ?? 3);
    });

    // Reconcile: walk the new list, reuse rows by content, mutate status in
    // place, drop rows whose content is no longer present.
    const seen = new Set();
    const finalRows = [];
    for (const item of sorted) {
      seen.add(item.content);
      let row = stickyTodoRows.get(item.content);
      if (row) {
        updateTodoRow(row, item);
      } else {
        row = buildTodoRow(item);
        stickyTodoRows.set(item.content, row);
      }
      finalRows.push(row);
    }
    for (const [content, row] of stickyTodoRows) {
      if (!seen.has(content)) {
        row.remove();
        stickyTodoRows.delete(content);
      }
    }
    // Apply the (possibly reordered) row sequence in one pass — same DOM
    // identities reused, so spinners keep their animation state.
    stickyBodyEl.replaceChildren(...finalRows);

    updateTabsAreaUI();
  }

  // Messages area
  const messagesArea = el('div', { class: 'chat-messages' });

  // Approval requests area (shown between messages and input)
  const approvalArea = el('div', { class: 'chat-approval-area' });

  // Queued user-input area (mid-turn steering, plan §14). Rendered above
  // the approval/input region so the user always sees what's about to fire
  // when the current turn ends. Lives outside the main messages area so a
  // streaming-fastpath update doesn't have to repaint it.
  const queuedArea = el('div', { class: 'chat-queued-area' });

  // Sub-agents panel (shown when active sub-agents exist)

  // ── Bottom panel — sits above the input area and surfaces the agent's
  // todo list and the cumulative changed-files view. Collapsed = a row of
  // tab pills. Click a pill to expand its content; click the panel header
  // to collapse back. The whole bar hides itself when neither tab has
  // anything to show.
  const TABS_ACTIVE_KEY = 'rustic_chat_active_tab';
  let tabsActiveTab = null;
  try {
    const saved = localStorage.getItem(TABS_ACTIVE_KEY);
    if (saved === 'todo' || saved === 'files') tabsActiveTab = saved;
  } catch {}
  const tabsAvailable = { todo: false, files: false };

  const chatTabsArea = el('div', { class: 'chat-tabs-area', style: 'display:none;' });

  // Tab row (collapsed state).
  const tabsRow = el('div', { class: 'chat-tabs-area__row' });
  function buildTabButton(name, label, iconPath) {
    const btn = el('button', { class: 'chat-tab', 'data-tab': name, type: 'button' });
    if (iconPath) btn.appendChild(icon(iconPath, 12));
    btn.appendChild(el('span', { class: 'chat-tab__label' }, [label]));
    const badge = el('span', { class: 'chat-tab__badge' }, ['0']);
    btn.appendChild(badge);
    btn.addEventListener('click', () => setActiveTab(name));
    return { btn, badge };
  }
  const todoTab = buildTabButton(
    'todo',
    'Todo',
    'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2',
  );
  const filesTab = buildTabButton('files', 'Changed files', null);
  const todoTabBtn = todoTab.btn, todoTabBadge = todoTab.badge;
  const filesTabBtn = filesTab.btn, filesTabBadge = filesTab.badge;
  tabsRow.appendChild(todoTabBtn);
  tabsRow.appendChild(filesTabBtn);

  // Panel (expanded state). Header shows active tab title + count + a
  // chevron (clicking the header collapses back). Actions slot holds tab-
  // specific controls (e.g. "Revert all" for the files tab).
  const tabsPanel = el('div', { class: 'chat-tabs-area__panel', style: 'display:none;' });
  const tabsPanelHeader = el('div', {
    class: 'chat-tabs-area__panel-header',
    role: 'button',
    tabindex: '0',
    title: 'Click to collapse',
  });
  const tabsPanelChevron = el('span', { class: 'chat-tabs-area__panel-chevron' });
  tabsPanelChevron.appendChild(icon('M19 9l-7 7-7-7', 10));
  const tabsPanelTitle = el('span', { class: 'chat-tabs-area__panel-title' }, ['']);
  const tabsPanelCount = el('span', { class: 'chat-tabs-area__panel-count' }, ['0']);
  const tabsPanelActions = el('div', { class: 'chat-tabs-area__panel-actions' });
  tabsPanelHeader.appendChild(tabsPanelChevron);
  tabsPanelHeader.appendChild(tabsPanelTitle);
  tabsPanelHeader.appendChild(tabsPanelCount);
  tabsPanelHeader.appendChild(tabsPanelActions);
  tabsPanelHeader.addEventListener('click', (e) => {
    if (e.target.closest('.chat-tabs-area__panel-actions')) return;
    setActiveTab(null);
  });
  tabsPanelHeader.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      setActiveTab(null);
    }
  });

  const tabsPanelBody = el('div', { class: 'chat-tabs-area__panel-body' });

  // Per-tab content slots. Each is a wrapper that the relevant render
  // function writes into; the tab system just toggles their visibility.
  // Todo content: stickyBodyEl was created above so renderStickyCard can
  // write rows into it without indirection.
  const todoTabContent = el('div', { class: 'chat-tabs-content', 'data-tab': 'todo' });
  todoTabContent.appendChild(stickyBodyEl);

  const changedFilesList = el('ul', { class: 'chat-changed-files-list' });
  const filesTabContent = el('div', { class: 'chat-tabs-content', 'data-tab': 'files' });
  filesTabContent.appendChild(changedFilesList);

  // "Revert all" — restores every file the agent touched in this task to its
  // pre-task state. Parked in the panel header's actions slot whenever the
  // files tab is active. Stop click bubbling so the panel-header collapse
  // handler doesn't fire alongside.
  const changedFilesRevertBtn = el('button', {
    class: 'chat-changed-files-revert',
    title: 'Revert every file the agent touched in this task to its pre-task state. Chat history stays intact.',
  }, ['Revert all']);
  changedFilesRevertBtn.addEventListener('click', (e) => e.stopPropagation());

  tabsPanelBody.appendChild(todoTabContent);
  tabsPanelBody.appendChild(filesTabContent);
  tabsPanel.appendChild(tabsPanelHeader);
  tabsPanel.appendChild(tabsPanelBody);

  chatTabsArea.appendChild(tabsRow);
  chatTabsArea.appendChild(tabsPanel);

  function setActiveTab(name) {
    if (name && !tabsAvailable[name]) name = null;
    tabsActiveTab = name;
    try { localStorage.setItem(TABS_ACTIVE_KEY, name || ''); } catch {}
    updateTabsAreaUI();
  }

  function updateTabsAreaUI() {
    const anyAvailable = tabsAvailable.todo || tabsAvailable.files;
    if (!anyAvailable) {
      chatTabsArea.style.display = 'none';
      if (inputArea) inputArea.classList.remove('chat-input-area--has-files-panel');
      return;
    }
    chatTabsArea.style.display = '';
    if (inputArea) inputArea.classList.add('chat-input-area--has-files-panel');

    todoTabBtn.style.display = tabsAvailable.todo ? '' : 'none';
    filesTabBtn.style.display = tabsAvailable.files ? '' : 'none';

    // Drop activeTab if the tab is no longer available.
    if (tabsActiveTab && !tabsAvailable[tabsActiveTab]) tabsActiveTab = null;

    if (tabsActiveTab) {
      tabsRow.style.display = 'none';
      tabsPanel.style.display = '';
      todoTabContent.style.display = tabsActiveTab === 'todo' ? '' : 'none';
      filesTabContent.style.display = tabsActiveTab === 'files' ? '' : 'none';
      if (tabsActiveTab === 'todo') {
        tabsPanelTitle.textContent = 'Todo';
        tabsPanelCount.textContent = todoTabBadge.textContent;
        tabsPanelActions.replaceChildren();
      } else if (tabsActiveTab === 'files') {
        tabsPanelTitle.textContent = 'Changed files';
        tabsPanelCount.textContent = filesTabBadge.textContent;
        tabsPanelActions.replaceChildren(changedFilesRevertBtn);
      }
    } else {
      tabsRow.style.display = '';
      tabsPanel.style.display = 'none';
    }
  }

  // ── Changed-files tab (cumulative net change across the whole task).
  //
  // The map mirrors what `fh_list_task_net_changes` returns: one entry per
  // path the agent touched, classified by net result vs the file's pre-task
  // state (created / modified / deleted) — NOT by what the latest turn did
  // locally. So a file created in turn A and edited in turn C reads as
  // "created" here. Per-message revert is the existing per-turn UI; this
  // panel is purely the "what changed in total" view.
  //
  // We don't try to derive net change client-side from incremental
  // `agent-file-tracked` events; that requires anchor-snapshot bookkeeping
  // the backend already does. Instead we recompute fully via the backend
  // command, debounced.
  const netChanges = new Map();
  let netChangesProjectRoot = null;
  let netChangesRefreshScheduled = false;

  function renderChangedFilesPanel() {
    if (netChanges.size === 0) {
      tabsAvailable.files = false;
      updateTabsAreaUI();
      return;
    }
    tabsAvailable.files = true;
    const countText = String(netChanges.size);
    if (filesTabBadge.textContent !== countText) filesTabBadge.textContent = countText;
    if (tabsActiveTab === 'files' && tabsPanelCount.textContent !== countText) {
      tabsPanelCount.textContent = countText;
    }
    renderFilesListInto(changedFilesList, netChanges, netChangesProjectRoot);
    updateTabsAreaUI();
  }

  function renderFilesListInto(listEl, filesMap, projectRoot) {
    listEl.innerHTML = '';
    const sorted = Array.from(filesMap.entries()).sort((a, b) => a[0].localeCompare(b[0]));
    for (const [path, meta] of sorted) {
      const isCreated = meta.kind === 'created';
      const isDeleted = meta.kind === 'deleted';
      const isBinary = meta.binary === true || meta.kind === 'binary';
      const clickable = !isDeleted && !!meta.anchorMessageId;
      const titleAction = isDeleted
        ? '(deleted — file is gone from disk)'
        : (isCreated || isBinary)
          ? 'click to open file'
          : 'click to view diff vs pre-task state';

      const li = el('li', {
        class: `chat-changed-files-item${clickable ? '' : ' chat-changed-files-item--disabled'}`,
        'data-kind': meta.kind,
        title: `${path} — ${titleAction}`,
      });
      li.appendChild(el('span', { class: 'chat-changed-files-dot' }));
      li.appendChild(el('span', { class: 'chat-changed-files-path' }, [path]));

      const stats = el('span', { class: 'chat-changed-files-stats' });
      if (isCreated) {
        stats.appendChild(el('span', { class: 'chat-changed-files-badge chat-changed-files-badge--new' }, ['new']));
      } else if (isDeleted) {
        stats.appendChild(el('span', { class: 'chat-changed-files-badge chat-changed-files-badge--deleted' }, ['deleted']));
      }
      if (isBinary) {
        if (!isCreated && !isDeleted) {
          stats.appendChild(el('span', { class: 'chat-changed-files-binary' }, ['binary']));
        }
      } else if (typeof meta.additions === 'number' || typeof meta.deletions === 'number') {
        const add = meta.additions || 0;
        const del = meta.deletions || 0;
        if (add > 0) stats.appendChild(el('span', { class: 'chat-changed-files-add' }, [`+${add}`]));
        if (del > 0) stats.appendChild(el('span', { class: 'chat-changed-files-del' }, [`-${del}`]));
      }
      li.appendChild(stats);

      if (clickable) {
        li.addEventListener('click', () => openChangedFile(path, meta, projectRoot));
      }
      listEl.appendChild(li);
    }
  }

  async function openChangedFile(path, meta, projectRoot) {
    if (!projectRoot || !meta?.anchorMessageId) return;
    const isBinary = meta.binary === true || meta.kind === 'binary';
    const isCreated = meta.kind === 'created';
    const resolveAbs = () => {
      const sep = projectRoot.includes('\\') && !projectRoot.includes('/') ? '\\' : '/';
      const trimmedRoot = projectRoot.replace(/[\\/]+$/, '');
      const absPath = `${trimmedRoot}${sep}${path.replace(/[\\/]+/g, sep)}`;
      const projects = workspaceStore.getState('projects') || [];
      const norm = (p) => p.replace(/\\/g, '/').replace(/\/+$/, '');
      const project = projects.find((p) => norm(p.root_path) === norm(projectRoot));
      return { absPath, projectName: project?.name || '' };
    };
    if (isCreated || isBinary) {
      const resolved = resolveAbs();
      window.dispatchEvent(new CustomEvent('rustic:open-file', {
        detail: { path: resolved.absPath, projectName: resolved.projectName },
      }));
      return;
    }
    try {
      const diff = await api.fhFileDiff(projectRoot, meta.anchorMessageId, path);
      if (!diff) return;
      if (!diff.unified) {
        const resolved = resolveAbs();
        window.dispatchEvent(new CustomEvent('rustic:open-file', {
          detail: { path: resolved.absPath, projectName: resolved.projectName },
        }));
        return;
      }
      openDiffView({ filePath: path, unifiedDiff: diff.unified });
    } catch (e) {
      console.error('[file-history] open diff failed:', e);
    }
  }

  /// Reload the cumulative net-change list for `taskId` from the backend.
  /// Debounced (~250ms) so a burst of file-tracked events from one turn
  /// produces at most one DB query per quiet window.
  function scheduleNetChangesRefresh(taskId) {
    if (!taskId) return;
    if (netChangesRefreshScheduled) return;
    netChangesRefreshScheduled = true;
    setTimeout(async () => {
      netChangesRefreshScheduled = false;
      const activeTaskId = agentStore.getState('activeTaskId');
      if (taskId !== activeTaskId) return; // user switched away
      const projectRoot = getTaskProjectRoot(taskId);
      if (!projectRoot) return;
      try {
        const rows = await api.fhListTaskNetChanges(projectRoot, taskId);
        if (taskId !== agentStore.getState('activeTaskId')) return;
        netChanges.clear();
        netChangesProjectRoot = projectRoot;
        if (Array.isArray(rows)) {
          for (const r of rows) {
            netChanges.set(r.path, {
              kind: r.kind,
              binary: r.binary === true,
              additions: r.additions,
              deletions: r.deletions,
              anchorMessageId: r.anchor_message_id,
            });
          }
        }
        renderChangedFilesPanel();
      } catch (e) {
        console.warn('[file-history] net-changes refresh failed:', e);
      }
    }, 250);
  }

  changedFilesRevertBtn.addEventListener('click', async () => {
    const taskId = agentStore.getState('activeTaskId');
    const projectRoot = netChangesProjectRoot || getTaskProjectRoot(taskId);
    if (!taskId || !projectRoot) return;
    let entries = [];
    try {
      entries = await api.fhPlanRevertTask(projectRoot, taskId);
    } catch (e) {
      console.warn('[file-history] plan revert task failed:', e);
    }
    const choice = await showRevertDialog({
      title: 'Revert all files in this task',
      subtitle: 'Restores every file the agent touched, across every turn, to the state before this task started. The chat history will not be cleared.',
      entries,
      actions: [
        { label: 'Cancel', value: 'cancel', kind: 'cancel' },
        { label: 'Revert files', value: 'revert', kind: 'danger' },
      ],
    });
    if (choice !== 'revert') return;
    // Auto-abort a running turn so the agent's next tool write doesn't race
    // the revert. Best-effort: harness/native both honour abortTask.
    const tasks = agentStore.getState('tasks');
    const activeTask = tasks[taskId];
    if (activeTask && (activeTask.status === 'Running' || activeTask.isStreaming)) {
      try { await api.abortTask(taskId); } catch {}
    }
    try {
      const outcomes = await api.fhRevertTask(projectRoot, taskId);
      netChanges.clear();
      renderChangedFilesPanel();
      if (Array.isArray(outcomes)) {
        const touched = outcomes.filter((o) => o.action !== 'unchanged').length;
        console.log(`[file-history] revert task touched ${touched} of ${outcomes.length} files`);
      }
    } catch (e) {
      showAlertDialog('Revert failed', String(e));
    }
  });

  // Live updates: tracker fires `agent-file-tracked` after every edit/sweep.
  // Debounced refetch keeps the panel in sync without one DB roundtrip per
  // event. `unlisten` is held in closure so the listener outlives chat-view
  // re-renders (chat-view itself is created once per app).
  api.onAgentFileTracked((payload) => {
    const taskId = payload?.task_id || agentStore.getState('activeTaskId');
    if (taskId === agentStore.getState('activeTaskId')) {
      scheduleNetChangesRefresh(taskId);
    }
  }).catch((e) => console.warn('[file-history] subscribe failed', e));

  /// Map a user message at `userMsgIndex` (0-based, the position in
  /// task.messages) to the file_history snapshot_message_id that was opened
  /// when that message was sent. We don't persist this mapping anywhere, so
  /// we reconstruct it: nth user message (counting from 0 over user-role
  /// messages only) ↔ nth snapshot in chronological order.
  ///
  /// Returns null if no snapshot covers this message (e.g. the task started
  /// before file_history was wired up, or the canonicalize failed for the
  /// turn). Caller should disable file-revert in that case.
  async function snapshotIdForUserMessage(taskId, userMsgIndex) {
    try {
      const snapshots = await api.fhListSnapshots(taskId);
      if (!Array.isArray(snapshots) || snapshots.length === 0) return null;
      const tasks = agentStore.getState('tasks');
      const task = tasks[taskId];
      if (!task) return null;
      // Count which user-message this is (0-based among user roles only).
      let userOrdinal = -1;
      for (let i = 0; i <= userMsgIndex && i < task.messages.length; i++) {
        if (task.messages[i].role === 'user') userOrdinal++;
      }
      if (userOrdinal < 0) return null;
      const userCount = task.messages.filter((m) => m.role === 'user').length;

      // Three alignment regimes are possible:
      //   • snapshots.length === userCount  — one snapshot per turn (the
      //     normal post-restart shape). Direct map: nth user msg ↔ nth snap.
      //   • snapshots.length  <  userCount  — some user messages predate the
      //     tracker (or the user did chat-only revert + new turns). Align
      //     from the end so the LATEST snapshots map to the LATEST messages.
      //   • snapshots.length  >  userCount  — chat was truncated (chat-only
      //     revert) but snapshots were kept. Align from the END too: the
      //     latest user message maps to the latest snapshot, older messages
      //     walk backwards from there.
      // In every regime we fall back to a clamp-to-bounds rather than
      // returning null, so the user always gets a "Revert chat + files"
      // option whenever any snapshot exists. The plan dialog still shows
      // exactly which paths would be touched, so the user can review.
      let snapIdx;
      if (snapshots.length === userCount) {
        snapIdx = userOrdinal;
      } else if (snapshots.length < userCount) {
        const offset = userCount - snapshots.length;
        snapIdx = userOrdinal - offset;
      } else {
        // More snapshots than messages — anchor from the end of both lists.
        const offset = snapshots.length - userCount;
        snapIdx = userOrdinal + offset;
      }
      if (snapIdx < 0) snapIdx = 0;
      if (snapIdx >= snapshots.length) snapIdx = snapshots.length - 1;
      return snapshots[snapIdx]?.message_id || null;
    } catch (e) {
      console.warn('[file-history] snapshotIdForUserMessage failed:', e);
      return null;
    }
  }

  async function handlePerMessageRevertClick(userMsgIndex, messageText) {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;
    const projectRoot = getTaskProjectRoot(taskId);
    const snapshotId = projectRoot
      ? await snapshotIdForUserMessage(taskId, userMsgIndex)
      : null;

    let entries = [];
    if (snapshotId && projectRoot) {
      try {
        entries = await api.fhPlanRevertFromMessage(projectRoot, snapshotId);
      } catch (e) {
        console.warn('[file-history] plan revert from message failed:', e);
      }
    }

    const actions = [];
    actions.push({ label: 'Cancel', value: 'cancel', kind: 'cancel' });
    actions.push({ label: 'Revert chat only', value: 'chat', kind: 'primary' });
    if (snapshotId) {
      actions.push({ label: 'Revert chat + files', value: 'chat-and-files', kind: 'danger' });
    }
    const subtitle = snapshotId
      ? 'Removes every message after this one from the chat. Optionally also restores files this and later turns modified.'
      : 'Removes every message after this one from the chat. (No file snapshot is available for this message — only the chat-only option is offered.)';

    const choice = await showRevertDialog({
      title: 'Revert from this message',
      subtitle,
      entries: snapshotId ? entries : [],
      actions,
    });
    if (choice === 'cancel') return;

    // Auto-abort a running turn before either path mutates state.
    const tasks = agentStore.getState('tasks');
    const activeTask = tasks[taskId];
    if (activeTask && (activeTask.status === 'Running' || activeTask.isStreaming)) {
      try { await api.abortTask(taskId); } catch {}
    }

    try {
      if (choice === 'chat-and-files' && snapshotId && projectRoot) {
        await api.fhRevertFromMessage(projectRoot, snapshotId);
      }
      // Both paths drop messages AFTER this user message (the user message
      // itself stays so the user can see what was sent — but the answer
      // confirmed by the question explicitly says "whatever the chat after
      // that message will be removed"). The chosen message's text gets
      // mirrored into the input box for easy edit-and-resend.
      const keepCount = userMsgIndex; // drop msg at userMsgIndex and everything after
      await api.truncateTaskMessages(taskId, keepCount);

      // Reflect truncation in the in-memory store so the chat re-renders
      // immediately rather than waiting for a hydrate.
      const updated = { ...agentStore.getState('tasks') };
      const t = updated[taskId];
      if (t && Array.isArray(t.messages)) {
        updated[taskId] = { ...t, messages: t.messages.slice(0, keepCount), isStreaming: false };
        agentStore.setState({ tasks: updated });
      }

      if (messageText) {
        textarea.value = messageText;
        autoResizeTextarea();
        textarea.focus();
      }

      // The per-message revert can change which files have a "net change" vs
      // the pre-task state (chat-only revert leaves files alone, but a
      // chat+files revert can shrink the set). Recompute so the bottom-panel
      // count stays accurate.
      netChanges.clear();
      renderChangedFilesPanel();
      scheduleNetChangesRefresh(taskId);
    } catch (e) {
      showAlertDialog('Revert failed', String(e));
    }
  }

  // Input area
  const inputArea = el('div', { class: 'chat-input-area' });
  const textarea = el('textarea', {
    class: 'chat-input',
    placeholder: 'Send a message...',
  });

  // Pull the user's tool config so the placeholder can advertise which
  // media tools (image / video / animate) are currently configured. The
  // hint only appears at idle — Running / WaitingForInput stay as before.
  let mediaToolsHint = '';
  function recomputeMediaHint(cfg) {
    if (!cfg || !cfg.media) { mediaToolsHint = ''; return; }
    const m = cfg.media;
    const enabled = [];
    if (m.image && m.image.provider_key && m.image.model) enabled.push('image');
    if (m.video && m.video.provider_key && m.video.model) enabled.push('video');
    const linked = !!m.link_animate_to_video;
    const animateEntry = linked ? m.video : m.animate;
    if (animateEntry && animateEntry.provider_key && animateEntry.model) enabled.push('animate');
    if (enabled.length === 0) { mediaToolsHint = ''; return; }
    // Pluralized, professional summary. The order is fixed (images → videos →
    // animations) so the same set of tools always reads identically.
    // Animator-only setups still get the proper "animations" noun, not the
    // verbal "animate one" we used to emit.
    const labelMap = { image: 'images', video: 'videos', animate: 'animations' };
    const parts = enabled.map((k) => labelMap[k]);
    const joined = parts.length === 1
      ? parts[0]
      : parts.length === 2
        ? `${parts[0]} and ${parts[1]}`
        : `${parts[0]}, ${parts[1]}, and ${parts[2]}`;
    mediaToolsHint = `  ·  or generate ${joined}`;
    // Refresh the placeholder immediately if the textarea is idle.
    if (typeof updateSendBtn === 'function') updateSendBtn();
  }
  api.getToolConfig().then(recomputeMediaHint).catch(() => {});
  // Re-pull when settings are saved (the panel writes via setToolConfig but
  // doesn't broadcast — a "storage" event from localStorage is the cheapest
  // signal that something changed, and tool-settings always writes there).
  window.addEventListener('storage', (e) => {
    if (e.key === 'rustic_tool_config') {
      try { recomputeMediaHint(JSON.parse(e.newValue || 'null')); } catch { /* ignore */ }
    }
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

  // Recent-models stash so the dropdown can surface models the user actually
  // uses rather than forcing a scroll through all 30+ Anthropic / OpenAI
  // entries every time. Persists in localStorage; capped at 8 to keep the
  // group compact.
  const RECENT_MODELS_KEY = 'rustic_recent_models';
  function loadRecentModels() {
    try {
      const raw = localStorage.getItem(RECENT_MODELS_KEY);
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : [];
    } catch { return []; }
  }
  function pushRecentModel(providerId, modelId) {
    try {
      const list = loadRecentModels()
        .filter((m) => !(m.providerId === providerId && m.modelId === modelId));
      list.unshift({ providerId, modelId });
      localStorage.setItem(RECENT_MODELS_KEY, JSON.stringify(list.slice(0, 8)));
    } catch {}
  }

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

  // Mirror of `is_harness_provider_key` in crates/rustic-agent/src/config.rs.
  // Harness providers (CC / Codex) own their own session context, so a chat
  // that started on one cannot be migrated to the other or to a stateless
  // API provider — Rustic only mirrors visible messages, not the CLI's
  // internal state. The dropdown locks incompatible entries; the backend
  // also rejects the call as a defence-in-depth check.
  function isHarnessProvider(providerId) {
    return providerId === 'ClaudeCode' || providerId === 'Codex';
  }
  function canSwitchTo(fromProvider, toProvider) {
    if (!fromProvider) return true;
    const fromHarness = isHarnessProvider(fromProvider);
    const toHarness = isHarnessProvider(toProvider);
    if (fromHarness !== toHarness) return false;
    if (fromHarness && toHarness) return fromProvider === toProvider;
    return true;
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

    // No more forced refresh on every dropdown open — was adding hundreds of
    // milliseconds of latency just to surface model lists the user already
    // has cached. The backend cache + Settings → Refresh covers the rare
    // case where a new model appeared. If the cache is empty, we still kick
    // a non-forced refresh in the background so the next open is fresh.
    const configs = loadProviderConfigs();
    const providerEntries = Object.entries(configs)
      .filter(([, cfg]) => cfg.hasKey && cfg.models?.length);

    if (providerEntries.length === 0) {
      if (!aiConfig) await loadAiConfig();
      if (!aiConfig?.providers?.length) return;
      // Background refresh so future opens see model lists.
      refreshAllProviderModels(false).catch(() => {});
    }

    closeThinkPopover();
    closeModeDropdown();

    modelDropdownOpen = true;
    modelDropdown = el('div', { class: 'chat-model-dropdown' });
    const currentModel = getCurrentModel();
    // Family lock: once a chat exists, harness chats can only swap models
    // within the same harness family; API chats can swap between any API
    // provider. Welcome screen (no active task) is unrestricted.
    const lockActive = !!taskId;
    const currentProvider = lockActive ? getCurrentProviderType() : '';

    // Search box at the top of the dropdown. Filters all groups by case-
    // insensitive substring match on model id + provider id.
    const searchInput = el('input', {
      class: 'chat-model-dropdown__search',
      type: 'text',
      placeholder: 'Search models…',
      autocomplete: 'off',
      spellcheck: 'false',
    });
    modelDropdown.appendChild(searchInput);

    const listWrap = el('div', { class: 'chat-model-dropdown__list' });
    modelDropdown.appendChild(listWrap);

    // Build a flat list of every model (provider, modelId) to filter against.
    // Then partition into Recent + provider groups before rendering.
    const allModels = [];
    if (providerEntries.length > 0) {
      for (const [providerId, cfg] of providerEntries) {
        for (const modelId of cfg.models) {
          allModels.push({ providerId, modelId, providerName: cfg.name || null });
        }
      }
    } else {
      for (const provider of (aiConfig?.providers || []).filter((p) => p.enabled)) {
        if (provider.default_model) {
          allModels.push({
            providerId: provider.provider_type,
            modelId: provider.default_model,
            providerName: null,
          });
        }
      }
    }

    const recents = loadRecentModels()
      .map((entry) => allModels.find((m) => m.providerId === entry.providerId && m.modelId === entry.modelId))
      .filter(Boolean)
      .slice(0, 5);

    function rerender(query) {
      listWrap.innerHTML = '';
      const q = (query || '').trim().toLowerCase();
      const matches = (m) => {
        if (!q) return true;
        return m.modelId.toLowerCase().includes(q)
          || m.providerId.toLowerCase().includes(q)
          || (m.providerName || '').toLowerCase().includes(q);
      };

      const renderItem = (m) => {
        const locked = lockActive && !canSwitchTo(currentProvider, m.providerId);
        const classes = ['chat-model-dropdown__item'];
        if (m.modelId === currentModel && m.providerId === currentProvider) {
          classes.push('chat-model-dropdown__item--active');
        }
        if (locked) classes.push('chat-model-dropdown__item--locked');
        const item = el('div', { class: classes.join(' ') });
        item.textContent = m.modelId;
        if (locked) {
          item.title = isHarnessProvider(currentProvider)
            ? `Locked — this chat started on ${currentProvider}; start a new chat to use ${m.providerId}.`
            : `Locked — this chat uses an API provider; start a new chat to use ${m.providerId}.`;
        } else {
          item.title = `${m.modelId} — ${m.providerId}`;
        }
        item.addEventListener('click', async (ev) => {
          ev.stopPropagation();
          if (locked) return;
          closeModelDropdown();
          try {
            if (!(await pickModel(m.providerId, m.modelId))) return;
            saveThinkingForModel(currentModel);
            await api.switchModel(taskId, m.providerId, m.modelId);
            restoreThinkingForModel(m.modelId);
            // Track usage for the Recent group on next open.
            pushRecentModel(m.providerId, m.modelId);
          } catch (err) {
            console.error('Failed to switch model:', err);
          }
        });
        listWrap.appendChild(item);
      };

      // Recent group (only when no query — searching across recents is
      // confusing because the model also appears in its provider group).
      if (!q && recents.length > 0) {
        listWrap.appendChild(el('div', { class: 'chat-model-dropdown__group' }, 'Recent'));
        for (const m of recents) renderItem(m);
      }

      // Provider groups.
      if (providerEntries.length > 0) {
        for (const [providerId, cfg] of providerEntries) {
          const groupLabel = providerId.startsWith('Compatible:')
            ? `OpenAI-Compatible — ${cfg.name || providerId.slice('Compatible:'.length)}`
            : providerId;
          const visibleModels = cfg.models
            .map((modelId) => ({ providerId, modelId, providerName: cfg.name || null }))
            .filter(matches);
          if (visibleModels.length === 0) continue;
          listWrap.appendChild(el('div', { class: 'chat-model-dropdown__group' }, groupLabel));
          for (const m of visibleModels) renderItem(m);
        }
      } else {
        for (const provider of (aiConfig?.providers || []).filter((p) => p.enabled)) {
          if (!provider.default_model) continue;
          const m = { providerId: provider.provider_type, modelId: provider.default_model, providerName: null };
          if (!matches(m)) continue;
          listWrap.appendChild(el('div', { class: 'chat-model-dropdown__group' }, provider.provider_type));
          renderItem(m);
        }
      }

      if (listWrap.childElementCount === 0) {
        listWrap.appendChild(el('div', { class: 'chat-model-dropdown__empty' }, 'No models match'));
      }
    }

    rerender('');
    searchInput.addEventListener('input', () => rerender(searchInput.value));
    searchInput.addEventListener('keydown', (ev) => {
      if (ev.key === 'Escape') {
        ev.stopPropagation();
        closeModelDropdown();
      }
    });
    setTimeout(() => searchInput.focus(), 0);

    const rect = modelBtn.getBoundingClientRect();
    const availableHeight = Math.max(220, rect.top - 12);
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

  // "Stop & send" — declared up front so updateSendBtn() and the click
  // handler can reference it during component init without hitting TDZ.
  // The actual placement into the toolbar happens further below where the
  // toolbar element is built; this just creates the node + sets initial
  // hidden state.
  const stopSendBtn = el('button', {
    class: 'chat-stop-send-btn',
    title: 'Stop the current turn and send this message immediately.',
    type: 'button',
  }, 'Stop & send');
  stopSendBtn.style.display = 'none';

  // Send button has three modes: 'send' (idle), 'stop' (Running, no input),
  // 'queue' (Running, has input). Tracked here so we can avoid a full DOM
  // rebuild when the mode hasn't changed.
  let sendBtnMode = 'send';
  // Backwards-compat for older code paths still reading the boolean.
  let sendBtnIsStop = false;

  /// Inspect the current chat / workspace / provider state and return either
  /// null (Send is allowed) or a short reason string (Send is disabled, used
  /// as the button's tooltip + the welcome card's empty-state CTA copy).
  function getSendBlockReason() {
    if (!hasAnyConnectedProvider()) {
      return 'Connect an AI provider in Settings to start chatting.';
    }
    const taskId = agentStore.getState('activeTaskId');
    if (taskId) {
      // Inside an existing chat — sending is always allowed; the per-task
      // provider has already been picked at create time.
      return null;
    }
    // Welcome card. A pending project (or Global) must be picked.
    const pending = agentStore.getState('pendingProjectId');
    if (!pending) {
      const projects = workspaceStore.getState('projects') || [];
      if (projects.filter((p) => p.id !== GLOBAL_PROJECT_ID).length === 0) {
        return 'Add a project from the Explorer to start a chat.';
      }
      return 'Pick a project (or Global) to start a chat.';
    }
    return null;
  }

  function hasInputContent() {
    return textarea.value.trim().length > 0
      || attachedFiles.length > 0
      || attachedTags.length > 0
      || pasteChips.length > 0;
  }

  // After a revert, drop the reverted message text back into the composer so
  // the user can edit and resend it without retyping. We only do this when
  // the input is genuinely empty (no text, files, tags, or paste chips) —
  // otherwise the user already had a draft going and shouldn't have it
  // clobbered.
  function prefillInputIfEmpty(text) {
    if (!text || hasInputContent()) return;
    textarea.value = text;
    autoResizeTextarea();
    updateSendBtn();
    textarea.focus();
    try { textarea.setSelectionRange(text.length, text.length); } catch { /* ignore */ }
  }

  function updateSendBtn() {
    const taskId = agentStore.getState('activeTaskId');
    const task = taskId ? agentStore.getState('tasks')[taskId] : null;
    const isRunning = task?.status === 'Running' || (task?.isStreaming === true && task?.status !== 'WaitingForInput');
    const isWaiting = task?.status === 'WaitingForInput';
    // Update textarea placeholder based on state
    textarea.placeholder = isWaiting
      ? 'Type your response...'
      : isRunning
        ? 'Agent is running... (type to interrupt)'
        : `Send a message${mediaToolsHint}`;

    // Mid-turn steering (plan §14): when the task is Running and the user
    // has typed something, the primary button morphs into "Send (interrupt)"
    // — clicking it aborts the current turn and fires the new message as a
    // fresh turn (queue acts as a brief buffer in case multiple sends stack
    // before the abort lands). Empty input keeps the Stop semantic so an
    // idle Enter doesn't fire a blank turn. Mode key is still 'queue' for
    // continuity; the behavior just shifted from passive-wait to interrupt.
    const inputHasContent = hasInputContent();
    const mode = !isRunning ? 'send' : (inputHasContent ? 'queue' : 'stop');

    // Reflect blocking conditions (no provider / no project) on the button.
    // Skip while a task is running so Stop / Queue stays clickable.
    const blockReason = isRunning ? null : getSendBlockReason();
    sendBtn.disabled = !!blockReason;
    sendBtn.classList.toggle('chat-send-btn--blocked', !!blockReason);

    // "Stop & send" surfaces whenever the task is running and the user has
    // typed a follow-up. Both harness and native paths now persist the
    // partial assistant text on cancel (executor.rs + harness_runtime.rs),
    // so the queued message lands as the next turn with a coherent history.
    stopSendBtn.style.display =
      isRunning && inputHasContent ? '' : 'none';

    sendBtnIsStop = isRunning && mode === 'stop';

    if (mode === sendBtnMode) {
      // Mode unchanged — just refresh the title in case input-content
      // toggled within the same mode (won't happen here but keeps it tidy).
      sendBtn.title = blockReason || titleForMode(mode);
      return;
    }
    sendBtnMode = mode;

    sendBtn.innerHTML = '';
    sendBtn.classList.toggle('chat-send-btn--stop', mode === 'stop');
    sendBtn.classList.toggle('chat-send-btn--queue', mode === 'queue');
    sendBtn.title = blockReason || titleForMode(mode);

    if (mode === 'stop') {
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
    } else if (mode === 'queue') {
      // Down-arrow into a tray — visually distinct from both Send (paper
      // plane) and Stop (square). Hover tooltip explains the behavior.
      sendBtn.appendChild(icon('M12 4v12m0 0l-4-4m4 4l4-4M4 20h16', 15));
    } else {
      sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 15));
    }
  }

  function titleForMode(mode) {
    if (mode === 'stop') return 'Stop task';
    if (mode === 'queue') return 'Send now — interrupts the current turn and fires this as a new turn. Stacks if you type more before the abort lands.';
    return 'Send';
  }

  function getContextWindow(model) {
    if (!model) return 200000;
    // Claude Code aliases (subscription harness). The CLI uses 1M-context
    // mode by default for Sonnet/Opus on Pro/Max plans; Haiku stays 200K.
    // Match these *before* the substring checks below so the bare aliases
    // don't fall through to the 128K default.
    if (model === 'opus' || model === 'sonnet') return 1000000;
    if (model === 'haiku') return 200000;
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

  const draftStore = new Map();

  // Paste chip state. Ids come from `nextPasteChipId()` (smallest free
  // positive integer) so removing #2 and pasting again brings the new chip
  // back to #2 instead of bumping the counter forward.
  const pastedTexts = new Map();
  let pasteChips = []; // Array of { id, text, el }

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

  const pasteChipsContainer = el('div', { class: 'chat-paste-chips' });
  pasteChipsContainer.style.display = 'none';

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

  function saveDraft(taskId) {
    if (!taskId) return;
    draftStore.set(taskId, {
      text: textarea.value,
      attachedFiles: attachedFiles.slice(),
      attachedTags: attachedTags.slice(),
      pasteChips: pasteChips.slice(),
    });
  }

  function restoreDraft(taskId) {
    const draft = taskId ? draftStore.get(taskId) : null;
    if (draft) {
      textarea.value = draft.text || '';
      attachedFiles = draft.attachedFiles || [];
      attachedTags = draft.attachedTags || [];
      pasteChips = draft.pasteChips || [];
      for (const chip of pasteChips) { pastedTexts.set(chip.id, chip.text); }
    } else {
      textarea.value = '';
      attachedFiles = [];
      attachedTags = [];
      pasteChips = [];
      pastedTexts.clear();
    }
    autoResizeTextarea();
    renderAttachmentPills();
    renderTagChips();
    renderPasteChips();
    updateSendBtn();
  }

  function renderPasteChips() {
    pasteChipsContainer.innerHTML = '';
    if (pasteChips.length === 0) {
      pasteChipsContainer.style.display = 'none';
      return;
    }
    pasteChipsContainer.style.display = 'flex';
    for (let i = 0; i < pasteChips.length; i++) {
      const chip = pasteChips[i];
      const chipEl = el('div', { class: 'paste-chip', title: chip.text.slice(0, 120) });
      chipEl.appendChild(el('span', { class: 'paste-chip__icon' }, '\uD83D\uDCCB'));
      chipEl.appendChild(el('span', { class: 'paste-chip__label' }, `Pasted text #${chip.id}`));
      const removeBtn = el('button', { class: 'paste-chip__remove' }, '\xd7');
      const idx = i;
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        pastedTexts.delete(pasteChips[idx].id);
        pasteChips.splice(idx, 1);
        renderPasteChips();
      });
      chipEl.appendChild(removeBtn);
      chipEl.addEventListener('click', async () => {
        try {
          const info = await api.openScratchBuffer(`Pasted text #${chip.id}`, chip.text, 'text');
          if (!info) return;
          const { editorStore, setActiveBuffer } = await import('../../state/editor.js');
          const buf = { id: info.id, filePath: info.file_path, fileName: info.file_name, projectName: '', lineCount: info.line_count, language: info.language, isModified: false, fileType: 'code', isPreview: false, isDualMode: false, viewMode: 'edit' };
          editorStore.setState({ openBuffers: { ...editorStore.getState('openBuffers'), [info.id]: buf } });
          setActiveBuffer(info.id);
        } catch (err) {
          console.error('Failed to open pasted text in editor:', err);
        }
      });
      chip.el = chipEl;
      pasteChipsContainer.appendChild(chipEl);
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

  // Persist any in-memory image attachments (pasted into the textarea) to the
  // project at `.rustic/uploaded/<task>/<timestamp>-<name>` so the agent can
  // reference them by path from `image_create` (image_paths), `video_create`,
  // and `animate` (image_path). Returns the saved project-relative paths in
  // input order. The pixel bytes still travel inline as `images` so the model
  // can see the content — the on-disk copy is purely so the model has a stable
  // path to pass back to its media tools.
  async function persistAttachedImagesAsFiles(taskId) {
    if (!taskId) return [];
    const projectRoot = getTaskProjectRoot(taskId);
    if (!projectRoot) return [];
    const targets = attachedFiles.filter((f) => f.base64 && f.type && f.type.startsWith('image/'));
    if (!targets.length) return [];

    const ts = new Date();
    const pad = (n) => String(n).padStart(2, '0');
    const stamp = `${ts.getFullYear()}${pad(ts.getMonth() + 1)}${pad(ts.getDate())}-${pad(ts.getHours())}${pad(ts.getMinutes())}${pad(ts.getSeconds())}`;
    const sep = projectRoot.includes('\\') && !projectRoot.includes('/') ? '\\' : '/';
    const trimmedRoot = projectRoot.replace(/[\\/]+$/, '');
    const safeTask = String(taskId).replace(/[^a-zA-Z0-9_\-]/g, '_');
    const saved = [];

    for (let i = 0; i < targets.length; i++) {
      const f = targets[i];
      const rawName = (f.name || `pasted-image-${i + 1}`).replace(/[^a-zA-Z0-9_\-.]/g, '_');
      const ext = (f.type.split('/')[1] || 'png').toLowerCase();
      const stem = rawName.replace(/\.[^.]+$/, '') || `pasted-${i + 1}`;
      const filename = `${stamp}-${i + 1}-${stem}.${ext}`;
      const relPath = `.rustic/uploaded/${safeTask}/${filename}`;
      const absPath = `${trimmedRoot}${sep}${relPath.split('/').join(sep)}`;
      try {
        await api.writeFileBase64(absPath, f.base64);
        saved.push(relPath);
      } catch (err) {
        console.warn('persistAttachedImagesAsFiles: failed to save', f.name, err);
      }
    }
    return saved;
  }

  // Build a system-style note the model sees, listing where the pasted
  // attachments were saved and how to feed them back through media tools.
  // Returns '' when nothing was persisted so callers don't need to branch.
  function buildAttachmentNote(savedPaths) {
    if (!savedPaths || !savedPaths.length) return '';
    const lines = savedPaths.map((p) => `- ${p}`).join('\n');
    return `\n\n<attached-images>\nThe user attached ${savedPaths.length} image${savedPaths.length === 1 ? '' : 's'}. They are saved at these project-relative paths and are also provided inline below:\n${lines}\n\nIf the user wants to edit, iterate on, or animate these images, pass the path(s) above as \`image_paths\` to \`image_create\` (image editing / image-to-image) or as \`image_path\` to \`video_create\` / \`animate\`.\n</attached-images>`;
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
  projectBtn.appendChild(projectBtnLabel);

  let projectPickerOverlay = null;

  function closeProjectPicker() {
    if (projectPickerOverlay) {
      if (projectPickerOverlay.__rusticEsc) {
        document.removeEventListener('keydown', projectPickerOverlay.__rusticEsc);
      }
      projectPickerOverlay.remove();
      projectPickerOverlay = null;
    }
  }

  function openProjectPicker() {
    closeProjectPicker();

    const currentId = getCurrentProjectId();
    const projects = workspaceStore.getState('projects');

    // Subscription harnesses (Claude Code / Codex) scope their session by
    // working directory — the CLI looks for `.claude/` or `.codex/` under
    // cwd, so a Global "no project" chat would dump session state into the
    // wrong place. Lock the Global option in that case rather than letting
    // the user pick something the backend will reject downstream.
    //
    // Read the provider straight from the active task / pending choice
    // instead of trying to reverse-engineer it from the model id —
    // findOwningProvider's heuristics will mis-classify things like a bare
    // `opus` alias or any model that exists under multiple configured
    // providers.
    const ownerProviderId = getCurrentProviderType();
    const harnessLocked = ownerProviderId === 'ClaudeCode' || ownerProviderId === 'Codex';

    const overlay = el('div', { class: 'project-picker-overlay' });
    const modal   = el('div', { class: 'project-picker-modal' });
    overlay.appendChild(modal);

    overlay.addEventListener('click', (ev) => {
      if (ev.target === overlay) closeProjectPicker();
    });
    modal.addEventListener('click', (ev) => ev.stopPropagation());

    const onKey = (ev) => { if (ev.key === 'Escape') { ev.stopPropagation(); closeProjectPicker(); } };
    document.addEventListener('keydown', onKey);
    overlay.__rusticEsc = onKey;

    // Header
    const header = el('div', { class: 'project-picker__header' });
    header.appendChild(el('h2', { class: 'project-picker__title' }, 'Select project'));
    const closeBtn = el('button', { class: 'project-picker__close', title: 'Close (Esc)' });
    closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 14));
    closeBtn.addEventListener('click', (ev) => { ev.stopPropagation(); closeProjectPicker(); });
    header.appendChild(closeBtn);
    modal.appendChild(header);

    // Body
    const body = el('div', { class: 'project-picker__body' });

    // ── Global / Context scope ─────────────────────────────
    body.appendChild(el('div', { class: 'project-picker__section-label' }, 'Context'));

    const globalActive = currentId === GLOBAL_PROJECT_ID && !harnessLocked;
    // Build attrs as an object so `disabled` can be omitted entirely when
    // unlocked. Passing `disabled: null` to el() still calls setAttribute,
    // which writes the literal string "null" — and the browser treats *any*
    // value of `disabled` as on, so the button stops accepting clicks.
    const globalAttrs = {
      class: `project-picker__row${globalActive ? ' project-picker__row--active' : ''}${harnessLocked ? ' project-picker__row--disabled' : ''}`,
      type: 'button',
      title: harnessLocked
        ? 'Disabled — Claude Code and Codex need a real project root.'
        : 'Orchestrator scope across all projects.',
    };
    if (harnessLocked) globalAttrs.disabled = 'true';
    const globalRow = el('button', globalAttrs);
    const globalIcon = el('span', { class: 'project-picker__row-icon' });
    // Globe (Heroicons "globe-alt")
    globalIcon.appendChild(icon('M21 12a9 9 0 11-18 0 9 9 0 0118 0z M3.6 9h16.8 M3.6 15h16.8 M11.5 3a17 17 0 000 18 M12.5 3a17 17 0 010 18', 16));
    globalRow.appendChild(globalIcon);

    const globalText = el('div', { class: 'project-picker__row-text' });
    globalText.appendChild(el('div', { class: 'project-picker__row-label' }, 'Global'));
    globalText.appendChild(el('div', { class: 'project-picker__row-desc' },
      harnessLocked
        ? `Locked because ${ownerProviderId === 'Codex' ? 'Codex' : 'Claude Code'} is selected — switch model first to enable Global.`
        : 'Read across every project, spawn sub-tasks, run orchestrator workflows.'));
    globalRow.appendChild(globalText);

    if (harnessLocked) {
      const lock = el('span', { class: 'project-picker__lock', title: 'Disabled' });
      // Padlock
      lock.appendChild(icon('M5 11h14a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2z M8 11V7a4 4 0 018 0v4', 14));
      globalRow.appendChild(lock);
    } else if (globalActive) {
      globalRow.appendChild(el('span', { class: 'project-picker__check' }, (() => {
        const s = el('span', {});
        s.appendChild(icon('M5 13l4 4L19 7', 14));
        return s;
      })()));
    }

    if (!harnessLocked) {
      globalRow.addEventListener('click', (ev) => {
        ev.stopPropagation();
        setPendingProjectId(GLOBAL_PROJECT_ID);
        closeProjectPicker();
      });
    }
    body.appendChild(globalRow);

    // ── Projects ───────────────────────────────────────────
    body.appendChild(el('div', { class: 'project-picker__section-label' }, 'Projects'));

    if (projects.length === 0) {
      body.appendChild(el('div', { class: 'project-picker__empty' }, 'No projects open. Add one from the Explorer to scope a chat to it.'));
    } else {
      const list = el('div', { class: 'project-picker__list' });
      for (const project of projects) {
        const isActive = String(currentId) === String(project.id);
        const row = el('button', {
          class: `project-picker__row${isActive ? ' project-picker__row--active' : ''}`,
          type: 'button',
          title: project.root_path || project.name,
        });
        const folderIcon = el('span', { class: 'project-picker__row-icon' });
        folderIcon.appendChild(icon('M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z', 16));
        row.appendChild(folderIcon);

        const text = el('div', { class: 'project-picker__row-text' });
        text.appendChild(el('div', { class: 'project-picker__row-label' }, project.name));
        if (project.root_path) {
          text.appendChild(el('div', { class: 'project-picker__row-desc project-picker__row-desc--mono' }, project.root_path));
        }
        row.appendChild(text);

        if (isActive) {
          const check = el('span', { class: 'project-picker__check' });
          check.appendChild(icon('M5 13l4 4L19 7', 14));
          row.appendChild(check);
        }

        row.addEventListener('click', (ev) => {
          ev.stopPropagation();
          setPendingProjectId(project.id);
          closeProjectPicker();
        });
        list.appendChild(row);
      }
      body.appendChild(list);
    }

    modal.appendChild(body);
    document.body.appendChild(overlay);
    projectPickerOverlay = overlay;
  }

  projectBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (projectPickerOverlay) { closeProjectPicker(); return; }
    if (agentStore.getState('activeTaskId')) return; // read-only when a task is active
    openProjectPicker();
  });

  function updateProjectBtn() {
    const currentId = getCurrentProjectId();
    projectBtnLabel.textContent = projectLabelFor(currentId);
    const readonly = !!agentStore.getState('activeTaskId');
    projectBtn.classList.toggle('chat-project-pill--readonly', readonly);
  }

  let callConfigOpen = false;
  let callConfigOverlay = null;          // outer modal-overlay element
  let callConfigModal = null;            // inner modal-card (rebuild target)
  let callConfigSelectedProvider = null; // currently focused provider id in the rail
  // Which tab is active in the model/tools picker. Persists across modal opens
  // so a user who frequently swaps tool models doesn't have to re-click Tools
  // every time they reopen the picker.
  let callConfigActiveTab = 'model';
  // In-memory snapshot of the user's ToolConfig + sub-agent config. Loaded
  // lazily when the Tools tab is first shown so opening the Model tab does
  // not pay for the round-trip. Mutated locally on each change and pushed to
  // the backend via setToolConfig / setSubagentConfig.
  let callConfigToolsState = null;
  let callConfigToolsLoading = false;
  // Per-(provider, include_all) model-list cache for the Tools-tab pickers.
  // Mirrors the cache in tool-settings.js but lives here so the modal can
  // refresh without re-fetching whenever the tab is re-entered.
  const callConfigToolModelCache = Object.create(null);
  // Combobox handles registered by the Tools tab so model lists can refresh
  // in place once the async fetchAiModels call resolves.
  const callConfigToolRefreshes = new Set();

  function closeCallConfig() {
    if (callConfigOverlay) {
      if (callConfigOverlay.__rusticEsc) {
        document.removeEventListener('keydown', callConfigOverlay.__rusticEsc);
      }
      callConfigOverlay.remove();
      callConfigOverlay = null;
      callConfigModal = null;
      callConfigOpen = false;
    }
  }

  // Registry-style fallback model lists. Used when a provider has no API
  // key yet (so cfg.models is empty) or to expose a "not configured" tail
  // alongside cfg.models for known-good models the user hasn't registered.
  const PROVIDER_FALLBACK_MODELS = {
    Claude:     ['claude-opus-4-7', 'claude-sonnet-4-6', 'claude-haiku-4-5', 'claude-opus-4-6', 'claude-sonnet-4'],
    OpenAi:     ['gpt-5.4-pro', 'gpt-5.4', 'gpt-5.4-mini', 'gpt-5.4-nano', 'gpt-5-codex', 'o3', 'o4-mini'],
    Gemini:     ['gemini-3.1-pro', 'gemini-3.1-flash-lite', 'gemini-3-flash', 'gemini-2.5-pro', 'gemini-2.5-flash'],
    OpenRouter: [
      // Western reasoning flagships routed via OpenRouter
      'anthropic/claude-sonnet-4-5', 'anthropic/claude-opus-4-1',
      'openai/gpt-5.5', 'openai/gpt-5.4',
      'google/gemini-2.5-pro', 'google/gemini-2.5-flash',
      // DeepSeek
      'deepseek/deepseek-r1', 'deepseek/deepseek-v3.2',
      'deepseek/deepseek-v3.2-exp', 'deepseek/deepseek-chat-v3.1',
      'deepseek/deepseek-chat',
      // Moonshot Kimi
      'moonshotai/kimi-k2.6', 'moonshotai/kimi-k2-thinking',
      'moonshotai/kimi-k2-0905', 'moonshotai/kimi-k2',
      // Z.AI / Zhipu GLM
      'z-ai/glm-5.1', 'z-ai/glm-5',
      'z-ai/glm-4.7', 'z-ai/glm-4.6', 'z-ai/glm-4.5-air', 'z-ai/glm-4.5',
      // MiniMax
      'minimax/minimax-m2.7', 'minimax/minimax-m2.5',
      'minimax/minimax-m2', 'minimax/minimax-m1',
      // Xiaomi MiMo
      'xiaomi/mimo-v2.5-pro', 'xiaomi/mimo-v2.5',
      'xiaomi/mimo-v2-pro', 'xiaomi/mimo-v2-flash',
      // Alibaba Qwen
      'qwen/qwen3.6-max-preview', 'qwen/qwen3.6-plus', 'qwen/qwen3.6-flash',
      'qwen/qwen3-coder', 'qwen/qwen3-235b-a22b',
      'qwen/qwen-2.5-72b-instruct',
    ],
    ClaudeCode: ['opus', 'sonnet', 'haiku'],
    Codex:      ['gpt-5.3-codex', 'gpt-5-codex'],
  };

  const KNOWN_PROVIDERS = [
    { id: 'Claude',     label: 'Anthropic Claude' },
    { id: 'OpenAi',     label: 'OpenAI' },
    { id: 'Gemini',     label: 'Google Gemini' },
    { id: 'OpenRouter', label: 'OpenRouter' },
    { id: 'ClaudeCode', label: 'Claude Code (sub)' },
    { id: 'Codex',      label: 'Codex (sub)' },
  ];

  function buildProviderEntries() {
    const configs = loadProviderConfigs();
    const entries = [];
    for (const sp of KNOWN_PROVIDERS) {
      const cfg = configs[sp.id];
      entries.push({
        id: sp.id,
        label: sp.label,
        hasKey: !!cfg?.hasKey,
        models: cfg?.models || [],
        fallback: PROVIDER_FALLBACK_MODELS[sp.id] || [],
      });
    }
    for (const [k, cfg] of Object.entries(configs)) {
      if (!k.startsWith('Compatible:')) continue;
      entries.push({
        id: k,
        label: providerDisplayLabel(k, cfg),
        hasKey: !!cfg?.hasKey,
        models: cfg?.models || [],
        fallback: [],
      });
    }
    return entries;
  }

  function findOwningProvider(providers, modelId) {
    if (!modelId) return null;
    for (const p of providers) {
      if (p.models?.includes(modelId)) return p.id;
    }
    for (const p of providers) {
      if (p.fallback?.includes(modelId)) return p.id;
    }
    if (modelId.startsWith('claude-')) return 'Claude';
    if (modelId.startsWith('gpt-') || /^o\d/.test(modelId)) return 'OpenAi';
    if (modelId.startsWith('gemini-')) return 'Gemini';
    return null;
  }

  // Friendly label for a provider id in the model picker. Mirrors the
  // labels shown in Settings → AI Providers so the two views match.
  function providerDisplayLabel(providerId, cfg) {
    if (providerId.startsWith('Compatible:')) {
      const slug = providerId.slice('Compatible:'.length);
      return `OpenAI-Compatible — ${cfg?.name || slug}`;
    }
    switch (providerId) {
      case 'Claude':     return 'Anthropic Claude';
      case 'OpenAi':     return 'OpenAI';
      case 'Gemini':     return 'Google Gemini';
      case 'OpenRouter': return 'OpenRouter';
      case 'ClaudeCode': return 'Claude Code (subscription)';
      case 'Codex':      return 'Codex (subscription)';
      default:           return providerId;
    }
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
    if (!callConfigModal) return;

    // Capture scroll positions before wiping so picking a model (or any
    // other action that triggers a rebuild) doesn't snap the user back to
    // the top of the OpenAI list — they were probably scrolled halfway
    // down to find the row they just clicked.
    const prevModelsScroll    = callConfigModal.querySelector('.agent-config__models')?.scrollTop ?? 0;
    const prevProvidersScroll = callConfigModal.querySelector('.agent-config__providers')?.scrollTop ?? 0;

    callConfigModal.innerHTML = '';

    const taskId = agentStore.getState('activeTaskId');
    const currentModel = getCurrentModel();
    const isGlobal = getCurrentProjectId() === GLOBAL_PROJECT_ID;

    // ── Header ──────────────────────────────────────────────
    const header = el('div', { class: 'agent-config__header' });
    header.appendChild(el('h2', { class: 'agent-config__title' }, 'Agent Configuration'));
    const closeBtn = el('button', { class: 'agent-config__close', title: 'Close (Esc)' });
    closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 14));
    closeBtn.addEventListener('click', (ev) => { ev.stopPropagation(); closeCallConfig(); });
    header.appendChild(closeBtn);
    callConfigModal.appendChild(header);

    // ── Body ────────────────────────────────────────────────
    const body = el('div', { class: 'agent-config__body' });
    body.appendChild(renderModesSection(taskId));
    // Reset the per-modal combobox refresh registry — handles from the
    // previous build are stale once we replaceChildren the modal.
    callConfigToolRefreshes.clear();
    if (callConfigActiveTab === 'tools') {
      body.appendChild(renderToolsSection(taskId, currentModel));
    } else {
      body.appendChild(renderModelSection(taskId, currentModel, isGlobal));
    }
    callConfigModal.appendChild(body);

    // ── Footer (effort) — only shown on the Model tab; effort is per-model.
    if (callConfigActiveTab !== 'tools') {
      callConfigModal.appendChild(renderEffortFooter(currentModel));
    }

    // Restore the captured scroll positions on the freshly-built lists so
    // the rebuild is visually invisible to the user. Both panes have a
    // fixed height + overflow-y:auto, so scrollTop applies immediately
    // without needing a layout flush.
    const newModels    = callConfigModal.querySelector('.agent-config__models');
    const newProviders = callConfigModal.querySelector('.agent-config__providers');
    if (newModels)    newModels.scrollTop    = prevModelsScroll;
    if (newProviders) newProviders.scrollTop = prevProvidersScroll;
  }

  function renderModesSection(taskId) {
    const section = el('div', { class: 'agent-config__section' });
    section.appendChild(el('div', { class: 'agent-config__section-label' }, 'Mode'));

    const current = getCurrentMode();
    let activeKey = 'chat';
    if (current === 'AutoEdit' || current === 'ManualEdit') activeKey = 'edit';
    else if (current === 'FullAuto') activeKey = 'fullauto';

    async function applyMode(perm, sens) {
      if (!taskId) {
        setPendingPermissionLevel(perm);
        setPendingSensitiveAccess(sens);
        return true;
      }
      const ok = await setTaskPermissions(taskId, perm);
      if (!ok) return false;
      try { await setTaskSensitiveAccess(taskId, sens); } catch {}
      return true;
    }

    const modes = [
      {
        key: 'chat', label: 'Chat',
        desc: 'Read-only conversation. The agent answers and reads files but never writes or runs commands.',
        iconPath: 'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z',
        apply: () => applyMode('Chat', false),
      },
      {
        key: 'edit', label: 'Edit',
        desc: 'File edits apply automatically. Shell commands still pause for your approval before running.',
        iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z',
        apply: () => applyMode('AutoEdit', false),
      },
      {
        key: 'fullauto', label: 'Full Auto',
        desc: 'Everything runs unattended, including writes to .env, credentials, and gitignored paths.',
        iconPath: 'M13 10V3L4 14h7v7l9-11h-7z',
        apply: () => applyMode('FullAuto', true),
      },
    ];

    const slider = el('div', {
      class: `agent-config__mode-slider${activeKey === 'fullauto' ? ' agent-config__mode-slider--danger' : ''}`,
      'data-active': activeKey,
    });
    slider.appendChild(el('span', { class: 'agent-config__mode-thumb' }));

    const segs = {};
    for (const m of modes) {
      const seg = el('button', {
        class: `agent-config__mode-seg${activeKey === m.key ? ' agent-config__mode-seg--active' : ''}${m.key === 'fullauto' ? ' agent-config__mode-seg--danger' : ''}`,
        type: 'button',
      });
      const ic = el('span', { class: 'agent-config__mode-seg-icon' });
      ic.appendChild(icon(m.iconPath, 13));
      seg.appendChild(ic);
      seg.appendChild(el('span', { class: 'agent-config__mode-seg-label' }, m.label));
      segs[m.key] = seg;
      slider.appendChild(seg);
    }

    section.appendChild(slider);

    const active = modes.find((m) => m.key === activeKey) || modes[1];
    const caption = el('p', {
      class: `agent-config__mode-caption${activeKey === 'fullauto' ? ' agent-config__mode-caption--danger' : ''}`,
    }, active.desc);
    section.appendChild(caption);

    // Update the slider in place rather than calling rebuildCallConfigContent
    // — destroying/re-creating the thumb element on every click swaps in a
    // fresh node already at the destination position, so the browser has no
    // interpolation to do and the change reads as a snap. Mutating the live
    // element keeps the CSS transition intact and lets the thumb actually
    // glide between segments.
    function setActive(nextKey) {
      if (nextKey === activeKey) return;
      activeKey = nextKey;
      slider.setAttribute('data-active', nextKey);
      slider.classList.toggle('agent-config__mode-slider--danger', nextKey === 'fullauto');
      for (const k of Object.keys(segs)) {
        segs[k].classList.toggle('agent-config__mode-seg--active', k === nextKey);
      }
      const m = modes.find((mm) => mm.key === nextKey);
      if (m) {
        caption.textContent = m.desc;
        caption.classList.toggle('agent-config__mode-caption--danger', nextKey === 'fullauto');
      }
    }

    for (const m of modes) {
      segs[m.key].addEventListener('click', async (ev) => {
        ev.stopPropagation();
        if (activeKey === m.key) return;
        // Optimistic visual update so the slide starts immediately; rollback
        // if the backend rejects (e.g. switchPermissions returns false).
        const prev = activeKey;
        setActive(m.key);
        const ok = await m.apply();
        if (!ok) setActive(prev);
      });
    }

    return section;
  }

  function renderTabBar() {
    // Clickable tab strip used in place of the static "Model" section label.
    // Switching tabs rebuilds just the body — provider rail / model list on
    // Model, the quick-config rows on Tools. Stays clear of native button
    // styling so the look matches the rest of the agent-config modal.
    const bar = el('div', { class: 'agent-config__tabs' });
    const tabs = [
      { key: 'model', label: 'Model' },
      { key: 'tools', label: 'Tools' },
    ];
    for (const t of tabs) {
      const isActive = callConfigActiveTab === t.key;
      const btn = el('button', {
        class: `agent-config__tab${isActive ? ' agent-config__tab--active' : ''}`,
        type: 'button',
      }, t.label);
      btn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        if (callConfigActiveTab === t.key) return;
        callConfigActiveTab = t.key;
        rebuildCallConfigContent();
      });
      bar.appendChild(btn);
    }
    return bar;
  }

  function renderModelSection(taskId, currentModel, isGlobal) {
    const section = el('div', { class: 'agent-config__section' });
    section.appendChild(renderTabBar());

    const pane = el('div', { class: 'agent-config__model-pane' });
    const providers = buildProviderEntries();

    if (!callConfigSelectedProvider || !providers.find((p) => p.id === callConfigSelectedProvider)) {
      callConfigSelectedProvider = findOwningProvider(providers, currentModel)
        || providers.find((p) => p.hasKey && p.models.length)?.id
        || providers[0]?.id
        || null;
    }

    // Provider rail
    const rail = el('div', { class: 'agent-config__providers' });
    for (const p of providers) {
      const isHarness = p.id === 'ClaudeCode' || p.id === 'Codex';
      const harnessBlocked = isGlobal && isHarness;
      const isActive = p.id === callConfigSelectedProvider;
      const item = el('button', {
        class: `agent-config__provider${isActive ? ' agent-config__provider--active' : ''}${!p.hasKey ? ' agent-config__provider--unconfigured' : ''}${harnessBlocked ? ' agent-config__provider--blocked' : ''}`,
        title: harnessBlocked
          ? 'Subscription providers need a real project root — pick a project from the Explorer.'
          : (p.hasKey ? p.label : `${p.label} — not configured`),
      });
      item.appendChild(el('span', { class: `agent-config__provider-dot${p.hasKey ? ' agent-config__provider-dot--ok' : ''}` }));
      item.appendChild(el('span', { class: 'agent-config__provider-label' }, p.label));
      const count = p.hasKey && p.models.length
        ? el('span', { class: 'agent-config__provider-count' }, String(p.models.length))
        : el('span', { class: 'agent-config__provider-count agent-config__provider-count--off' }, p.hasKey ? '0' : 'off');
      item.appendChild(count);
      item.addEventListener('click', (ev) => {
        ev.stopPropagation();
        callConfigSelectedProvider = p.id;
        rebuildCallConfigContent();
      });
      rail.appendChild(item);
    }

    const addBtn = el('button', { class: 'agent-config__providers-add', title: 'Open AI provider settings' });
    addBtn.appendChild(icon('M12 5v14M5 12h14', 12));
    addBtn.appendChild(el('span', {}, 'Manage providers'));
    addBtn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      closeCallConfig();
      setSettingsCategory('agent');
      openSettings();
    });
    rail.appendChild(addBtn);
    pane.appendChild(rail);

    const modelColumn = el('div', { class: 'agent-config__model-column' });

    const modelSearch = el('input', {
      class: 'agent-config__model-search',
      type: 'text',
      placeholder: 'Filter models…',
      autocomplete: 'off',
      spellcheck: 'false',
    });
    modelColumn.appendChild(modelSearch);

    const list = el('div', { class: 'agent-config__models' });
    const selected = providers.find((p) => p.id === callConfigSelectedProvider) || providers[0];

    if (!selected) {
      modelSearch.style.display = 'none';
      list.appendChild(el('div', { class: 'agent-config__models-empty' }, 'No providers available.'));
    } else {
      const isHarness = selected.id === 'ClaudeCode' || selected.id === 'Codex';
      const harnessBlocked = isGlobal && isHarness;

      if (harnessBlocked) {
        modelSearch.style.display = 'none';
        const empty = el('div', { class: 'agent-config__models-empty' });
        empty.appendChild(el('div', { class: 'agent-config__models-empty-title' }, 'Project required'));
        empty.appendChild(el('div', { class: 'agent-config__models-empty-desc' },
          'Subscription harnesses scope their session by working directory. Pick a project from the Explorer to enable this provider.'));
        list.appendChild(empty);
      } else if (!selected.hasKey) {
        modelSearch.style.display = 'none';
        const empty = el('div', { class: 'agent-config__models-empty' });
        empty.appendChild(el('div', { class: 'agent-config__models-empty-title' }, `${selected.label} not connected`));
        empty.appendChild(el('div', { class: 'agent-config__models-empty-desc' },
          'Add an API key in Settings to unlock the models below.'));
        const cta = el('button', { class: 'btn btn--primary btn--sm' }, 'Open settings');
        cta.addEventListener('click', (ev) => {
          ev.stopPropagation();
          closeCallConfig();
          setSettingsCategory('agent');
          openSettings();
        });
        empty.appendChild(cta);
        list.appendChild(empty);
        for (const modelId of selected.fallback) {
          list.appendChild(buildModelRow(modelId, false, false, selected, taskId, currentModel));
        }
      } else {
        const seen = new Set();
        const ordered = [];
        // A model is "configured" once we know its pricing/spec - either via
        // the built-in registry (pricingFor) or a user-saved custom entry.
        // Without that, cost and context-window numbers are unknown so the
        // agent can't run it safely; we surface that with a "not configured"
        // badge and route the click through the register-model modal.
        for (const m of selected.models) {
          if (!seen.has(m)) {
            seen.add(m);
            const ready = !!pricingFor(m) || !!getCustomModel(m);
            ordered.push({ id: m, configured: ready });
          }
        }
        for (const m of selected.fallback) {
          if (!seen.has(m)) { seen.add(m); ordered.push({ id: m, configured: false }); }
        }
        if (!ordered.length) {
          modelSearch.style.display = 'none';
          list.appendChild(el('div', { class: 'agent-config__models-empty' }, 'No models - refresh the provider in Settings.'));
        } else {
          for (const m of ordered) {
            list.appendChild(buildModelRow(m.id, m.configured, m.id === currentModel, selected, taskId, currentModel));
          }

          let filterNoMatch = null;

          modelSearch.addEventListener('input', () => {
            const q = modelSearch.value.trim().toLowerCase();
            if (filterNoMatch) { filterNoMatch.remove(); filterNoMatch = null; }
            let anyVisible = false;
            for (const row of list.children) {
              const name = (row.querySelector('.agent-config__model-name')?.textContent || '').toLowerCase();
              const hidden = q.length > 0 && !name.includes(q);
              row.style.display = hidden ? 'none' : '';
              if (!hidden) anyVisible = true;
            }
            if (q.length > 0 && !anyVisible) {
              filterNoMatch = el('div', { class: 'agent-config__models-empty' }, 'No models match');
              list.appendChild(filterNoMatch);
            }
          });

          modelSearch.addEventListener('keydown', (ev) => {
            if (ev.key === 'Escape') { ev.stopPropagation(); closeCallConfig(); }
          });
        }
      }
    }

    modelColumn.appendChild(list);
    pane.appendChild(modelColumn);
    section.appendChild(pane);
    return section;
  }

  function buildModelRow(modelId, isConfigured, isActive, providerEntry, taskId, currentModel) {
    const currentProvider = taskId ? getCurrentProviderType() : '';
    const locked = !!taskId && !canSwitchTo(currentProvider, providerEntry.id);
    const dimmed = !isConfigured || !providerEntry.hasKey || locked;
    const row = el('button', {
      class: `agent-config__model${isActive ? ' agent-config__model--active' : ''}${dimmed ? ' agent-config__model--unconfigured' : ''}`,
      title: locked
        ? (isHarnessProvider(currentProvider)
          ? `Locked — this chat started on ${currentProvider}; start a new chat to use ${providerEntry.id}.`
          : `Locked — this chat uses an API provider; start a new chat to use ${providerEntry.id}.`)
        : modelId,
    });
    row.appendChild(el('span', { class: 'agent-config__model-name' }, modelId));

    if (isActive) {
      const check = el('span', { class: 'agent-config__model-check' });
      check.appendChild(icon('M5 13l4 4L19 7', 12));
      row.appendChild(check);
    } else if (locked) {
      row.appendChild(el('span', { class: 'agent-config__model-badge' }, 'locked'));
    } else if (dimmed) {
      row.appendChild(el('span', { class: 'agent-config__model-badge' }, 'not configured'));
    } else {
      const p = pricingFor(modelId);
      if (p) row.appendChild(el('span', { class: 'agent-config__model-meta' }, `$${p.input}/$${p.output} per 1M`));
    }

    // Per-model Edit button — only shown for already-configured (custom)
    // models so the user can re-tune cost / capabilities (e.g. flip
    // "supports temperature" off for Claude Opus 4.7 on a Compatible host).
    // Using a span+role=button because the row itself is a <button> and
    // nested <button>s are invalid HTML; the click handler stops
    // propagation so it doesn't also fire the row's "switch model" action.
    if (isConfigured && getCustomModel(modelId)) {
      const editBtn = el('span', {
        class: 'agent-config__model-edit',
        role: 'button',
        tabindex: '0',
        title: 'Edit model spec & capabilities',
        'aria-label': `Edit ${modelId}`,
      });
      editBtn.appendChild(icon('M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', 12));
      const openEditModal = (ev) => {
        ev.stopPropagation();
        ev.preventDefault();
        const providerType = providerEntry.id.startsWith('Compatible:') ? 'Compatible' : providerEntry.id;
        const savedProviderId = callConfigSelectedProvider;
        closeCallConfig();
        openCustomModelModal({
          modelId,
          providerType,
          onSaved: () => {
            callConfigSelectedProvider = savedProviderId;
            openCallConfig();
          },
          onCancelled: () => {
            callConfigSelectedProvider = savedProviderId;
            openCallConfig();
          },
        });
      };
      editBtn.addEventListener('click', openEditModal);
      editBtn.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter' || ev.key === ' ') openEditModal(ev);
      });
      row.appendChild(editBtn);
    }

    row.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      if (isActive) return;
      if (locked) return;

      if (!providerEntry.hasKey) {
        closeCallConfig();
        setSettingsCategory('agent');
        openSettings();
        return;
      }

      if (!isConfigured) {
        // Close the agent-config modal first — the register-model modal uses
        // a lower z-index stack and would otherwise render *behind* us.
        // We re-open after save/cancel so the user lands back where they
        // were, on the same provider rail entry.
        const providerType = providerEntry.id.startsWith('Compatible:') ? 'Compatible' : providerEntry.id;
        const savedProviderId = callConfigSelectedProvider;
        closeCallConfig();
        openCustomModelModal({
          modelId,
          providerType,
          onSaved: () => {
            callConfigSelectedProvider = savedProviderId;
            openCallConfig();
          },
          onCancelled: () => {
            callConfigSelectedProvider = savedProviderId;
            openCallConfig();
          },
        });
        return;
      }

      if (!(await pickModel(providerEntry.id, modelId))) return;
      if (taskId) {
        try {
          saveThinkingForModel(currentModel);
          await api.switchModel(taskId, providerEntry.id, modelId);
          restoreThinkingForModel(modelId);
        } catch (err) {
          console.error('Failed to switch model:', err);
        }
      } else {
        setPendingModelChoice({ providerId: providerEntry.id, modelId });
      }
      updateCallConfigBtn();
      rebuildCallConfigContent();
    });

    return row;
  }

  // ── Tools tab ──────────────────────────────────────────────────────────────
  // Quick-pickers for the agent's non-chat models: the sub-agent fast model,
  // the web-search backend, and the three media tools (image / video /
  // animate). The provider list is drawn from the user's existing
  // ProviderEntries so they don't have to re-enter credentials — only the
  // provider id + model id (+ a couple of toggles) are tweakable here.
  // Heavier knobs (per-tool max-per-call, web-search API key) live in
  // Settings → Tools to keep this surface compact.

  /// Provider-types each tool is allowed to use. Mirrors the lists in
  /// tool-settings.js for the media tools and adds Claude for the sub-agent.
  const TOOLS_TAB_TOOLS = [
    {
      key: 'subagent',
      title: 'Sub-agent (fast model)',
      hint: 'Cheaper model used when the main agent spawns a sub-agent with model_tier=fast.',
      providers: ['Claude', 'OpenAi', 'Gemini', 'OpenRouter', 'Compatible'],
      includeAllModels: false,
    },
    {
      key: 'image',
      title: 'Image creator',
      hint: 'image_create tool — generates one or more images from a prompt.',
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
      includeAllModels: true,
    },
    {
      key: 'video',
      title: 'Video creator',
      hint: 'video_create tool — generates a short video from a prompt or first-frame image.',
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
      includeAllModels: true,
    },
    {
      key: 'animate',
      title: 'Animator (image → video)',
      hint: 'animate tool — turns an existing image into a short video clip.',
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
      includeAllModels: true,
    },
  ];

  function renderToolsSection(taskId, currentModel) {
    const section = el('div', { class: 'agent-config__section' });
    section.appendChild(renderTabBar());

    const body = el('div', { class: 'agent-config__tools-body' });

    if (callConfigToolsState == null) {
      if (!callConfigToolsLoading) {
        callConfigToolsLoading = true;
        Promise.all([api.getToolConfig(), api.getAiConfig()])
          .then(([toolCfg, aiCfg]) => {
            callConfigToolsState = {
              toolConfig: normalizeToolConfig(toolCfg),
              aiConfig: aiCfg || { providers: [] },
            };
            callConfigToolsLoading = false;
            if (callConfigActiveTab === 'tools') rebuildCallConfigContent();
          })
          .catch((err) => {
            callConfigToolsLoading = false;
            console.warn('Tools tab: failed to load config', err);
          });
      }
      body.appendChild(el('div', { class: 'agent-config__tools-loading' }, 'Loading…'));
      section.appendChild(body);
      return section;
    }

    body.appendChild(renderWebSearchRow());
    for (const tool of TOOLS_TAB_TOOLS) {
      body.appendChild(renderQuickToolRow(tool));
    }
    body.appendChild(renderToolsFooter());
    section.appendChild(body);
    return section;
  }

  /// Default-fill a backend-supplied ToolConfig so the renderer doesn't have
  /// to guard every read with `?.`.
  function normalizeToolConfig(raw) {
    const r = raw || {};
    const media = r.media || {};
    const entry = (m) => ({
      provider_key: m?.provider_key || '',
      model: m?.model || '',
      max_per_call: m?.max_per_call || 1,
    });
    return {
      web_search: {
        enabled: !!r.web_search?.enabled,
        backend: r.web_search?.backend || 'Tavily',
        api_key: r.web_search?.api_key || '',
      },
      web_fetch: { enabled: r.web_fetch?.enabled !== false },
      media: {
        image: entry(media.image),
        video: entry(media.video),
        animate: entry(media.animate),
        link_animate_to_video: !!media.link_animate_to_video,
      },
    };
  }

  /// Returns the list of {provider_key, label, hasKey} the user can pick from
  /// for a given tool. Filtered by the tool's allowed provider-types and
  /// drawn from the actual configured providers so keys / base_urls are
  /// implicit — the user isn't double-entering credentials.
  function providerChoicesFor(tool) {
    const aiCfg = callConfigToolsState?.aiConfig;
    if (!aiCfg) return [];
    const allowed = new Set(tool.providers);
    const choices = [];
    for (const p of aiCfg.providers || []) {
      const type = p.provider_type;
      if (!allowed.has(type)) continue;
      const key = providerKeyOfBackendEntry(p);
      const hasKey = !!p.api_key; // backend redacts to "__STORED__" when present
      choices.push({
        value: key,
        label: providerDisplayLabel(key, p),
        hint: hasKey ? '' : 'no api key',
        disabled: !hasKey,
      });
    }
    return choices;
  }

  function providerKeyOfBackendEntry(p) {
    if (p.provider_type === 'Compatible') {
      const slug = String(p.name || '')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '');
      return slug ? `Compatible:${slug}` : 'Compatible';
    }
    return p.provider_type;
  }

  function effectiveAnimateEntry() {
    const m = callConfigToolsState.toolConfig.media;
    return m.link_animate_to_video ? m.video : m.animate;
  }

  function getToolEntry(toolKey) {
    if (toolKey === 'subagent') {
      const sub = callConfigToolsState.aiConfig.subagent || null;
      return {
        provider_key: sub?.provider_key || '',
        model: sub?.model || '',
      };
    }
    if (toolKey === 'animate' && callConfigToolsState.toolConfig.media.link_animate_to_video) {
      return effectiveAnimateEntry();
    }
    return callConfigToolsState.toolConfig.media[toolKey];
  }

  /// Persist a tool's provider+model change. Routes to the right backend
  /// command for sub-agent vs media tools, then refreshes the in-memory
  /// snapshot so the next render uses the new values.
  async function commitToolEntry(toolKey, providerKey, model) {
    try {
      if (toolKey === 'subagent') {
        if (!providerKey || !model) {
          await api.clearSubagentConfig();
          callConfigToolsState.aiConfig.subagent = null;
        } else {
          await api.setSubagentConfig(providerKey, model);
          callConfigToolsState.aiConfig.subagent = { provider_key: providerKey, model };
        }
        return;
      }
      // Media tools: mutate the in-memory ToolConfig and push the whole thing.
      const media = callConfigToolsState.toolConfig.media;
      if (toolKey === 'animate' && media.link_animate_to_video) {
        // Link is on — animate isn't an independent slot, push into video.
        media.video.provider_key = providerKey;
        media.video.model = model;
      } else {
        media[toolKey].provider_key = providerKey;
        media[toolKey].model = model;
      }
      await api.setToolConfig(callConfigToolsState.toolConfig);
      // Mirror localStorage so the Settings panel and chat placeholder stay in sync.
      try {
        localStorage.setItem('rustic_tool_config', JSON.stringify(callConfigToolsState.toolConfig));
        window.dispatchEvent(new StorageEvent('storage', {
          key: 'rustic_tool_config',
          newValue: JSON.stringify(callConfigToolsState.toolConfig),
        }));
      } catch { /* ignore */ }
    } catch (err) {
      console.error(`Tools tab: failed to save ${toolKey}:`, err);
    }
  }

  /// Compact two-column row used for each tool entry: title + small hint on
  /// the left, [provider combobox][model combobox] on the right. The model
  /// list is populated by fetchAiModels once the user picks a provider.
  function renderQuickToolRow(tool) {
    const entry = getToolEntry(tool.key);
    const row = el('div', { class: 'agent-config__tool-row' });

    const head = el('div', { class: 'agent-config__tool-head' });
    head.appendChild(el('div', { class: 'agent-config__tool-title' }, tool.title));
    head.appendChild(el('div', { class: 'agent-config__tool-hint' }, tool.hint));

    // For the animator, expose the link-to-video toggle inline so the user
    // can decide whether this row is independently editable.
    let linkChecked = false;
    if (tool.key === 'animate') {
      linkChecked = !!callConfigToolsState.toolConfig.media.link_animate_to_video;
      const linkWrap = el('label', { class: 'agent-config__tool-link', title: 'Reuse the Video creator\'s provider + model.' });
      const linkBox = el('input', { type: 'checkbox' });
      if (linkChecked) linkBox.checked = true;
      linkBox.addEventListener('change', async () => {
        callConfigToolsState.toolConfig.media.link_animate_to_video = linkBox.checked;
        try {
          await api.setToolConfig(callConfigToolsState.toolConfig);
          localStorage.setItem('rustic_tool_config', JSON.stringify(callConfigToolsState.toolConfig));
        } catch (e) { console.warn(e); }
        rebuildCallConfigContent();
      });
      linkWrap.appendChild(linkBox);
      linkWrap.appendChild(el('span', {}, 'Link to video'));
      head.appendChild(linkWrap);
    }
    row.appendChild(head);

    const controls = el('div', { class: 'agent-config__tool-controls' });

    // Provider combobox
    const providerCombo = createCombobox({
      initialValue: entry.provider_key || '',
      placeholder: 'Provider…',
      allowCustom: false,
      getOptions: () => providerChoicesFor(tool),
      onChange: (newKey) => {
        // Wipe the model when provider changes so the user picks fresh.
        commitToolEntry(tool.key, newKey, '');
        modelCombo.setValue('');
        modelCombo.setDisabled(!newKey);
        if (newKey) loadToolModelsForRow(newKey, tool.includeAllModels);
      },
    });
    controls.appendChild(providerCombo.root);

    // Model combobox
    const modelCombo = createCombobox({
      initialValue: entry.model || '',
      placeholder: 'Type to search models…',
      allowCustom: true,
      getOptions: () => {
        const pk = (callConfigToolsState ? getToolEntry(tool.key).provider_key : '') || '';
        if (!pk) return [{ value: '', label: 'Pick a provider first', disabled: true }];
        const cacheKey = pk + '|' + (tool.includeAllModels ? '1' : '0');
        const cached = callConfigToolModelCache[cacheKey];
        if (!cached || cached.state === 'idle') {
          return [];
        }
        if (cached.state === 'loading') {
          return [{ value: '', label: 'Loading…', disabled: true }];
        }
        if (cached.state === 'error') {
          return [{ value: '', label: `Failed: ${cached.error || 'check API key'}`, disabled: true }];
        }
        return cached.models.map((id) => ({ value: id, label: id }));
      },
      onChange: (model) => {
        const pk = getToolEntry(tool.key).provider_key || '';
        commitToolEntry(tool.key, pk, model);
      },
    });
    if (!entry.provider_key) modelCombo.setDisabled(true);
    if (tool.key === 'animate' && linkChecked) {
      providerCombo.setDisabled(true);
      modelCombo.setDisabled(true);
    }
    controls.appendChild(modelCombo.root);

    // Eagerly start loading the model list if a provider is already picked.
    if (entry.provider_key && !(tool.key === 'animate' && linkChecked)) {
      loadToolModelsForRow(entry.provider_key, tool.includeAllModels);
    }

    // Register both comboboxes for refresh once async data lands.
    callConfigToolRefreshes.add(() => {
      providerCombo.refresh();
      modelCombo.refresh();
    });

    row.appendChild(controls);
    return row;
  }

  function loadToolModelsForRow(providerKey, includeAll) {
    const cacheKey = providerKey + '|' + (includeAll ? '1' : '0');
    const existing = callConfigToolModelCache[cacheKey];
    if (existing && (existing.state === 'ready' || existing.state === 'loading')) return;
    callConfigToolModelCache[cacheKey] = { state: 'loading', models: [] };
    refreshToolsTabComboboxes();
    api.fetchAiModels(providerKey, '__STORED__', null, false, includeAll)
      .then((models) => {
        callConfigToolModelCache[cacheKey] = {
          state: 'ready',
          models: Array.isArray(models) ? models : [],
        };
        refreshToolsTabComboboxes();
      })
      .catch((err) => {
        callConfigToolModelCache[cacheKey] = {
          state: 'error',
          models: [],
          error: String(err?.message || err || '').slice(0, 80),
        };
        refreshToolsTabComboboxes();
      });
  }

  function refreshToolsTabComboboxes() {
    for (const fn of callConfigToolRefreshes) {
      try { fn(); } catch { /* ignore */ }
    }
  }

  function renderWebSearchRow() {
    const ws = callConfigToolsState.toolConfig.web_search;
    const row = el('div', { class: 'agent-config__tool-row' });

    const head = el('div', { class: 'agent-config__tool-head' });
    head.appendChild(el('div', { class: 'agent-config__tool-title' }, 'Web search'));
    head.appendChild(el('div', { class: 'agent-config__tool-hint' },
      'Anthropic, Gemini, and OpenAI GPT-5 run this server-side. The backend below is only used by OpenAI Chat Completions, OpenAI-compatible providers, and OpenRouter.'));

    const toggleWrap = el('label', { class: 'agent-config__tool-link', title: 'Enable / disable web search' });
    const toggleBox = el('input', { type: 'checkbox' });
    if (ws.enabled) toggleBox.checked = true;
    toggleBox.addEventListener('change', async () => {
      ws.enabled = toggleBox.checked;
      try {
        await api.setToolConfig(callConfigToolsState.toolConfig);
        localStorage.setItem('rustic_tool_config', JSON.stringify(callConfigToolsState.toolConfig));
      } catch (e) { console.warn(e); }
    });
    toggleWrap.appendChild(toggleBox);
    toggleWrap.appendChild(el('span', {}, 'Enabled'));
    head.appendChild(toggleWrap);
    row.appendChild(head);

    const controls = el('div', { class: 'agent-config__tool-controls' });
    const backendCombo = createCombobox({
      initialValue: ws.backend || 'Tavily',
      placeholder: 'Backend…',
      allowCustom: false,
      getOptions: () => ([
        { value: 'Tavily', label: 'Tavily' },
        { value: 'Brave',  label: 'Brave Search' },
        { value: 'Mcp',    label: 'Tavily MCP (defer to MCP server)' },
      ]),
      onChange: async (v) => {
        ws.backend = v;
        try {
          await api.setToolConfig(callConfigToolsState.toolConfig);
          localStorage.setItem('rustic_tool_config', JSON.stringify(callConfigToolsState.toolConfig));
        } catch (e) { console.warn(e); }
      },
    });
    controls.appendChild(backendCombo.root);
    const status = el('div', { class: 'agent-config__tool-status' });
    if (ws.backend !== 'Mcp' && !ws.api_key) {
      status.textContent = 'No API key — set one in Settings → Tools.';
      status.classList.add('agent-config__tool-status--warn');
    } else {
      status.textContent = ws.backend === 'Mcp' ? 'Delegated to MCP server' : 'API key stored';
    }
    controls.appendChild(status);
    row.appendChild(controls);
    return row;
  }

  function renderToolsFooter() {
    const footer = el('div', { class: 'agent-config__tools-footer' });
    const link = el('button', { class: 'agent-config__tools-footer-link', type: 'button' },
      'Open full tool settings →');
    link.addEventListener('click', (ev) => {
      ev.stopPropagation();
      closeCallConfig();
      setSettingsCategory('agent');
      openSettings();
    });
    footer.appendChild(link);
    return footer;
  }

  function renderEffortFooter(currentModel) {
    const footer = el('div', { class: 'agent-config__footer' });
    const cap = getThinkingCapability(currentModel);

    const labelGroup = el('span', { class: 'agent-config__effort-label' });
    labelGroup.appendChild(icon('M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z', 14));
    labelGroup.appendChild(el('span', {}, 'Thinking effort'));
    footer.appendChild(labelGroup);

    if (!cap) {
      footer.appendChild(el('span', { class: 'agent-config__effort-empty' }, 'Not available for this model'));
      return footer;
    }

    if (cap.type === 'effort') {
      const toggles = el('div', { class: 'agent-config__effort-toggles' });
      const offBtn = el('button', {
        class: `agent-config__effort-btn${!thinkingEnabled ? ' agent-config__effort-btn--active' : ''}`,
      }, 'off');
      offBtn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        thinkingEnabled = false;
        saveThinkingForModel(currentModel);
        setPendingThinking({ enabled: thinkingEnabled, effort: thinkingEffort, budget: thinkingBudget });
        updateThinkBtn();
        updateCallConfigBtn();
        rebuildCallConfigContent();
      });
      toggles.appendChild(offBtn);
      for (const level of cap.levels) {
        const isActive = thinkingEnabled && thinkingEffort === level;
        const btn = el('button', {
          class: `agent-config__effort-btn${isActive ? ' agent-config__effort-btn--active' : ''}`,
        }, level);
        btn.addEventListener('click', (ev) => {
          ev.stopPropagation();
          thinkingEnabled = true;
          thinkingEffort = level;
          saveThinkingForModel(currentModel);
          setPendingThinking({ enabled: thinkingEnabled, effort: thinkingEffort, budget: thinkingBudget });
          updateThinkBtn();
          updateCallConfigBtn();
          rebuildCallConfigContent();
        });
        toggles.appendChild(btn);
      }
      footer.appendChild(toggles);
      return footer;
    }

    if (cap.type === 'budget') {
      const sliderRow = el('div', { class: 'agent-config__effort-slider-row' });
      const slider = el('input', {
        type: 'range', class: 'agent-config__effort-slider',
        min: String(cap.min), max: String(cap.max),
        step: String(Math.max(128, Math.floor((cap.max - cap.min) / 100))),
        value: String(thinkingBudget),
      });
      const budgetReadout = el('span', { class: 'agent-config__effort-budget' }, formatTokens(thinkingBudget));
      slider.addEventListener('input', (ev) => {
        ev.stopPropagation();
        thinkingBudget = parseInt(ev.target.value, 10);
        thinkingEnabled = thinkingBudget > 0;
        saveThinkingForModel(currentModel);
        setPendingThinking({ enabled: thinkingEnabled, effort: thinkingEffort, budget: thinkingBudget });
        updateThinkBtn();
        updateCallConfigBtn();
        budgetReadout.textContent = formatTokens(thinkingBudget);
      });
      sliderRow.appendChild(slider);
      sliderRow.appendChild(budgetReadout);
      footer.appendChild(sliderRow);
      return footer;
    }

    return footer;
  }

  function openCallConfig() {
    closeCallConfig();
    callConfigOpen = true;

    callConfigOverlay = el('div', { class: 'agent-config-overlay' });
    callConfigModal   = el('div', { class: 'agent-config-modal' });
    callConfigOverlay.appendChild(callConfigModal);

    callConfigOverlay.addEventListener('click', (ev) => {
      if (ev.target === callConfigOverlay) closeCallConfig();
    });
    callConfigModal.addEventListener('click', (ev) => ev.stopPropagation());

    const onKey = (ev) => { if (ev.key === 'Escape') { ev.stopPropagation(); closeCallConfig(); } };
    document.addEventListener('keydown', onKey);
    callConfigOverlay.__rusticEsc = onKey;

    // Seed the rail so the provider that owns the current model is focused.
    const providers = buildProviderEntries();
    callConfigSelectedProvider = findOwningProvider(providers, getCurrentModel())
      || providers.find((p) => p.hasKey && p.models.length)?.id
      || providers[0]?.id
      || null;

    rebuildCallConfigContent();
    document.body.appendChild(callConfigOverlay);
    requestAnimationFrame(() => {
      const activeRow = callConfigModal?.querySelector('.agent-config__model--active');
      if (activeRow) activeRow.scrollIntoView({ block: 'nearest' });
    });

    // Refresh persisted model lists in the background so newly-released
    // models appear without forcing the user to re-enter their API key.
    refreshAllProviderModels(true).then((changed) => {
      if (!callConfigOpen) return;
      if (changed && changed.size > 0) rebuildCallConfigContent();
    }).catch(() => {});
  }

  callConfigBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (callConfigOpen) { closeCallConfig(); return; }
    closeModelDropdown();
    openCallConfig();
  });

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

  // Pick the smallest positive integer not already in use as a chip id, so
  // removing "Pasted text #2" and pasting again brings the new chip back to
  // #2 / #1 instead of marching the counter monotonically upward (#3, #4, …).
  // Walks the existing ids in sorted order and stops at the first gap; O(n).
  function nextPasteChipId() {
    const used = pasteChips.map(c => c.id).sort((a, b) => a - b);
    let next = 1;
    for (const id of used) {
      if (id === next) next++;
      else if (id > next) break;
    }
    return next;
  }

  textarea.addEventListener('paste', (e) => {
    // **Read clipboard data synchronously** so the `preventDefault()` call
    // lands inside the same event tick as the paste — `getAsString()` is
    // callback-based and resolves on the next microtask, which is too late:
    // the browser has already inserted the pasted text into the textarea
    // before the await returns. `clipboardData.getData('text/plain')` is
    // synchronous and returns the same string.
    const cd = e.clipboardData;
    if (!cd) return;
    const pastedStr = cd.getData('text/plain') || '';
    console.log(`[chip][paste] event fired — clipboard length=${pastedStr.length}, threshold=800`);
    if (pastedStr.length > 800) {
      e.preventDefault();
      const chipId = nextPasteChipId();
      pastedTexts.set(chipId, pastedStr);
      pasteChips.push({ id: chipId, text: pastedStr });
      // Keep the chip row sorted by id so the visual order matches the
      // numeric ids — otherwise a refilled gap (#2 inserted after #3) shows
      // out of order.
      pasteChips.sort((a, b) => a.id - b.id);
      console.log(`[chip][paste] chip added id=${chipId}, total chips=${pasteChips.length}`);
      renderPasteChips();
      updateSendBtn();
      return;
    }
    // Image-paste branch — must run async because `readFileAsBase64` reads
    // the File. preventDefault is unnecessary here: an image paste into a
    // <textarea> already inserts nothing.
    (async () => {
      for (const item of cd.items) {
        if (item.type.startsWith('image/')) {
          const file = item.getAsFile();
          if (file) {
            const base64 = await readFileAsBase64(file);
            attachedFiles.push({ name: `pasted-image.${file.type.split('/')[1] || 'png'}`, type: file.type, base64 });
            renderAttachmentPills();
          }
        }
      }
    })();
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

    // Claude Code (subscription harness) tasks get the CLI's builtin command
    // list plus any custom commands in `~/.claude/commands/` or
    // `<project>/.claude/commands/`. The user-typed `/foo args` is forwarded
    // verbatim to the CLI's stdin — this is purely for discoverability.
    if (getCurrentProviderType() === 'ClaudeCode') {
      try {
        const projectRoot = getActiveProjectRoot();
        const cmds = await api.listClaudeCodeSlashCommands(projectRoot);
        for (const c of (cmds || [])) {
          results.push({
            type: 'claudeSlash',
            name: c.name,
            description: c.description || '',
            source: c.source,
          });
        }
      } catch {}
    }

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
    if (type === 'claudeSlash') return 'Claude';
    return type;
  }

  // The picker uses `position: fixed` so it can escape the `overflow: hidden`
  // clip on the card-style chat-input-area variants (welcome + expanded chat).
  // Coordinates anchor to the input area's current rect; we recompute on every
  // render so the picker tracks the input across keystrokes / layout shifts.
  function positionSlashPicker() {
    const rect = inputArea.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    slashPicker.style.left = `${rect.left}px`;
    slashPicker.style.width = `${rect.width}px`;
    slashPicker.style.bottom = `${window.innerHeight - rect.top + 4}px`;
  }

  function renderSlashPicker() {
    slashPicker.innerHTML = '';
    if (!slashPickerOpen || slashPickerFiltered.length === 0) {
      slashPicker.classList.add('slash-picker--hidden');
      return;
    }
    slashPicker.classList.remove('slash-picker--hidden');
    positionSlashPicker();

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

    // Claude Code slash commands are forwarded verbatim to the CLI's stdin
    // (the CLI expands `/foo args` itself). So we INLINE the literal command
    // text instead of converting to a chip — chip-based inlining-on-send
    // doesn't fire for slash commands which the CLI needs to see at the
    // start of the user message.
    if (item.type === 'claudeSlash') {
      const value = textarea.value;
      const inserted = `/${item.name} `;
      textarea.value = value.slice(0, ctx.slashStart) + inserted + value.slice(ctx.slashEnd);
      const cursor = ctx.slashStart + inserted.length;
      textarea.selectionStart = textarea.selectionEnd = cursor;
      autoResizeTextarea();
      textarea.focus();
      return;
    }

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

  // Assemble the outgoing message body from the current composer state.
  // Three paths now share this: the regular send, the mid-turn queue, and
  // the stop-and-send handler. Before this helper existed, the queue paths
  // sent only the typed text — paste chips and attached tags were silently
  // dropped, which is what reintroduced the "pasted text disappears" bug
  // mid-turn. `text` should already be `textarea.value.trim()`.
  function buildOutgoingText(text) {
    console.log(`[chip][build] called — text.length=${(text || '').length}, pasteChips.length=${pasteChips.length}, attachedTags.length=${attachedTags.length}`);
    if (pasteChips.length > 0) {
      console.log('[chip][build] chip ids:', pasteChips.map(c => `#${c.id}(${c.text.length} chars)`).join(', '));
    }
    const workflowParts = attachedTags
      .filter(t => t.type === 'workflow' && t.body)
      .map(t => {
        const safeName = String(t.name || '').replace(/"/g, '&quot;');
        return `<workflow-tag name="${safeName}">\n## Workflow: ${t.name}\n\n${t.body}\n</workflow-tag>`;
      });
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
        if (t.pid != null) bits.push(`pid=${t.pid}`);
        if (t.label)       bits.push(`label="${t.label}"`);
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
    if (text) finalParts.push(text);
    for (const chip of pasteChips) {
      // Wrap each pasted chunk in a parseable sentinel. The bubble renderer
      // strips these tags back out and shows them as collapsible
      // "Pasted text #N" chips at the top of the user message — so the chip
      // doesn't visually vanish when the message is sent. The XML-style tag
      // also gives the model a clear "this is pasted content" delimiter
      // instead of a raw concatenation.
      finalParts.push(`<pasted-text id="${chip.id}">\n${chip.text}\n</pasted-text>`);
    }
    const finalText = finalParts.join('\n\n');
    const hasMarker = finalText.includes('<pasted-text id="');
    console.log(`[chip][build] returning finalText.length=${finalText.length}, hasMarker=${hasMarker}`);
    return finalText;
  }

  // Reset every composer affordance after a successful enqueue/send. Single
  // source of truth — every send path was clearing a different subset
  // before, which is how chips and tags got stranded mid-turn.
  function clearComposerAfterSend(taskId) {
    textarea.value = '';
    textarea.style.height = '';
    attachedFiles = [];
    attachedTags = [];
    pasteChips = [];
    pastedTexts.clear();
    if (taskId) draftStore.delete(taskId);
    renderAttachmentPills();
    renderTagChips();
    renderPasteChips();
    autoResizeTextarea();
    updateSendBtn();
  }

  /// Stop-and-send: aborts the current turn, then flushes the input as a
  /// brand-new turn. Works for both harness and native tasks: the executor
  /// persists whatever streamed before the abort (partial assistant text
  /// for native via executor.rs cancel branch; full per-event history for
  /// harness via harness_runtime.rs cancel branch), so the queued follow-up
  /// lands with a coherent conversation context.
  stopSendBtn.addEventListener('click', async () => {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;
    const text = textarea.value.trim();
    // Empty-content guard now also considers tags + paste chips — without
    // this a paste-only nudge silently drops because the literal `text`
    // string is empty.
    if (!text && attachedFiles.length === 0 && attachedTags.length === 0 && pasteChips.length === 0) return;
    const images = attachedFiles
      .filter((f) => f.base64 && f.type.startsWith('image/'))
      .map((f) => ({ media_type: f.type, data: f.base64 }));
    // Save attachments to disk before send so the model can refer to them by
    // path from media tools. The on-disk copy is in addition to the inline
    // images, not a replacement.
    const savedAttachmentPaths = await persistAttachedImagesAsFiles(taskId);
    const finalText = buildOutgoingText(text) + buildAttachmentNote(savedAttachmentPaths);

    // Snapshot the input *before* aborting so a state-change cascade can't
    // race-clear the textarea. Then queue and let the auto-drain in
    // updateTaskStatus fire the message as the next turn — same code path
    // as the regular Queue button. This avoids us having to call
    // sendMessage directly here (which would race with abort_task's
    // worker-thread shutdown).
    console.log(`[chip][send/stop-and-send] queueMessage finalText.length=${finalText.length}, hasMarker=${finalText.includes('<pasted-text id="')}`);
    queueMessage(taskId, finalText, images);
    clearComposerAfterSend(taskId);
    renderQueuedArea();

    stopSendBtn.disabled = true;
    try {
      await api.abortTask(taskId);
    } catch (e) {
      console.error('stop-and-send: abort failed', e);
    } finally {
      stopSendBtn.disabled = false;
    }
  });

  sendBtn.addEventListener('click', async () => {
    let taskId = agentStore.getState('activeTaskId');

    // Mid-turn steering: if the task is Running and the user has typed
    // something, interrupt the current turn and let drainPendingUserInput
    // fire the new message as a fresh turn — Claude-Code-style nudge, not
    // a passive wait-for-end. The empty-input case still falls through to
    // the plain Stop branch below so an idle Enter doesn't fire a blank
    // turn.
    if (sendBtnMode === 'queue' && taskId) {
      const text = textarea.value.trim();
      // Empty-content guard now also considers tags + paste chips — without
      // this a paste-only mid-turn nudge silently drops because the literal
      // `text` string is empty.
      if (!text && attachedFiles.length === 0 && attachedTags.length === 0 && pasteChips.length === 0) return;
      const images = attachedFiles
        .filter((f) => f.base64 && f.type.startsWith('image/'))
        .map((f) => ({ media_type: f.type, data: f.base64 }));
      const savedAttachmentPaths = await persistAttachedImagesAsFiles(taskId);
      const finalText = buildOutgoingText(text) + buildAttachmentNote(savedAttachmentPaths);
      console.log(`[chip][send/mid-turn-queue] queueMessage finalText.length=${finalText.length}, hasMarker=${finalText.includes('<pasted-text id="')}`);
      // Stage the message in the queue first so a fast Running →
      // not-Running transition (from the abort) finds something to drain.
      // If the user types another follow-up before the abort completes,
      // it stacks here as a second queue entry and lands as the *next*
      // turn — never concatenated.
      queueMessage(taskId, finalText, images);
      clearComposerAfterSend(taskId);
      renderQueuedArea();
      // Fire the abort. Backend persists partial assistant output (executor.rs
      // / harness_runtime.rs cancel branches) so the next turn has coherent
      // context. Errors are non-fatal — if the abort racy-loses to a natural
      // turn end, drainPendingUserInput still fires our message normally.
      api.abortTask(taskId).catch((e) => console.error('mid-turn interrupt failed:', e));
      return;
    }

    if (sendBtnMode === 'stop') {
      if (!taskId) return;
      sendBtn.disabled = true;
      try { await api.abortTask(taskId); } finally { sendBtn.disabled = false; }
      return;
    }

    const text = textarea.value.trim();
    if (!text && attachedFiles.length === 0 && attachedTags.length === 0 && pasteChips.length === 0) return;

    // Snapshot the composer at click time. The welcome-screen send below
    // calls `createTask`, which sets `activeTaskId` to the new task — that
    // fires the activeTaskId subscriber, which calls `restoreDraft(newId)`.
    // The new task has no saved draft, so restoreDraft *wipes* every
    // closure-scoped composer field (textarea, paste chips, tags, files).
    // Without this snapshot, paste-then-send-from-welcome-screen silently
    // drops the chip — exactly what the `[chip][build] pasteChips.length=0`
    // log was showing. The same wipe pattern is already mitigated for
    // thinking config a few lines below (see "Re-apply the welcome-screen
    // thinking choice" comment).
    const composerSnap = {
      text,
      chips: pasteChips.slice(),
      tags: attachedTags.slice(),
      files: attachedFiles.slice(),
      pastedTextEntries: Array.from(pastedTexts.entries()),
    };

    // Welcome-screen send: no active task yet. Auto-create one under the
    // picked project. Global now has its own backing row in the DB so no
    // first-project fallback is needed.
    if (!taskId) {
      const pending = agentStore.getState('pendingProjectId');

      // Subscription harnesses (Claude Code / Codex) require a real project
      // root: the CLIs scope their session storage / conversation memory by
      // cwd, so a Global "no project" chat ends up looking at internal
      // `~/.claude/projects/<cwd-encoded>/` paths instead of the user's
      // code. Block the combo up front with a clear alert rather than
      // letting the user discover it through a confused-looking response.
      const pendingModelChoice = agentStore.getState('pendingModelChoice');
      const pendingProvider = pendingModelChoice?.providerId || '';
      const isHarnessProvider = pendingProvider === 'ClaudeCode' || pendingProvider === 'Codex';
      if (pending === GLOBAL_PROJECT_ID && isHarnessProvider) {
        await showAlertDialog(
          'Pick a project first',
          'Claude Code and Codex are subscription CLIs that scope their work by project — Global chats aren\'t supported for these providers. Pick a project from the Explorer (or switch to an API provider) and try again.',
        );
        return;
      }

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

      // Restore the composer from the click-time snapshot. The activeTaskId
      // subscriber fired during createTask above and ran restoreDraft for
      // the brand-new task id — that wiped pasteChips/attachedTags/attachedFiles
      // since no draft exists for a task we just made. Re-binding here is
      // the matching fix to the thinking-config restore right above.
      pasteChips = composerSnap.chips;
      attachedTags = composerSnap.tags;
      attachedFiles = composerSnap.files;
      pastedTexts.clear();
      for (const [id, txt] of composerSnap.pastedTextEntries) pastedTexts.set(id, txt);
      textarea.value = composerSnap.text;
      renderPasteChips();
      renderTagChips();
      renderAttachmentPills();
      console.log(`[chip][welcome-restore] re-bound after createTask — chips=${pasteChips.length}, tags=${attachedTags.length}, files=${attachedFiles.length}`);
    }

    // If the model is waiting for a question response, route via respondToAgentQuestion.
    // We still build the full message body (text + tags + wrapped paste chips)
    // so that pasting and answering an `ask_user` question doesn't silently
    // drop the chip — same end-to-end path as a regular send.
    const currentTask = agentStore.getState('tasks')[taskId];
    if (currentTask?.pendingQuestion) {
      if (!text && pasteChips.length === 0 && attachedTags.length === 0) return;
      const finalText = buildOutgoingText(text);
      console.log(`[chip][send/pending-question] respondToAgentQuestion finalText.length=${finalText.length}, hasMarker=${finalText.includes('<pasted-text id="')}`);
      await respondToAgentQuestion(taskId, currentTask.pendingQuestion.request_id, finalText);
      clearComposerAfterSend(taskId);
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
    const savedAttachmentPaths = await persistAttachedImagesAsFiles(taskId);

    // Tag/chip expansion + chip wrapping happen in buildOutgoingText so that
    // the queue / stop-and-send paths produce identical output. The
    // attachment note (if any) sits at the end so it doesn't interrupt the
    // user's prose.
    const finalText = buildOutgoingText(text) + buildAttachmentNote(savedAttachmentPaths);
    console.log(`[chip][send/main] sendMessage finalText.length=${finalText.length}, hasMarker=${finalText.includes('<pasted-text id="')}`);

    sendMessage(taskId, finalText, thinkBudget, images.length ? images : undefined);

    clearComposerAfterSend(taskId);
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
    // Mid-turn steering: the send button morphs between Stop and Queue
    // based on whether the input has content while a task is Running, so
    // it has to react to every keystroke (cheap — no DOM rebuild unless
    // the *mode* actually changes).
    updateSendBtn();
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

  // Toolbar right: optional "Stop & send" + main send/stop/queue button.
  // The stop-and-send button only appears when:
  //   * the active task is `Running`
  //   * the input has text (or attachments)
  //   * the task is harness-backed (native interrupt-and-send is its own
  //     follow-up — see plan §14 native-provider section)
  // (`stopSendBtn` itself is created up near `sendBtn` so updateSendBtn /
  // the click handler can reference it without hitting TDZ at init time.)
  const toolbarRight = el('div', { class: 'chat-toolbar-right' });
  toolbarRight.appendChild(stopSendBtn);
  toolbarRight.appendChild(sendBtn);

  inputToolbar.appendChild(toolbarLeft);
  inputToolbar.appendChild(toolbarRight);

  // Input wrapper: bordered box containing textarea on top + toolbar on bottom
  const inputWrapper = el('div', { class: 'chat-input-wrapper' });
  inputWrapper.appendChild(textarea);
  inputWrapper.appendChild(inputToolbar);

  inputArea.appendChild(attachmentPills);
  inputArea.appendChild(pasteChipsContainer);
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
  container.appendChild(messagesArea);
  container.appendChild(approvalArea);
  container.appendChild(queuedArea);
  container.appendChild(chatTabsArea);
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
    // Welcome screen defaults to Global scope. loadPendingProjectId()
    // already returns GLOBAL_PROJECT_ID when nothing is persisted, but cover
    // the empty / null edge case here too in case state was cleared at runtime.
    if (!projectId) {
      projectId = GLOBAL_PROJECT_ID;
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

    // If no provider is configured, show a CTA above the input directing the
    // user to settings rather than letting them type and discover it on send.
    if (!hasAnyConnectedProvider()) {
      const cta = el('div', { class: 'chat-empty__connect-cta' });
      cta.appendChild(icon('M12 9v2m0 4h.01M5.07 19h13.86a2 2 0 0 0 1.74-3L13.73 4a2 2 0 0 0-3.46 0L3.34 16a2 2 0 0 0 1.73 3z', 16));
      const text = el('div', { class: 'chat-empty__connect-cta-text' });
      text.appendChild(el('div', { class: 'chat-empty__connect-cta-title' },
        'No AI provider connected'));
      text.appendChild(el('div', { class: 'chat-empty__connect-cta-body' },
        'Add a key for Anthropic, OpenAI, Gemini, or any OpenAI-compatible endpoint to start chatting.'));
      cta.appendChild(text);
      const ctaBtn = el('button', { class: 'chat-empty__connect-cta-btn' }, 'Open AI settings');
      ctaBtn.addEventListener('click', () => {
        setSettingsCategory('agent');
        openSettings();
      });
      cta.appendChild(ctaBtn);
      inner.appendChild(cta);
    }

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

    // Sub-agent view mode: parent task stays active, but the chat panel
    // mirrors the sub-agent's run as if it were its own task. View-only —
    // the input area is hidden via the container class.
    const inSubagentView = subagentViewAgentId && subagentViewParentTaskId === taskId;
    container.classList.toggle('chat-view--subagent-view', !!inSubagentView);
    if (inSubagentView) {
      renderSubagentView(task);
      return;
    }

    renderMessages(task);
  }

  // Tracks which logical "view" the renderMessages caches currently hold —
  // a real task id while normally rendering, or a synthetic 'sub:...' id
  // while in subagent-view mode. renderSubagentView and render() use this to
  // wipe the cache on transitions so the parent's tool-card DOM never gets
  // reused for the sub-agent (or vice versa) via tool_use_id collision.
  let lastRenderedSource = null;

  // Synthesize a "task" object from a live sub-agent's state and route it
  // through the standard renderMessages pipeline so the sub-agent's tool
  // calls render with the same rich UI as the main agent. Caches are cleared
  // when entering/leaving so the parent and sub-agent never share cached
  // DOM keyed by tool_use_id collisions.
  function renderSubagentView(parentTask) {
    const taskId = subagentViewParentTaskId;
    const agentId = subagentViewAgentId;
    const agent = agentStore.getState('subagents')?.[taskId]?.[agentId];

    // Build the "first user message" from the prompt and an "assistant
    // message" from the streamed/persisted output + tool_use blocks. Tool
    // results live on a trailing `tool` role message so buildResultMap finds
    // them in the same shape it expects from the harness.
    const prompt = agent?.prompt || '';
    const output = agent?.output || '';
    const toolCalls = agent?.toolCalls || [];
    const events = agent?.events || [];

    const messages = [];
    if (prompt) {
      messages.push({ role: 'user', content: [{ type: 'text', text: prompt }] });
    }
    const assistantContent = [];
    if (events.length > 0) {
      // Preserve the original interleaved order of text and tool_use blocks
      // captured by the `events` stream so the rendering matches what the
      // model actually emitted (text → tool_use → text → tool_use → …).
      for (const ev of events) {
        if (ev.kind === 'text') {
          if (ev.text) assistantContent.push({ type: 'text', text: ev.text });
        } else if (ev.kind === 'tool_use') {
          assistantContent.push({
            type: 'tool_use',
            id: ev.tool_use_id,
            name: ev.tool_name,
            input: ev.input || {},
          });
        }
      }
    } else {
      // History-loaded sub-agents (or older runs) have no events stream — fall
      // back to "all tool calls, then text". The original interleaving is
      // not recoverable from persisted state.
      for (const tc of toolCalls) {
        assistantContent.push({
          type: 'tool_use',
          id: tc.tool_use_id,
          name: tc.tool_name,
          input: tc.input || {},
        });
      }
      if (output) assistantContent.push({ type: 'text', text: output });
    }
    if (assistantContent.length > 0) {
      messages.push({ role: 'assistant', content: assistantContent });
    }
    for (const tc of toolCalls) {
      if (tc.result == null) continue;
      const content = typeof tc.result === 'string'
        ? tc.result
        : (() => { try { return JSON.stringify(tc.result); } catch { return String(tc.result); } })();
      messages.push({
        role: 'tool',
        content: [{
          type: 'tool_result',
          tool_use_id: tc.tool_use_id,
          content,
          is_error: !!tc.is_error,
        }],
      });
    }

    const status = agent?.status === 'failed'
      ? 'Failed'
      : agent?.status === 'completed'
      ? 'Completed'
      : 'Running';
    const isStreaming = agent?.status === 'running';

    const syntheticTask = {
      id: 'sub:' + taskId + ':' + agentId,
      title: agentId,
      project_id: parentTask?.project_id,
      projectId: parentTask?.projectId,
      model: agent?.model || parentTask?.model,
      provider_type: parentTask?.provider_type,
      messages,
      status,
      isStreaming,
      cost: agent?.cost,
      _isSubagent: true,
    };

    // The renderMessages pipeline keys its DOM cache by tool_use_id /
    // msgIdx. Sub-agent IDs come from the harness so they don't collide
    // with parent-task IDs in practice, but the per-render fingerprint is
    // a single global. Reset both on entry so the first sub-agent render
    // doesn't get suppressed by a matching parent-task fingerprint.
    if (lastRenderedSource !== syntheticTask.id) {
      nodeRenderCache.clear();
      timelineWrappers.clear();
      itemWrappers.clear();
      lastRenderFingerprint = null;
      lastRenderedSource = syntheticTask.id;
    }

    renderMessages(syntheticTask);

    // Prepend the back-bar so it survives the renderMessages reconciliation
    // (which only touches messagesArea's children produced by its own pass).
    // The bar shows the sub-agent's name as the title — the parent task's
    // own title sits in the regular chat header above, so repeating it in
    // the button label was redundant and crowded the bar on long titles.
    const backBar = el('div', { class: 'subagent-view__back-bar' });
    const topRow = el('div', { class: 'subagent-view__back-bar-top' });
    const backBtn = el('button', {
      class: 'subagent-view__back-btn',
      title: 'Back to ' + (parentTask?.title || 'parent task'),
    });
    backBtn.appendChild(icon('M15 19l-7-7 7-7', 14));
    backBtn.addEventListener('click', () => {
      subagentViewAgentId = null;
      subagentViewParentTaskId = null;
      lastRenderedSource = null;
      nodeRenderCache.clear();
      timelineWrappers.clear();
      itemWrappers.clear();
      lastRenderFingerprint = null;
      render();
    });
    topRow.appendChild(backBtn);
    // Prefer the original (cased) sub-agent name from the spawn block in
    // the parent task — agentId is the lowercase-hyphen slug, so falling
    // back to it loses both casing and any non-alphanum punctuation.
    let agentTitle = agentId.replace(/-/g, ' ');
    for (const m of (parentTask?.messages || [])) {
      if (m.role !== 'assistant') continue;
      for (const b of (m.content || [])) {
        if (b?.type !== 'tool_use') continue;
        if (b.name !== 'spawn_subagent' && b.name !== 'Task') continue;
        const rawName = b.input?.name || b.input?.description;
        if (rawName && slugifyAgentName(rawName) === agentId) {
          agentTitle = rawName;
        }
      }
    }
    topRow.appendChild(el('span', { class: 'subagent-view__back-bar-title' }, agentTitle));
    backBar.appendChild(topRow);

    // ── Stats row: ↑ tokens / ↓ tokens / Answer / $ cost / words ──
    // Same content the inline subagent card used to carry. Live-updated
    // because the `subagents` store subscriber re-renders this view.
    const liveCost = agent?.cost || {};
    const inputTokens = liveCost.total_input_tokens || 0;
    const cacheRead = liveCost.total_cache_read_tokens || 0;
    const cacheWrite = liveCost.total_cache_write_tokens || 0;
    const sentTotal = inputTokens + cacheRead + cacheWrite;
    const outputTokens = liveCost.total_output_tokens || 0;
    const subCostUsd = liveCost.estimated_cost_usd || 0;
    const wordCount = output ? output.trim().split(/\s+/).filter(Boolean).length : 0;

    const statsRow = el('div', { class: 'subagent-view__stats' });

    // The input/output token counts are non-interactive in the dedicated
    // sub-agent view: the full prompt, streamed activity, and final answer
    // are all visible inline below, so the click-to-open-scratch behaviour
    // from the inline card would be redundant here. Render them as plain
    // stat spans matching the cost / words pills.
    const inputStat = el('span', { class: 'subagent-card__stat subagent-card__stat--sent' });
    inputStat.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '↑'));
    inputStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, sentTotal > 0 ? formatTokens(sentTotal) : '0'));
    inputStat.title = [
      `Input (fresh): ${inputTokens.toLocaleString()}`,
      `Cache read: ${cacheRead.toLocaleString()}`,
      `Cache write: ${cacheWrite.toLocaleString()}`,
    ].join('\n');
    statsRow.appendChild(inputStat);

    const outputStat = el('span', { class: 'subagent-card__stat subagent-card__stat--recv' });
    outputStat.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '↓'));
    outputStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, outputTokens > 0 ? formatTokens(outputTokens) : '0'));
    statsRow.appendChild(outputStat);

    const costStat = el('span', { class: 'subagent-card__stat subagent-card__stat--cost' });
    costStat.appendChild(el('span', { class: 'subagent-card__stat-icon' }, '$'));
    costStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, subCostUsd > 0 ? subCostUsd.toFixed(3) : '0'));
    statsRow.appendChild(costStat);

    const wordStat = el('span', { class: 'subagent-card__stat subagent-card__stat--words' });
    wordStat.appendChild(el('span', { class: 'subagent-card__stat-value' }, wordCount > 0 ? `${wordCount} words` : '0 words'));
    statsRow.appendChild(wordStat);

    backBar.appendChild(statsRow);

    // Drop any existing back-bar from a previous render then prepend.
    messagesArea.querySelector(':scope > .subagent-view__back-bar')?.remove();
    messagesArea.insertBefore(backBar, messagesArea.firstChild);
  }

  // Wire the module-scope slot so renderSubagentCard (also at module scope,
  // outside this closure) can ask the chat-view to enter sub-agent view.
  openSubagentView = (parentTaskId, agentId) => {
    if (!parentTaskId || !agentId) return;
    subagentViewParentTaskId = parentTaskId;
    subagentViewAgentId = agentId;
    nodeRenderCache.clear();
    timelineWrappers.clear();
    itemWrappers.clear();
    lastRenderFingerprint = null;
    lastRenderedSource = null;
    render();
  };

  // ── Keyed reconciliation cache ──────────────────────────────────────────
  // Persists across renders. Keys are stable per logical node (tool_use_id,
  // msgIdx, etc.); values store { version, element }. When a node's version
  // matches the cached one, we reuse the *same* DOM element so its CSS
  // animations, hover state, and any expand/collapse state survive the
  // re-render. Without this, every tool_use / tool_result event rebuilt all
  // activity cards from scratch — moving them into the new fragment was
  // atomic, but the spinner restart + re-attached event listeners read as a
  // flash on every event.
  const nodeRenderCache = new Map();
  const streamingMarkdownTimers = new Map();
  const taskMessagesFragments = new Map();
  const taskRenderCaches = new Map();
  let prevActiveTaskId = null;
  let pendingTaskSwitchScroll = null;

  function nodeKey(node) {
    switch (node.type) {
      case 'user-message':       return `u:${node.msgIdx}`;
      case 'assistant-text':     return `at:${node.msgIdx}`;
      case 'thinking':           return `t:${node.msgIdx}:${node.contentIdx ?? node.blockIdx}`;
      case 'thinking-indicator': return `ti:${node.msgIdx}`;
      case 'tool-use':           return `tu:${node.toolUseId || node.block?.id}`;
      case 'collapsed-group': {
        const id = node.children?.[0]?.toolUseId || node.children?.[0]?.block?.id || '';
        return `cg:${id}`;
      }
      case 'parallel-group': {
        const first = node.children?.[0];
        const id = first?.type === 'tool-use'
          ? (first.toolUseId || first.block?.id)
          : (first?.children?.[0]?.toolUseId || first?.children?.[0]?.block?.id || '');
        return `pg:${node.msgIdx}:${id}`;
      }
      case 'context-condense':   return `cc:${node.msgIdx}`;
      case 'task-complete':      return `tc:${node.msgIdx}`;
      case 'model-switch':       return `ms:${node.msgIdx}`;
      default:                   return null;
    }
  }

  // Stringifies the result-state of one tool-use child so collapsed/parallel
  // group fingerprints flip when any child gains a result or its result content
  // grows. Mirroring this in both helpers keeps groups in sync with their
  // standalone children.
  function toolFingerprint(n) {
    if (n.type === 'tool-use') {
      const id = n.toolUseId || n.block?.id || '';
      const r = n.toolResult;
      if (!r) return `${id}:p`;
      return `${id}:${r.is_error ? 'e' : 'd'}:${(r.content || '').length}`;
    }
    if (n.type === 'collapsed-group') {
      return `cg[${(n.children || []).map(toolFingerprint).join('|')}]`;
    }
    return '';
  }

  function nodeVersion(node, task) {
    switch (node.type) {
      case 'user-message': {
        // **Don't** include turnUsage or task.status here. They mutate on
        // every `agent-request-usage` event (5+ times per multi-tool turn)
        // and rebuilding the whole bubble for a small cost-pill change is
        // what was perceived as flicker. The pill is updated in place by
        // `updateCostPillsInPlace` from the tasks-subscriber on every
        // change without rebuilding the bubble.
        const len = (node.msg.content || []).reduce(
          (s, b) => s + (b.type === 'text' ? (b.text?.length || 0) : 0), 0);
        const imgCount = (node.msg.content || []).filter(b => b.type === 'image').length;
        return `text:${len}:img:${imgCount}`;
      }
      case 'assistant-text': {
        // The streaming fast-path mutates innerHTML directly. Keep the
        // version stable while live so renderMessages doesn't overwrite that
        // work; flip to a length-based version once streaming ends so the
        // final pass attaches code-copy buttons + final markdown.
        const isStreaming = task.isStreaming && node.isLastMsg;
        if (isStreaming) return 'streaming';
        const len = node.blocks.reduce((s, b) => s + (b.text?.length || 0), 0);
        const errKey = node.blocks.some(b => b.errorMeta) ? '+err' : '';
        return `done:${len}${errKey}`;
      }
      case 'thinking': {
        const len = node.block.thinking?.length || 0;
        const dur = node.block.duration_secs || 0;
        // The thinking card is "live" only while it is genuinely the tail of
        // the last assistant message. Once another block (tool_use / non-empty
        // text) appears after it, the card must flip to its "Thought for Xs"
        // done state — even if the task itself is still streaming the next
        // block. Without this, the cached shimmering DOM gets reused on every
        // re-render and the card stays stuck on "Thinking…" until tool_result
        // finally bumps `isLastMsg` to false much later.
        //
        // We treat a trailing *empty* text block as still-tail so the brief
        // window between content_block_start and the first text delta doesn't
        // tear down the live DOM (which is what was happening before — the
        // fix is `isTail`, not the previous `dur`-in-version attempt).
        const msgContent = task.messages[node.msgIdx]?.content || [];
        const ci = node.contentIdx;
        const last = msgContent[msgContent.length - 1];
        const isTail = ci === msgContent.length - 1
          || (ci === msgContent.length - 2 && last?.type === 'text' && !last?.text);
        return task.isStreaming && node.isLastMsg && isTail ? 'live' : `done:${len}:${dur}`;
      }
      case 'thinking-indicator':
        return 'static';
      case 'tool-use': {
        const r = node.toolResult;
        if (!r) {
          // Pre-result phase: header only, no dropdown. The header text
          // updates only when the one-line `summary` (path / command /
          // pattern) changes, which `getToolSummary` derives from a few
          // specific input fields. Bumping on every input delta would
          // force a rebuild for every fragment; instead bump when the
          // summary string actually differs.
          const summary = (() => {
            try { return getToolSummary(node.toolName, node.toolInput || {}); } catch { return ''; }
          })();
          return `pending:${summary.length}:${summary}`;
        }
        return `done:${r.is_error ? 1 : 0}:${(r.content || '').length}`;
      }
      case 'collapsed-group':
        return (node.children || []).map(toolFingerprint).join('|');
      case 'parallel-group':
        return (node.children || []).map(toolFingerprint).join('||');
      case 'context-condense':
        return `${node.content?.status || ''}:${node.content?.original_messages || 0}:${node.content?.condensed_to || 0}`;
      case 'task-complete': {
        const c = node.content || {};
        return `${(c.summary || '').length}`;
      }
      case 'model-switch':
        return `${node.content?.from_model || ''}->${node.content?.to_model || ''}:${node.content?.provider_type || ''}`;
      default:
        return null;
    }
  }

  // Whole-render fingerprint — concatenation of every node's key+version in
  // order. When this is identical to the previous render's value, *nothing*
  // visible changed: same nodes, same order, same per-node fingerprints. We
  // can skip the entire reconciliation pass.
  let lastRenderFingerprint = null;

  // Persistent wrappers for the activity-timeline structure. Without these,
  // every render rebuilt both the `activity-timeline` div (which draws the
  // vertical line via CSS) and every `activity-timeline__item` div from
  // scratch — even though the *cards* inside survived via `nodeRenderCache`.
  // The wrapper recreation is what painted as flicker even on legitimate
  // single-node updates: the parent chain of every cached card was being
  // torn down and rebuilt every render.
  //
  // Keys:
  //   - `timelineWrappers`:  first-activity-node-key → <div.activity-timeline>
  //   - `itemWrappers`:      activity-node-key → <div.activity-timeline__item>
  // Both are pruned at the end of each render based on what was actually used.
  const timelineWrappers = new Map();
  const itemWrappers = new Map();

  /// Minimum-mutation reconciliation: align `parent`'s children with the
  /// ordered `desired` array. Children already at the right position aren't
  /// touched. Children missing from `desired` are removed. New or moved
  /// children are inserted/relocated via `insertBefore`. Crucially this
  /// never calls `replaceChildren`, so elements that are already in `parent`
  /// at the right index keep their layout/animation state intact — that's
  /// what fixes the residual flicker.
  function reconcileChildren(parent, desired) {
    const desiredSet = new Set(desired);
    // Pass 1: drop children that aren't in the desired list.
    let cur = parent.firstChild;
    while (cur) {
      const next = cur.nextSibling;
      if (!desiredSet.has(cur)) parent.removeChild(cur);
      cur = next;
    }
    // Pass 2: walk desired, insert/move into the correct position. The
    // existing-child check is essential — without it `insertBefore` of a
    // node already at index `i` would still detach + reattach (unnecessary
    // layout work).
    for (let i = 0; i < desired.length; i++) {
      const want = desired[i];
      const have = parent.childNodes[i];
      if (have !== want) {
        parent.insertBefore(want, have || null);
      }
    }
  }

  // Pull `<pasted-text id="N">…</pasted-text>` chunks out of a user-message
  // text body. The send path wraps every chip with these tags (see send
  // handler) so the model sees the paste with clear delimiters AND the bubble
  // can re-display it as a collapsible chip card. Pre-fix messages don't have
  // tags — they fall through with zero chips and the original text intact.
  // Pull `<workflow-tag name="...">…</workflow-tag>` chunks out of a user
  // message so the bubble shows a chip instead of inlining the full body. The
  // model still receives the wrapped body verbatim — only the rendered bubble
  // collapses it.
  const WORKFLOW_TAG_RE = /<workflow-tag name="([^"]*)">\n?([\s\S]*?)\n?<\/workflow-tag>/g;
  function extractWorkflowChips(text) {
    const t = text || '';
    if (t.indexOf('<workflow-tag') < 0) return { workflows: [], cleanedText: t };
    const workflows = [];
    const cleanedText = t
      .replace(WORKFLOW_TAG_RE, (_, name) => {
        workflows.push({ name: String(name || '').replace(/&quot;/g, '"') });
        return '';
      })
      .replace(/\n{3,}/g, '\n\n')
      .trim();
    return { workflows, cleanedText };
  }

  // Non-clickable workflow chip for the rendered user bubble. Reuses the
  // .paste-chip pill styling but overrides the pointer cursor and skips the
  // click handler — opening the full workflow text isn't useful since the
  // user already authored the workflow elsewhere.
  function renderBubbleWorkflowChip(wf) {
    const chipEl = el('div', { class: 'paste-chip paste-chip--workflow', title: `Workflow: ${wf.name}` });
    chipEl.style.cursor = 'default';
    chipEl.appendChild(icon(tagIconPath('workflow'), 12));
    chipEl.appendChild(el('span', { class: 'paste-chip__label' }, `Workflow: ${wf.name}`));
    return chipEl;
  }

  // Composer appends an `<attached-images>…</attached-images>` block to the
  // outgoing text when the user pastes images, so the model has the saved
  // disk path(s) to feed back into image_create / video_create / animate. The
  // user shouldn't see this block in the rendered bubble — the inline image
  // chips below already convey "you attached an image", and the path note is
  // purely a hint for the agent. Strip on render in every preview path
  // (message bubble, queued preview).
  const ATTACHED_IMAGES_RE = /\n*<attached-images>[\s\S]*?<\/attached-images>\n*/g;
  function stripAttachedImagesNote(text) {
    const t = text || '';
    if (t.indexOf('<attached-images>') < 0) return t;
    return t.replace(ATTACHED_IMAGES_RE, '\n\n').replace(/\n{3,}/g, '\n\n').trim();
  }

  const PASTED_TEXT_RE = /<pasted-text id="(\d+)">\n?([\s\S]*?)\n?<\/pasted-text>/g;
  function extractPastedChips(text) {
    const t = text || '';
    const hasMarker = t.indexOf('<pasted-text') >= 0;
    if (!hasMarker) {
      // Sample first 80 chars so we can see whether the wrapper got stripped
      // somewhere in the round-trip (DB → optimistic state → bubble).
      console.log(`[chip][extract] no marker — text.length=${t.length}, head=${JSON.stringify(t.slice(0, 80))}`);
      return { chips: [], cleanedText: t };
    }
    const chips = [];
    const cleanedText = t
      .replace(PASTED_TEXT_RE, (_, idStr, body) => {
        const id = parseInt(idStr, 10);
        chips.push({ id: Number.isFinite(id) ? id : (chips.length + 1), text: body });
        return '';
      })
      .replace(/\n{3,}/g, '\n\n')
      .trim();
    console.log(`[chip][extract] found ${chips.length} chip(s) — input.length=${t.length}, cleaned.length=${cleanedText.length}`);
    return { chips, cleanedText };
  }

  // Count newlines without allocating a per-line string array. `.split('\n')`
  // on a 500 KB paste creates 50k+ short strings just to read `.length`.
  function countNewlines(s) {
    let n = 0;
    for (let i = 0; i < s.length; i++) if (s.charCodeAt(i) === 10) n++;
    return n;
  }

  // Render a paste chip inside a user message bubble. Visual + interaction
  // mirror the input-area chip (same `.paste-chip` class, click opens a
  // scratch buffer with the full content). No remove button — once sent it's
  // part of the conversation history.
  function renderBubblePasteChip(chip) {
    const lineCount = countNewlines(chip.text) + 1;
    const chipEl = el('div', { class: 'paste-chip', title: chip.text.slice(0, 120) });
    chipEl.appendChild(el('span', { class: 'paste-chip__icon' }, '📋'));
    chipEl.appendChild(el('span', { class: 'paste-chip__label' }, `Pasted text #${chip.id} · ${lineCount} ${lineCount === 1 ? 'line' : 'lines'}`));
    chipEl.addEventListener('click', async () => {
      try {
        const info = await api.openScratchBuffer(`Pasted text #${chip.id}`, chip.text, 'text');
        if (!info) return;
        const { editorStore, setActiveBuffer } = await import('../../state/editor.js');
        const buf = { id: info.id, filePath: info.file_path, fileName: info.file_name, projectName: '', lineCount: info.line_count, language: info.language, isModified: false, fileType: 'code', isPreview: false, isDualMode: false, viewMode: 'edit' };
        editorStore.setState({ openBuffers: { ...editorStore.getState('openBuffers'), [info.id]: buf } });
        setActiveBuffer(info.id);
      } catch (err) {
        console.error('Failed to open pasted text in editor:', err);
      }
    });
    return chipEl;
  }

  function renderMessages(task) {
    // Cancel any pending streaming markdown timers - this full render supersedes them
    for (const [k, t] of streamingMarkdownTimers) { clearTimeout(t); streamingMarkdownTimers.delete(k); }
    // Capture scroll state before clearing so we can restore it
    const prevDistFromBottom =
      messagesArea.scrollHeight - messagesArea.scrollTop - messagesArea.clientHeight;
    const wasAtBottom = prevDistFromBottom <= 80;

    // ── Double-buffered render ──
    // Building into a detached DocumentFragment and swapping it in via
    // `replaceChildren` at the end keeps the visible DOM stable for the full
    // duration of the rebuild. Combined with the keyed cache above, unchanged
    // nodes keep their DOM identity (and thus animation state) — only nodes
    // whose fingerprint actually changed are rebuilt.
    const pendingArea = document.createDocumentFragment();

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

    // ── Whole-render short-circuit ───────────────────────────────────────
    // Compute the fingerprint of every keyed node up front. If it's
    // identical to the last render's fingerprint, the new DOM would be
    // byte-for-byte identical to what's already on screen — skip the swap
    // entirely. This is the key fix for the "redundant tasks-sub events
    // cause flicker" pattern: even when every node is a cache hit, the
    // `replaceChildren` still moves elements through a detached fragment,
    // and a burst of 3+ such no-op renders in the same frame paints as a
    // flash. Avoiding the fragment build when nothing changed eliminates
    // the redundant DOM mutation entirely.
    const fingerprintParts = [];
    for (const node of nodes) {
      const k = nodeKey(node);
      if (!k) {
        fingerprintParts.push(`u:${node.type}`);
      } else {
        fingerprintParts.push(`${k}@${nodeVersion(node, task)}`);
      }
    }
    const fingerprint = fingerprintParts.join('|');
    if (fingerprint === lastRenderFingerprint) {
      if (window.__rusticDebugCache) {
        console.log(`[render-msgs] skipped — fingerprint unchanged (${nodes.length} nodes)`);
      }
      return;
    }
    lastRenderFingerprint = fingerprint;

    // Helper: is this node an "activity" (connected by the timeline line)?
    const isActivityNode = (n) => ['thinking', 'thinking-indicator', 'tool-use', 'collapsed-group', 'parallel-group', 'context-condense', 'assistant-text'].includes(n.type);

    // Render a single node into a DOM element (returns null to skip)
    const renderNodeEl = (node) => {
      switch (node.type) {
        case 'task-complete': {
          const b = node.content;

          const card = el('div', { class: 'chat-task-complete' });

          const header = el('div', { class: 'chat-task-complete__header' });
          const checkIcon = icon('M5 12l5 5L20 7', 13);
          header.appendChild(checkIcon);
          header.appendChild(el('span', { class: 'chat-task-complete__label' }, 'Task complete'));
          card.appendChild(header);

          if (b.summary) {
            logBigString('task-complete.summary', b.summary);
            const body = el('div', { class: 'chat-task-complete__body md' });
            try {
              const html = timeSync('task-complete.renderMarkdown', () => renderMarkdown(b.summary));
              timeSync('task-complete.body.innerHTML', () => { body.innerHTML = html; });
            } catch {
              body.textContent = b.summary;
            }
            timeSync('task-complete.attachCodeCopyButtons', () => attachCodeCopyButtons(body));
            card.appendChild(body);

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
          }

          return card;
        }
        case 'context-condense': {
          return renderContextCondenseIndicator(node.content);
        }
        case 'model-switch': {
          const m = node.content.to_model, cur = task.model || task.info?.model || '', same = m === cur;
          // Carry the provider_type so subscription harnesses (Claude Code /
          // Codex) can prefix the model with the harness name. Older marker
          // rows persisted before this field landed fall back to the task's
          // current provider_type, which is correct for any chat that hasn't
          // switched providers mid-session.
          const providerType = node.content.provider_type
            || task?.provider_type
            || task?.info?.provider_type
            || '';
          return renderModelSwitchSeparator(
            m,
            same && thinkingEnabled ? thinkingEffort : null,
            same && thinkingEnabled ? thinkingBudget : null,
            providerType,
          );
        }
        case 'user-message': {
          const msg = node.msg, i = node.msgIdx;
          // `data-msg-idx` lets the in-place updaters
          // (updateCostPillsInPlace) find this bubble without re-running
          // the full render pipeline.
          const msgEl = el('div', { class: 'chat-message chat-message--user', 'data-msg-idx': String(i) });
          const textBlocks = msg.content.filter(b => b.type === 'text' && b.text);
          const imageBlocks = msg.content.filter(b => b.type === 'image' && b.data);

          // Pull pasted-text and workflow chips out of each block so they
          // render as chip cards at the top of the bubble instead of getting
          // inlined as a wall of text. cleanedText is what ends up in the body.
          const parsedBlocks = textBlocks.map(b => {
            const stripped = stripAttachedImagesNote(b.text);
            const w = extractWorkflowChips(stripped);
            const p = extractPastedChips(w.cleanedText);
            return { workflows: w.workflows, chips: p.chips, cleanedText: p.cleanedText };
          });
          const allWorkflows = parsedBlocks.flatMap(p => p.workflows);
          const allChips = parsedBlocks.flatMap(p => p.chips);
          const bodyTexts = parsedBlocks.map(p => p.cleanedText);

          // Line count drives the "Show more" collapse — count only the
          // visible body text so a 5,000-line paste doesn't collapse a
          // one-line typed message that happens to carry a chip.
          const totalLines = bodyTexts.reduce((n, t) => n + (t ? countNewlines(t) + 1 : 0), 0);
          const needsCollapse = totalLines > 3;
          const stateKey = 'user-collapse-' + i;
          const isExpanded = !!expandedState.get(stateKey);
          const bodyClass = needsCollapse && !isExpanded ? 'chat-message__user-body chat-message__user-body--collapsed' : 'chat-message__user-body';
          const bodyEl = el('div', { class: bodyClass });

          if (allWorkflows.length > 0 || allChips.length > 0) {
            const chipsRow = el('div', { class: 'chat-message__paste-chips' });
            for (const wf of allWorkflows) chipsRow.appendChild(renderBubbleWorkflowChip(wf));
            for (const chip of allChips) chipsRow.appendChild(renderBubblePasteChip(chip));
            bodyEl.appendChild(chipsRow);
          }

          for (const cleaned of bodyTexts) {
            if (!cleaned) continue;
            const t = el('div', { class: 'chat-message__text' });
            const lines = cleaned.split('\n');
            for (let li = 0; li < lines.length; li++) {
              if (li > 0) t.appendChild(document.createElement('br'));
              t.appendChild(document.createTextNode(lines[li]));
            }
            bodyEl.appendChild(t);
          }
          if (imageBlocks.length > 0) {
            const imgChips = el('div', { class: 'chat-message__img-chips' });
            for (const b of imageBlocks) {
              const img = el('img', { class: 'chat-message__image-chip', src: 'data:' + b.media_type + ';base64,' + b.data, title: 'Click to expand' });
              img.addEventListener('click', () => openImageLightbox(img.src));
              imgChips.appendChild(img);
            }
            bodyEl.appendChild(imgChips);
          }
          msgEl.appendChild(bodyEl);
          if (needsCollapse) {
            const expandBtn = el('button', { class: 'chat-message__expand-btn', title: isExpanded ? 'Show less' : 'Show more' });
            expandBtn.textContent = isExpanded ? 'Show less' : 'Show more';
            const chevEl = el('span', { class: 'chat-message__expand-chevron' });
            chevEl.appendChild(icon('M19 9l-7 7-7-7', 10));
            if (isExpanded) chevEl.style.transform = 'rotate(180deg)';
            expandBtn.appendChild(chevEl);
            expandBtn.addEventListener('click', (e) => {
              e.stopPropagation();
              const nowExpanded = !expandedState.get(stateKey);
              expandedState.set(stateKey, nowExpanded);
              bodyEl.classList.toggle('chat-message__user-body--collapsed', !nowExpanded);
              expandBtn.childNodes[0].textContent = nowExpanded ? 'Show less' : 'Show more';
              expandBtn.title = nowExpanded ? 'Show less' : 'Show more';
              chevEl.style.transform = nowExpanded ? 'rotate(180deg)' : '';
            });
            msgEl.appendChild(expandBtn);
          }
          // Per-turn cost pill - tokens + $ spent answering this specific message.
          const tu = msg.turnUsage;
          if (tu && (tu.input || tu.output || tu.cacheRead || tu.cacheWrite)) {
            const sent = (tu.input || 0) + (tu.cacheRead || 0) + (tu.cacheWrite || 0);
            const recv = tu.output || 0;
            const costTxt = tu.cost > 0
              ? (tu.cost < 0.001 ? '<$0.001' : '$' + tu.cost.toFixed(3))
              : '$0';
            const pill = el('div', { class: 'chat-message__turn-usage' });
            pill.title = [
              'Input: ' + (tu.input || 0).toLocaleString(),
              'Output: ' + (tu.output || 0).toLocaleString(),
              'Cache read: ' + (tu.cacheRead || 0).toLocaleString(),
              'Cache write: ' + (tu.cacheWrite || 0).toLocaleString(),
              'Cost: $' + (tu.cost || 0).toFixed(4),
            ].join('\n');
            pill.appendChild(el('span', { class: 'turn-usage__sent' }, '\u2191' + formatTokens(sent)));
            pill.appendChild(el('span', { class: 'turn-usage__sep' }, ' \u00b7 '));
            pill.appendChild(el('span', { class: 'turn-usage__recv' }, '\u2193' + formatTokens(recv)));
            pill.appendChild(el('span', { class: 'turn-usage__sep' }, ' \u00b7 '));
            pill.appendChild(el('span', { class: 'turn-usage__cost' }, costTxt));
            msgEl.appendChild(pill);
          }
          const actions = el('div', { class: 'chat-message__actions chat-message__actions--user' });
          const copyBtn = el('button', { class: 'chat-message__action-btn', title: 'Copy' });
          copyBtn.appendChild(icon('M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z', 13));
          copyBtn.addEventListener('click', (e) => { e.stopPropagation(); navigator.clipboard.writeText(extractMessageText(msg)).catch(() => {}); copyBtn.title = 'Copied!'; setTimeout(() => { copyBtn.title = 'Copy'; }, 1500); });
          actions.appendChild(copyBtn);

          const revertBtn = el('button', {
            class: 'chat-message__action-btn',
            title: 'Revert from here',
          });
          // Counter-clockwise arrow (Heroicons "arrow-uturn-left").
          revertBtn.appendChild(icon('M3 10h10a8 8 0 0 1 8 8v2M3 10l6 6M3 10l6-6', 13));
          revertBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            handlePerMessageRevertClick(i, extractMessageText(msg));
          });
          actions.appendChild(revertBtn);

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
            // Friendlier error bubble: if the block carries errorMeta, render
            // a structured card with classification + Retry / Open Settings
            // actions instead of dumping the raw exception as text.
            if (b.errorMeta) {
              w.appendChild(renderErrorBubble(b.errorMeta));
              continue;
            }
            const isStreaming = s && b === last;
            const t = el('div', { class: `chat-message__text${isStreaming ? ' chat-message__text--streaming' : ''}` });
            logBigString('assistant-text.block', b.text);
            const html = timeSync('assistant-text.formatText', () => formatText(b.text));
            timeSync('assistant-text.innerHTML', () => { t.innerHTML = html; });
            // Don't add buttons to the actively-streaming block — it rebuilds every delta.
            // They're added once streaming finishes and renderMessages re-runs without the class.
            if (!isStreaming) timeSync('assistant-text.attachCodeCopyButtons', () => attachCodeCopyButtons(t));
            w.appendChild(t);
          }
          return w;
        }
        case 'tool-use': {
          if (node.toolName === 'todo_write') return renderMinimalToolIndicator('todo_write', node.block, node.toolResult);
          if (node.toolName === 'spawn_subagent' || node.toolName === 'Task') return renderSubagentCard(node.block, node.toolResult);
          if (node.toolName === 'wait_for_subagents' || node.toolName === 'list_active_agents') return renderMinimalToolIndicator(node.toolName, node.block, node.toolResult);
          return renderToolCallCard(node.block, node.toolResult);
        }
        case 'collapsed-group': return renderCollapsedGroup(node);
        case 'parallel-group': return renderParallelGroup(node);
      }
      return null;
    };

    // Render nodes — group consecutive activity nodes into timeline sections.
    // "Transparent" node types (model-switch) render to null
    // most of the time and should NOT break an ongoing timeline when they do.
    const isTransparentNode = (n) => n.type === 'model-switch';

    // Memoized wrapper: reuse the cached DOM element when the node's
    // version is unchanged, otherwise build fresh and update the cache.
    // Tracks every key visited this pass so we can prune stale entries
    // after the swap.
    const usedNodeKeys = new Set();
    let cacheHits = 0;
    let cacheMisses = 0;
    const missDetails = [];
    const renderNodeMemo = (node) => {
      const key = nodeKey(node);
      if (!key) {
        // Untracked node type — anything that hits this path gets logged
        // so we can spot keying gaps.
        const fresh = timeSync(`renderNodeEl:${node.type}`, () => renderNodeEl(node));
        if (fresh) {
          cacheMisses++;
          if (window.__rusticDebugCache) missDetails.push(`unkeyed:${node.type}`);
        }
        return fresh;
      }
      const version = nodeVersion(node, task);
      usedNodeKeys.add(key);
      const cached = nodeRenderCache.get(key);
      if (cached && cached.version === version) {
        cacheHits++;
        return cached.element;
      }
      cacheMisses++;
      if (window.__rusticDebugCache) {
        const why = !cached ? 'new' : `v:${cached.version}→${version}`;
        missDetails.push(`${key}(${why})`);
      }
      const fresh = timeSync(`renderNodeEl:${key}`, () => renderNodeEl(node));
      if (fresh) nodeRenderCache.set(key, { version, element: fresh });
      return fresh;
    };

    // ── Build the desired list of top-level children for messagesArea ────
    // Activity nodes get bucketed into a timeline wrapper; everything else
    // becomes a direct child. Both the timeline wrappers and the per-item
    // wrappers are reused across renders via the maps above so their CSS
    // animations / pseudo-elements (the timeline's vertical line) don't
    // restart on every event.
    const topLevelChildren = [];
    const usedTimelineKeys = new Set();
    const usedItemKeys = new Set();
    let currentTimelineKey = null;
    let currentTimelineItems = null;

    function flushTimeline() {
      if (!currentTimelineKey) return;
      let wrapper = timelineWrappers.get(currentTimelineKey);
      if (!wrapper) {
        wrapper = el('div', { class: 'activity-timeline' });
        timelineWrappers.set(currentTimelineKey, wrapper);
      }
      reconcileChildren(wrapper, currentTimelineItems);
      topLevelChildren.push(wrapper);
      usedTimelineKeys.add(currentTimelineKey);
      currentTimelineKey = null;
      currentTimelineItems = null;
    }

    for (const node of nodes) {
      if (isActivityNode(node)) {
        const rendered = renderNodeMemo(node);
        if (!rendered) continue;
        const itemKey = nodeKey(node) || `anon-${currentTimelineItems?.length ?? 0}`;
        if (!currentTimelineKey) currentTimelineKey = itemKey;
        if (!currentTimelineItems) currentTimelineItems = [];
        // Reuse the activity-timeline__item wrapper for this node so its
        // identity (and any CSS state on it) persists across renders. The
        // wrapper's only child is the rendered card; if the card was
        // rebuilt because of a cache miss, swap in the new one. Otherwise
        // leave the wrapper untouched.
        let item = itemWrappers.get(itemKey);
        if (!item) {
          item = el('div', { class: 'activity-timeline__item' });
          itemWrappers.set(itemKey, item);
        }
        if (item.firstChild !== rendered) {
          item.replaceChildren(rendered);
        }
        usedItemKeys.add(itemKey);
        currentTimelineItems.push(item);
      } else if (isTransparentNode(node)) {
        const rendered = renderNodeMemo(node);
        if (rendered) {
          flushTimeline();
          topLevelChildren.push(rendered);
        }
        // If null, just skip — timeline stays intact.
      } else {
        flushTimeline();
        const rendered = renderNodeMemo(node);
        if (rendered) topLevelChildren.push(rendered);
      }
    }
    flushTimeline();

    if (window.__rusticDebugCache) {
      const total = cacheHits + cacheMisses;
      console.log(
        `[render-msgs] nodes=${nodes.length} hits=${cacheHits}/${total}` +
        (missDetails.length ? ` misses=[${missDetails.join(', ')}]` : '')
      );
    }

    // ── Reconcile messagesArea in place ───────────────────────────────────
    // Direct minimum-mutation diff against the live DOM — children that are
    // already at the right index aren't touched at all. No fragment, no
    // `replaceChildren` swap; CSS animations on every wrapper survive.
    reconcileChildren(messagesArea, topLevelChildren);

    // Prune caches: drop wrapper entries that weren't used this render so
    // they don't grow unboundedly across long conversations.
    let pruned = 0;
    for (const key of nodeRenderCache.keys()) {
      if (!usedNodeKeys.has(key)) { nodeRenderCache.delete(key); pruned++; }
    }
    for (const key of timelineWrappers.keys()) {
      if (!usedTimelineKeys.has(key)) timelineWrappers.delete(key);
    }
    for (const key of itemWrappers.keys()) {
      if (!usedItemKeys.has(key)) itemWrappers.delete(key);
    }
    if (window.__rusticDebugCache && pruned) {
      console.log(`[render-msgs] pruned ${pruned} stale cache entries (size now ${nodeRenderCache.size})`);
    }

    // Task-switch scroll overrides normal auto-scroll logic.
    if (pendingTaskSwitchScroll === 'bottom') {
      pendingTaskSwitchScroll = null;
      messagesArea.scrollTop = messagesArea.scrollHeight;
    } else if (pendingTaskSwitchScroll === 'top') {
      pendingTaskSwitchScroll = null;
      messagesArea.scrollTop = 0;
    } else if (wasAtBottom) {
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
      .map((b) => b.text
        .replace(ATTACHED_IMAGES_RE, '')
        .replace(WORKFLOW_TAG_RE, (_, _name, body) => body)
        .replace(PASTED_TEXT_RE, (_, _id, body) => body))
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

      // Buttons only — no countdown, wait indefinitely for user response.
      // Three buttons for both harness and native tasks. `acceptForSession`
      // semantics differ slightly:
      //   • Harness (Claude Code) — uses the CLI's own session rules via
      //     `addRules` with destination `session`.
      //   • Native — the broker keeps an in-memory per-task allowlist keyed
      //     by an op-shape signature (run_command:<bin>, write_file,
      //     create_file). Sensitive-file tiers are intentionally excluded —
      //     they always re-prompt regardless of decision.
      const actions = el('div', { class: 'chat-approval-widget__actions' });

      const denyBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--deny' }, 'Deny');
      const allowBtn = el('button', { class: 'chat-approval-widget__btn chat-approval-widget__btn--allow' }, 'Allow');

      denyBtn.addEventListener('click', () => {
        respondToPermission(taskId, req.request_id, 'deny');
      });

      allowBtn.addEventListener('click', () => {
        respondToPermission(taskId, req.request_id, 'accept');
      });

      actions.appendChild(denyBtn);

      const allowSessionBtn = el(
        'button',
        { class: 'chat-approval-widget__btn chat-approval-widget__btn--allow-session', title: 'Allow this tool for the rest of this conversation without prompting again.' },
        'Allow for session'
      );
      allowSessionBtn.addEventListener('click', () => {
        respondToPermission(taskId, req.request_id, 'acceptForSession');
      });
      actions.appendChild(allowSessionBtn);

      actions.appendChild(allowBtn);
      widget.appendChild(actions);
      approvalArea.appendChild(widget);
    }
  }

  // Preserve expand state across re-renders (queue mutations cause full
  // repaints of the area, so we'd otherwise collapse on every change).
  let queuedExpanded = false;

  /// Render the queued-input panel between the message list and the input
  /// box. Mirrors the changed-files panel: a single toggle header showing
  /// the count, expanding to reveal each queued message with a dismiss
  /// button. Empty queue → hidden, no layout cost.
  function renderQueuedArea() {
    const taskId = agentStore.getState('activeTaskId');
    queuedArea.innerHTML = '';
    queuedArea.classList.remove('chat-queued-area--visible');
    if (!taskId) return;
    const queue = (agentStore.getState('pendingUserInput') || {})[taskId] || [];
    if (queue.length === 0) {
      queuedExpanded = false;
      return;
    }

    const toggle = el('div', { class: 'chat-queued-area__toggle' });
    const arrowIcon = icon('M19 9l-7 7-7-7', 14);
    arrowIcon.style.transition = 'transform 0.15s';
    if (queuedExpanded) arrowIcon.style.transform = 'rotate(180deg)';
    toggle.appendChild(arrowIcon);
    toggle.appendChild(
      el('span', {}, `${queue.length} message${queue.length !== 1 ? 's' : ''} queued`)
    );
    const pill = el('span', {
      class: 'chat-queued-area__pill',
      title: 'Will be sent automatically when the current turn ends.',
    }, 'QUEUED');
    toggle.appendChild(pill);
    queuedArea.appendChild(toggle);

    const list = el('div', { class: 'chat-queued-area__list' });
    if (!queuedExpanded) list.classList.add('chat-queued-area__list--collapsed');

    for (let i = 0; i < queue.length; i++) {
      const item = queue[i];
      const row = el('div', { class: 'chat-queued-area__row' });
      // Show a clean preview — strip the `<pasted-text id="N">…</pasted-text>`
      // and `<workflow-tag name="…">…</workflow-tag>` wrappers that
      // buildOutgoingText emits so the user sees a short label/typed text
      // instead of the marker tags or the full workflow body.
      const previewSource = (item.text || '')
        .replace(ATTACHED_IMAGES_RE, '')
        .replace(WORKFLOW_TAG_RE, (_, name) => `Workflow: ${name}`)
        .replace(PASTED_TEXT_RE, (_, _id, body) => body);
      const text = el('span', { class: 'chat-queued-area__text' }, previewSource.slice(0, 240));
      if (previewSource.length > 240) text.textContent += '…';
      const dismiss = el('button', {
        class: 'chat-queued-area__dismiss',
        type: 'button',
        title: 'Discard this queued message',
      }, '×');
      dismiss.addEventListener('click', (e) => {
        e.stopPropagation();
        clearQueuedMessage(taskId, i);
      });
      row.appendChild(text);
      row.appendChild(dismiss);
      list.appendChild(row);
    }

    queuedArea.appendChild(list);

    toggle.style.cursor = 'pointer';
    toggle.addEventListener('click', () => {
      queuedExpanded = !queuedExpanded;
      list.classList.toggle('chat-queued-area__list--collapsed', !queuedExpanded);
      arrowIcon.style.transform = queuedExpanded ? 'rotate(180deg)' : '';
    });

    queuedArea.classList.add('chat-queued-area--visible');
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

  // ── DEBUG: render-flicker diagnostics ─────────────────────────────────────
  // Toggle these flags via the console to drill into a flicker repro:
  //   window.__rusticDebugRender   — log every scheduleFullRender call + reason
  //   window.__rusticDebugCache    — log per-node cache hit/miss inside renderMessages
  //   window.__rusticDebugSubs     — log every subscriber that fires
  // Default to ON so the next repro produces a transcript without further setup.
  if (typeof window !== 'undefined') {
    if (window.__rusticDebugRender === undefined) window.__rusticDebugRender = true;
    if (window.__rusticDebugCache === undefined)  window.__rusticDebugCache  = true;
    if (window.__rusticDebugSubs === undefined)   window.__rusticDebugSubs   = true;
  }
  let renderTickCounter = 0;
  let pendingRenderReason = null;

  function scheduleFullRender(reason) {
    if (reason && pendingRenderReason !== reason) {
      // Keep the most recent reason — useful when several subscribers
      // schedule a render in the same frame.
      pendingRenderReason = reason;
    }
    if (renderRafId) {
      if (window.__rusticDebugRender) {
        console.log(`[render] coalesced (pending: ${pendingRenderReason || 'unknown'})`);
      }
      cancelAnimationFrame(renderRafId);
    }
    renderRafId = requestAnimationFrame(() => {
      renderRafId = null;
      const tick = ++renderTickCounter;
      const r = pendingRenderReason || 'unknown';
      pendingRenderReason = null;
      if (window.__rusticDebugRender) {
        console.log(`[render] tick #${tick} firing — reason: ${r}`);
      }
      const t0 = performance.now();
      render();
      const dt = (performance.now() - t0).toFixed(1);
      if (window.__rusticDebugRender) {
        console.log(`[render] tick #${tick} done in ${dt}ms`);
      }
    });
  }

  agentStore.subscribe('lastRequestUsage', () => {
    if (window.__rusticDebugSubs) console.log('[lastRequestUsage-sub] fired');
    // Context % is driven off the LAST request's input/cache tokens — refresh
    // the progress ring (and its tooltip) whenever a new usage report lands.
    updateContextBadge();
    updateCostDisplay();
  });

  // Re-render queued bubbles whenever the queue changes (queueMessage,
  // clearQueuedMessage, drainPendingUserInput in agent.js all mutate it).
  agentStore.subscribe('pendingUserInput', () => {
    if (window.__rusticDebugSubs) console.log('[pendingUserInput-sub] fired');
    renderQueuedArea();
  });

  // ── In-place mutators for the bits of a user-message that change without
  // a real content change. These let the tasks-subscriber reflect cost-pill
  // and revert-button updates immediately without invalidating the cached
  // DOM for the bubble. Without them we'd either flicker the bubble (cache
  // miss every API call) or starve the user of live cost feedback (no
  // update at all).

  /// Build (or return null to remove) the cost-pill DOM for a user-message
  /// from a turnUsage object. Mirrors the markup in `renderNodeEl`'s
  /// `user-message` branch so the in-place update produces identical HTML.
  function buildTurnUsagePill(tu) {
    if (!tu || (!tu.input && !tu.output && !tu.cacheRead && !tu.cacheWrite)) {
      return null;
    }
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
    return pill;
  }

  function updateCostPillsInPlace(task) {
    if (!task) return;
    const bubbles = messagesArea.querySelectorAll('.chat-message--user[data-msg-idx]');
    for (const bubble of bubbles) {
      const idx = parseInt(bubble.dataset.msgIdx, 10);
      const msg = task.messages?.[idx];
      if (!msg) continue;
      const tu = msg.turnUsage;
      const existing = bubble.querySelector(':scope > .chat-message__turn-usage');
      const fresh = buildTurnUsagePill(tu);
      if (!fresh) {
        if (existing) existing.remove();
        continue;
      }
      if (existing) {
        // Update three text spans in place — no DOM destroy/create. The
        // pill stays, the user sees the numbers tick up, no flicker.
        const sentSpan = existing.querySelector('.turn-usage__sent');
        const recvSpan = existing.querySelector('.turn-usage__recv');
        const costSpan = existing.querySelector('.turn-usage__cost');
        const sentText = fresh.querySelector('.turn-usage__sent')?.textContent;
        const recvText = fresh.querySelector('.turn-usage__recv')?.textContent;
        const costText = fresh.querySelector('.turn-usage__cost')?.textContent;
        if (sentSpan && sentText) sentSpan.textContent = sentText;
        if (recvSpan && recvText) recvSpan.textContent = recvText;
        if (costSpan && costText) costSpan.textContent = costText;
        existing.title = fresh.title;
      } else {
        // Insert before the actions row (which is the last child).
        const actions = bubble.querySelector(':scope > .chat-message__actions--user');
        if (actions) bubble.insertBefore(fresh, actions);
        else bubble.appendChild(fresh);
      }
    }
  }

  /// Walk every visible subagent card and fold the latest live state from
  /// the `subagents` store into it. This is the hot-path for subagent text
  /// deltas: previously each delta triggered a full `renderMessages`, which
  /// did a `replaceChildren` on the entire conversation 10+ times per
  /// second even when every node was a cache hit. The repeated DOM moves
  /// (cached children → fragment → back) read as flicker even though the
  /// painted content was identical.
  ///
  /// Now subagent text deltas only mutate the small parts of each card
  /// that actually changed (token counts, cost, words, answer button
  /// visibility, status icon). The rest of the messages area is untouched.
  function updateSubagentCardsInPlace() {
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) return;
    const subagents = agentStore.getState('subagents')?.[taskId];
    if (!subagents) return;
    const cards = messagesArea.querySelectorAll('.subagent-card[data-subagent-id]');
    let appliedCount = 0;
    let skippedNoAgent = 0;
    let skippedTaskMismatch = 0;
    for (const card of cards) {
      // Skip cards that belong to a different task — defensive in case the
      // user switches tasks while subagents are still streaming.
      const cardTask = card.dataset.taskId;
      if (cardTask && cardTask !== taskId) {
        skippedTaskMismatch++;
        continue;
      }
      const agentId = card.dataset.subagentId;
      const agent = subagents[agentId];
      if (!agent) {
        skippedNoAgent++;
        continue;
      }
      // hasResult: signals "tool already finished from the parent's POV"; we
      // pass false here because the runtime status drives the icon during
      // active streaming. If the task itself completes the next renderMessages
      // pass will rebuild the card with its frozen final state.
      applySubagentLiveStateToCard(card, agent, false);
      appliedCount++;
    }
    if (window.__rusticDebugSubs && (cards.length > 0 || skippedNoAgent > 0)) {
      // Only log when there's something interesting to report — silent when
      // there are no cards at all (welcome screen, etc.). Helps diagnose
      // "I see cost updates arriving but the card doesn't change":
      //   - applied=N: the in-place updater ran on N cards.
      //   - skippedNoAgent=N: card present but agent not in store (slug mismatch?).
      //   - skippedTaskMismatch=N: card belongs to a different task.
      if (window.__rusticDebugSubs) console.log(
        `[updateSubagentCards] cards=${cards.length} applied=${appliedCount} ` +
        `skippedNoAgent=${skippedNoAgent} skippedTaskMismatch=${skippedTaskMismatch}`
      );
    }
  }


  // Track the last-seen "shape" of the active task so each tasks-subscriber
  // tick can log *what* actually changed. Pure diagnostics — drives the
  // [tasks-sub] log lines below.
  let lastSeenTaskShape = null;
  function describeTaskShape(task) {
    if (!task) return null;
    const msgs = task.messages || [];
    const last = msgs[msgs.length - 1];
    let lastDesc = '∅';
    if (last) {
      const blocks = (last.content || []).map(b => b.type).join(',');
      const lastBlock = last.content?.[last.content.length - 1];
      const tail = lastBlock?.type === 'text'
        ? `text/${lastBlock.text?.length || 0}`
        : lastBlock?.type === 'thinking'
        ? `think/${lastBlock.thinking?.length || 0}`
        : lastBlock?.type === 'tool_use'
        ? `tool/${lastBlock.name || '?'}`
        : lastBlock?.type === 'tool_result'
        ? `result/${(lastBlock.content || '').length}`
        : (lastBlock?.type || '?');
      lastDesc = `${last.role}[${blocks}] tail=${tail}`;
    }
    return {
      status: task.status,
      isStreaming: !!task.isStreaming,
      msgCount: msgs.length,
      lastDesc,
    };
  }

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

    if (window.__rusticDebugSubs) {
      const shape = describeTaskShape(task);
      const prev = lastSeenTaskShape;
      const diff = [];
      if (prev && shape) {
        if (prev.status !== shape.status) diff.push(`status:${prev.status}→${shape.status}`);
        if (prev.isStreaming !== shape.isStreaming) diff.push(`stream:${prev.isStreaming}→${shape.isStreaming}`);
        if (prev.msgCount !== shape.msgCount) diff.push(`msgs:${prev.msgCount}→${shape.msgCount}`);
        if (prev.lastDesc !== shape.lastDesc) diff.push(`tail:${prev.lastDesc} → ${shape.lastDesc}`);
      } else {
        diff.push('initial');
      }
      console.log(`[tasks-sub] fired — ${diff.length ? diff.join(' | ') : 'no visible diff'}`);
      lastSeenTaskShape = shape;
    }

    if (!task) { scheduleFullRender('tasks-sub:no-task'); return; }

    // Cost pill update in place — this used to be a driver of the
    // user-bubble cache miss (turnUsage flipped 5+ times per multi-tool
    // turn). We now mutate it directly so the pill ticks up live but
    // the bubble keeps its DOM identity. No-op if the bubble doesn't
    // need a change.
    updateCostPillsInPlace(task);

    // During streaming, the most frequent events are text deltas and thinking deltas.
    // We intercept these and do targeted DOM updates to avoid the full rebuild flicker.
    if (task.isStreaming) {
      const msgs = task.messages;
      const lastMsg = msgs[msgs.length - 1];
      if (lastMsg?.role === 'assistant') {
        const lastBlock = lastMsg.content[lastMsg.content.length - 1];

        // ── Fast-path: Text delta ──
        // Render the streaming assistant message as markdown live. The rAF
        // below coalesces a burst of token events into a single paint per
        // frame, so we re-parse at most ~60 times/sec regardless of chunk
        // rate. Code-copy buttons are intentionally skipped while streaming
        // and are attached once the full re-render fires on completion.
        if (lastBlock?.type === 'text') {
          const streamingEl = messagesArea.querySelector('.chat-message__text--streaming');
          if (streamingEl && lastBlock.text) {
            const streamKey = streamingEl.dataset.streamKey || (streamingEl.dataset.streamKey = taskId + ':stream');
            const prevTimer = streamingMarkdownTimers.get(streamKey);
            if (prevTimer) clearTimeout(prevTimer);
            streamingMarkdownTimers.set(streamKey, setTimeout(() => {
              streamingMarkdownTimers.delete(streamKey);
              const liveTask = agentStore.getState('tasks')?.[taskId];
              const liveLast = liveTask?.messages?.[liveTask.messages.length - 1];
              const liveBlock = liveLast?.content?.[liveLast.content.length - 1];
              if (liveBlock?.type === 'text' && typeof liveBlock.text === 'string') {
                try { streamingEl.innerHTML = renderMarkdown(liveBlock.text); } catch { streamingEl.textContent = liveBlock.text; }
                autoScrollIfNeeded();
              }
            }, 50));
            if (window.__rusticDebugSubs) console.log('[tasks-sub] text-delta fast-path — skipping full render');
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
            if (window.__rusticDebugSubs) console.log('[tasks-sub] thinking-delta fast-path — skipping full render');
            return; // Skip full re-render
          }
        }
      }
    }

    // All other state changes — debounced full re-render
    scheduleFullRender('tasks-sub');
  });
  agentStore.subscribe('activeTaskId', () => {
    // Cached node DOM is per-task (keys aren't namespaced by task id) — drop
    // it on a task switch so the new task's first render doesn't accidentally
    // reuse the previous task's tool cards by msgIdx collision.
    const newTaskId = agentStore.getState('activeTaskId');

    // Switching tasks always exits sub-agent view — its state is tied to a
    // specific parent. Without this, a stale view flag could linger and
    // suppress the next regular render once the user comes back.
    if (subagentViewAgentId && subagentViewParentTaskId !== newTaskId) {
      subagentViewAgentId = null;
      subagentViewParentTaskId = null;
      lastRenderedSource = null;
      container.classList.remove('chat-view--subagent-view');
    }

    if (prevActiveTaskId && prevActiveTaskId !== newTaskId) {
      saveDraft(prevActiveTaskId);
      const frag = document.createDocumentFragment();
      while (messagesArea.firstChild) frag.appendChild(messagesArea.firstChild);
      taskMessagesFragments.set(prevActiveTaskId, frag);
      taskRenderCaches.set(prevActiveTaskId, {
        nodeCache: new Map(nodeRenderCache),
        timelines: new Map(timelineWrappers),
        items: new Map(itemWrappers),
        fingerprint: lastRenderFingerprint,
        scrollTop: messagesArea.scrollTop,
      });
    }

    if (newTaskId && taskMessagesFragments.has(newTaskId)) {
      const saved = taskRenderCaches.get(newTaskId);
      messagesArea.replaceChildren(taskMessagesFragments.get(newTaskId));
      const switchedTask = agentStore.getState('tasks')[newTaskId];
      const switchedRunning = switchedTask?.status === 'Running';
      if (switchedRunning) {
        messagesArea.scrollTop = messagesArea.scrollHeight;
      } else {
        messagesArea.scrollTop = 0;
      }
      taskMessagesFragments.delete(newTaskId);
      nodeRenderCache.clear();
      for (const [k, v] of saved.nodeCache) nodeRenderCache.set(k, v);
      timelineWrappers.clear();
      for (const [k, v] of saved.timelines) timelineWrappers.set(k, v);
      itemWrappers.clear();
      for (const [k, v] of saved.items) itemWrappers.set(k, v);
      lastRenderFingerprint = saved.fingerprint;
    } else {
      nodeRenderCache.clear();
      timelineWrappers.clear();
      itemWrappers.clear();
  
    }

    prevActiveTaskId = newTaskId;
    // Same reasoning for the whole-render fingerprint: a different task's
    // node sequence might happen to fingerprint-match, which would silently
    // suppress the first render of the new task.
    if (!taskRenderCaches.has(newTaskId)) lastRenderFingerprint = null;
    const switchingTask = newTaskId ? agentStore.getState('tasks')[newTaskId] : null;
    pendingTaskSwitchScroll = switchingTask?.status === 'Running' ? 'bottom' : 'top';
    scheduleFullRender('task-switch'); updateCostDisplay(); updateHeaderBar(); renderStickyCard(); renderTaskTabs();
    // Apply project defaults (thinking effort) when switching to a new task
    applyProjectDefaults();
    restoreDraft(newTaskId);
    // Drop the previous task's accumulated net-change state and refetch
    // for the new task, so a stale entry from another task isn't shown
    // for a moment after switching.
    netChanges.clear();
    netChangesProjectRoot = null;
    renderChangedFilesPanel();
    scheduleNetChangesRefresh(newTaskId);
    // Re-render the per-task queued bubbles (each task has its own queue).
    renderQueuedArea();
    updateSendBtn();
  });
  // Sub-agent state churns on every tool_use / tool_result / text-delta
  // event. While the user is inside the sub-agent view, those should drive
  // a render of *that* view — the parent task's `tasks` subscriber doesn't
  // fire for sub-agent updates. While the user is on the parent task, the
  // existing in-place updaters handle card refreshes and we don't need a
  // full re-render.
  agentStore.subscribe('subagents', () => {
    if (subagentViewAgentId && subagentViewParentTaskId === agentStore.getState('activeTaskId')) {
      scheduleFullRender('subagents-view');
    }
  });

  // Welcome screen depends on the picked project + the project list.
  agentStore.subscribe('pendingProjectId', () => {
    if (!agentStore.getState('activeTaskId')) render();
    updateSendBtn();
  });
  workspaceStore.subscribe('projects', () => {
    if (!agentStore.getState('activeTaskId')) render();
    updateSendBtn();
  });
  // Provider config lives in localStorage (managed by ai-settings.js); a
  // CustomEvent fires whenever it changes. Re-evaluate the Send button so
  // connecting / disconnecting a provider while the chat is open immediately
  // updates the disabled state and the welcome CTA.
  window.addEventListener('rustic:provider-configs-changed', () => {
    updateSendBtn();
    if (!agentStore.getState('activeTaskId')) render();
  });
  agentStore.subscribe('permissionRequests', () => {
    if (window.__rusticDebugSubs) console.log('[permissionRequests-sub] fired');
    renderApprovalArea();
    renderTaskTabs();
  });
  agentStore.subscribe('todos', () => {
    if (window.__rusticDebugSubs) console.log('[todos-sub] fired');
    renderStickyCard();
  });

  // Subagent state changes — text deltas, cost updates, status flips —
  // arrive at varying rates (text-deltas: many per second; cost updates:
  // every 1-2s per active subagent). We always update via the cheap
  // in-place path (no `replaceChildren`, no fragment), but we still
  // throttle to avoid scheduling a JS task per text-delta.
  //
  // Throttle was 300ms — too long for cost updates to feel responsive
  // (the user's complaint was "input/output tokens don't update during
  // the run, only at the end"). Dropped to 80ms: still tames text-delta
  // floods (max ~12 updates per second), but cost updates that fire
  // every 1-2s now reflect within ~80ms instead of ~300ms.
  //
  // The in-place updater is cheap: a few querySelector calls and text
  // assignments per visible card. No layout-shifting work.
  let subagentRenderTimer = null;
  agentStore.subscribe('subagents', () => {
    if (window.__rusticDebugSubs) console.log('[subagents-sub] fired (throttled)');
    if (subagentRenderTimer) return;
    subagentRenderTimer = setTimeout(() => {
      subagentRenderTimer = null;
      updateSubagentCardsInPlace();
      updateCostDisplay();
    }, 80);
  });

  render();
  updateCostDisplay();
  updateHeaderBar();
  renderStickyCard();
  renderTaskTabs();
  // App-start path: if a task is already active when chat-view mounts (e.g.
  // hydrated from disk before our task-switch subscriber attached), pull
  // the cumulative changed-files view for it.
  scheduleNetChangesRefresh(agentStore.getState('activeTaskId'));

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

  // Cycle through words every 2.5 s. Self-cancels when the indicator is
  // removed from the DOM — previously this used a MutationObserver on
  // `document.body` with subtree:true, which fires on every DOM mutation in
  // the entire app (hundreds per second during streaming) just to check
  // whether one element is still attached.
  let idx = THINKING_WORDS.indexOf(wordEl.textContent);
  const timer = setInterval(() => {
    if (!wrapper.isConnected) {
      clearInterval(timer);
      return;
    }
    idx = (idx + 1) % THINKING_WORDS.length;
    wordEl.classList.add('chat-thinking-indicator__word--fade');
    setTimeout(() => {
      wordEl.textContent = THINKING_WORDS[idx];
      wordEl.classList.remove('chat-thinking-indicator__word--fade');
    }, 250);
  }, 2500);

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

    // Update every second; self-cancel when the thinking card is unmounted.
    // Previously used a MutationObserver on document.body+subtree, which
    // wakes on every DOM change in the app — replaced with an isConnected
    // check inside the timer tick.
    const timer = setInterval(() => {
      if (!card.isConnected) {
        clearInterval(timer);
        return;
      }
      updateMeta();
    }, 1000);
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
  const rawText = (typeof input.text === 'string' && input.text.trim())
    || (typeof input.question === 'string' && input.question.trim())
    || '';
  // Empty/malformed payloads (e.g. the model called chat_message with `{}`)
  // get a placeholder rather than rendering `{}` as the body. The server-side
  // tool already returns is_error in this case so the model retries — this
  // just prevents the in-flight card from looking broken.
  const text = rawText || '*(empty message — agent will retry)*';
  const msgType = input.type === 'question' ? 'question' : 'message';
  // A question is "answered" if either (a) the backend produced a tool_result
  // (live broker round-trip) or (b) the user clicked a choice on a stale
  // restored card and we recorded it locally in `pickedChoiceState`.
  const localPick = pickedChoiceState.get(id);
  const isAnswered = !!result || !!localPick;
  const isPending = !isAnswered;
  const hasResponse = (result && !result.is_error) || !!localPick;

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

  // Choice buttons (only for pending questions with choices)
  if (isQuestion && isPending) {
    const choices = Array.isArray(input.choices) ? input.choices : [];
    if (choices.length > 0) {
      const choicesEl = el('div', { class: 'chat-msg-card__choices' });

      // Local guard: the first click flips this so any rapid follow-up
      // clicks (e.g. double-tap, keyboard activation) on sibling buttons
      // are ignored before the DOM has been swapped out for the answered
      // state. Without this the user could send two messages by clicking
      // two choices in quick succession.
      let locked = false;
      const lockAndShow = (choice) => {
        if (locked) return false;
        locked = true;
        pickedChoiceState.set(id, choice);
        // Replace the choices block with a "Your response" footer so the
        // card matches its post-answer appearance immediately, without
        // waiting for the next renderMessages pass.
        const responseEl = el('div', { class: 'chat-msg-card__response' });
        responseEl.appendChild(el('span', { class: 'chat-msg-card__response-label' }, 'Your response:'));
        responseEl.appendChild(el('span', {}, choice));
        choicesEl.replaceWith(responseEl);
        return true;
      };

      choices.forEach((choice) => {
        const btn = el('button', { class: 'chat-msg-card__choice', type: 'button' }, choice);
        btn.addEventListener('click', () => {
          // Resolve the pending question fresh on every click \u2014 the card may
          // have been built before the `agent-question-request` event arrived
          // (the chat_message tool_use block is rendered via the regular
          // assistant-stream path, which races with the broker event), and
          // the keyed render cache means the original DOM persists across
          // store updates. Reading from the store inside the handler avoids
          // the silent no-op caused by capturing `null` at render time.
          //
          // If there's no live pending request (the most common case after a
          // process restart \u2014 the question was restored from the persisted
          // tool_use block but the backend's pending request was thrown
          // away when the worker thread died), fall back to sending the
          // chosen text as a fresh user message so the conversation can
          // continue.
          const taskId = agentStore.getState('activeTaskId');
          const task = taskId ? agentStore.getState('tasks')[taskId] : null;
          const pq = task?.pendingQuestion;
          if (!lockAndShow(choice)) return;
          if (pq && pq.request_id) {
            respondToAgentQuestion(taskId, pq.request_id, choice);
          } else if (taskId) {
            sendMessage(taskId, choice).catch((err) => {
              console.error('Failed to resume task with choice:', err);
            });
          }
        });
        choicesEl.appendChild(btn);
      });

      const otherBtn = el('button', { class: 'chat-msg-card__choice chat-msg-card__choice--other', type: 'button' }, 'Other\u2026');
      otherBtn.addEventListener('click', () => {
        const chatInput = document.querySelector('.chat-input');
        if (chatInput) chatInput.focus();
      });
      choicesEl.appendChild(otherBtn);

      card.appendChild(choicesEl);
    }
  }

  // Response (only for questions that have been answered)
  if (isQuestion && hasResponse) {
    const responseEl = el('div', { class: 'chat-msg-card__response' });
    responseEl.appendChild(el('span', { class: 'chat-msg-card__response-label' }, 'Your response:'));
    // Prefer the live tool_result content; fall back to the locally-recorded
    // pick when the question was answered after a restart.
    const responseText = result
      ? String(result.content).replace(/^User response:\s*/i, '')
      : String(localPick);
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
 * Render a sub-agent card. The whole card is clickable — it opens the
 * sub-agent's full activity in the chat view (parent UI hides until the
 * user clicks the back button).
 */
let subagentViewAgentId = null;
let subagentViewParentTaskId = null;
// Slot wired by createChatView so the module-scope renderSubagentCard (this
// file) can ask the chat-view component to switch into subagent-view mode.
// The component owns the messagesArea / container DOM that the view needs.
let openSubagentView = () => {};

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
  // `data-subagent-id` lets `updateSubagentCardsInPlace` find this card
  // without re-rendering it. The agent-id is the slug computed above; same
  // value the subagent store keys the live state under.
  const card = el('div', {
    class: `subagent-card subagent-card--clickable${statusClass}`,
    'data-tool-use-id': id,
    'data-subagent-id': agentId,
    'data-task-id': taskId || '',
    title: 'Open sub-agent activity',
  });

  // ── Header row: icon + name + status ──
  const headerRow = el('div', { class: 'subagent-card__header' });

  // Agent icon (purple)
  const iconWrap = el('span', { class: 'tool-call__icon tool-call__icon--purple' });
  iconWrap.appendChild(icon('M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', 13));
  headerRow.appendChild(iconWrap);

  // Agent name (truncated via CSS)
  headerRow.appendChild(el('span', { class: 'subagent-card__name' }, name));

  // Status: spinner | ✓ | ✗ — the in-place updater rewrites this element's
  // contents based on the latest subagent.status.
  const statusEl = el('span', { class: 'tool-call__status' });
  if (isRunning) {
    statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    const checkPath = isFailed ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
    statusEl.appendChild(icon(checkPath, 12));
    statusEl.classList.add(isFailed ? 'tool-call__status--error' : 'tool-call__status--ok');
  }
  headerRow.appendChild(statusEl);

  card.appendChild(headerRow);

  // Whole card is clickable — opens the sub-agent's full activity in the
  // chat view. The full stats (tokens / cost / Answer button) live inside
  // that view's back-bar, so the inline card only needs name + status.
  card.addEventListener('click', () => {
    openSubagentView(taskId, agentId);
  });

  return card;
}


/// In-place mutator for a single sub-agent card. The card now only carries
/// name + status — token / cost / Answer-button live state is shown inside
/// the sub-agent panel (back-bar), not on the inline card.
function applySubagentLiveStateToCard(card, agent, hasResult) {
  if (!card || !agent) return false;
  const status = agent.status || (hasResult ? 'completed' : 'running');
  const isRunning = status === 'running';
  const isFailed = status === 'failed';

  // Status class on the card root.
  card.classList.toggle('subagent-card--failed', isFailed);
  card.classList.toggle('subagent-card--completed', !isRunning && !isFailed);

  // Status icon: spinner ↔ ✓ ↔ ✗. The data-* attribute lets us avoid
  // rebuilding when the status didn't actually flip — without the cache
  // the spinner DOM would be recreated on every text-delta tick and
  // its CSS animation would restart constantly.
  const statusEl = card.querySelector(':scope > .subagent-card__header > .tool-call__status');
  if (statusEl && statusEl.dataset.statusKind !== status) {
    statusEl.replaceChildren();
    statusEl.classList.remove('tool-call__status--error', 'tool-call__status--ok');
    if (isRunning) {
      statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
    } else {
      const checkPath = isFailed ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
      statusEl.appendChild(icon(checkPath, 12));
      statusEl.classList.add(isFailed ? 'tool-call__status--error' : 'tool-call__status--ok');
    }
    statusEl.dataset.statusKind = status;
  }

  return true;
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
 * Render a media-generation tool card (image_create / video_create / animate).
 * Shows: header (icon + label + status), expandable prompt block, and the
 * generated media inline once the tool_result arrives. Saved file paths are
 * parsed out of the ```media-output JSON block the backend tool returns.
 */
function renderMediaToolCard(block, result) {
  const { name, input = {}, id } = block;
  const meta = TOOL_META[name] || { ...TOOL_META_DEFAULT, label: name };
  const label = meta.label || name;
  const isPending = !result;
  const isError = !!result?.is_error;
  const promptText = (input.prompt || '').trim();
  // `image_path` (singular) is used by `animate` and `video_create`. The new
  // `image_create` editing mode passes `image_paths` (array). Surface either
  // shape in the card summary so the user can see what's being edited.
  let sourceImage = (input.image_path || '').trim();
  if (!sourceImage && Array.isArray(input.image_paths) && input.image_paths.length) {
    const list = input.image_paths.map((p) => String(p || '').trim()).filter(Boolean);
    sourceImage = list.length > 1 ? `${list[0]} (+${list.length - 1} more)` : list[0] || '';
  }

  const promptKey = `tool-${id}-prompt`;
  const promptOpen = !!expandedState.get(promptKey);

  const card = el('div', { class: 'tool-call media-call', 'data-tool-use-id': id });

  // Header
  const header = el('button', { class: 'tool-call__header', type: 'button' });
  const iconWrap = el('span', { class: `tool-call__icon tool-call__icon--${meta.color}` });
  iconWrap.appendChild(icon(meta.iconPath, 13));
  header.appendChild(iconWrap);
  header.appendChild(el('span', { class: 'tool-call__name' }, label));
  // Summary: shortened prompt (or source image for animate)
  const summary = sourceImage ? `${sourceImage} — ${promptText}` : promptText;
  if (summary) {
    const trimmed = summary.length > 80 ? summary.slice(0, 77) + '…' : summary;
    header.appendChild(el('span', { class: 'tool-call__summary' }, trimmed));
  }
  const statusEl = el('span', { class: 'tool-call__status' });
  if (isPending) {
    statusEl.appendChild(el('span', { class: 'tool-call__spinner' }));
  } else {
    const checkPath = isError ? 'M18 6L6 18M6 6l12 12' : 'M5 13l4 4L19 7';
    statusEl.appendChild(icon(checkPath, 12));
    statusEl.classList.add(isError ? 'tool-call__status--error' : 'tool-call__status--ok');
  }
  header.appendChild(statusEl);

  // Chevron — matches the regular tool-card affordance. Toggles the prompt
  // panel below. Skipped while pending (no body yet) so the header doesn't
  // imply an empty expandable.
  let chevron = null;
  if (!isPending) {
    chevron = el('span', { class: 'tool-call__chevron' });
    chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
    if (promptOpen) chevron.style.transform = 'rotate(180deg)';
    header.appendChild(chevron);
  }
  card.appendChild(header);

  // Hidden-by-default prompt panel. Replaces the old "Show prompt" button —
  // the chevron in the header is the only toggle now. The generated media
  // gallery renders below this and stays visible regardless of prompt state.
  const promptPre = el('pre', { class: `tool-call__preview media-call__prompt${promptOpen ? '' : ' media-call__prompt--hidden'}` });
  let promptBody = promptText || '(no prompt)';
  if (sourceImage) promptBody = `image: ${sourceImage}\n\n${promptBody}`;
  promptPre.textContent = promptBody;
  card.appendChild(promptPre);

  if (!isPending) {
    header.addEventListener('click', (e) => {
      e.stopPropagation();
      const wasOpen = !promptPre.classList.contains('media-call__prompt--hidden');
      promptPre.classList.toggle('media-call__prompt--hidden', wasOpen);
      if (chevron) chevron.style.transform = wasOpen ? '' : 'rotate(180deg)';
      expandedState.set(promptKey, !wasOpen);
    });
  }

  // Once we have a result, parse out saved paths and render them inline.
  if (result && !isError) {
    const envelope = parseMediaOutput(result.content);
    const paths = envelope.paths;
    const isVideo = name === 'video_create' || name === 'animate';
    // Show the per-call estimated spend on the card so the user can see what
    // a single image / video cost them, on top of the cumulative total in
    // the chat header (which now includes media spend too — see
    // executor.rs tool_cost_sink drain).
    if (envelope.cost_usd != null && envelope.cost_usd > 0) {
      const costPill = el('span', {
        class: 'media-call__cost',
        title: 'Estimated cost for this call — list price per output, may differ from your actual bill.',
      }, `~$${envelope.cost_usd.toFixed(envelope.cost_usd < 0.01 ? 4 : 3)}`);
      // Insert into the header before the status icon so it sits next to the summary.
      const statusEl = header.querySelector('.tool-call__status');
      if (statusEl) header.insertBefore(costPill, statusEl);
      else header.appendChild(costPill);
    }
    if (paths.length > 0) {
      const gallery = el('div', { class: 'media-call__gallery' });
      const taskId = agentStore.getState('activeTaskId');
      const projectRoot = taskId ? getTaskProjectRoot(taskId) : null;
      for (const relPath of paths) {
        const tile = el('div', { class: 'media-call__tile' });
        if (isVideo) {
          const video = el('video', {
            class: 'media-call__video',
            controls: 'true',
            preload: 'metadata',
          });
          if (projectRoot) attachMediaSource(video, projectRoot, relPath, 'video/mp4');
          tile.appendChild(video);
        } else {
          // Click to enlarge — reuse the existing chat lightbox so a
          // generated image behaves the same as a user-pasted one. The
          // image data is already a data:image/...;base64,... URL set by
          // attachMediaSource, so the lightbox can render it directly.
          const img = el('img', {
            class: 'media-call__image',
            alt: promptText.slice(0, 100),
            title: 'Click to enlarge',
          });
          img.style.cursor = 'zoom-in';
          img.addEventListener('click', (ev) => {
            ev.stopPropagation();
            if (img.src) openImageLightbox(img.src);
          });
          if (projectRoot) attachMediaSource(img, projectRoot, relPath, 'image/png');
          tile.appendChild(img);
        }
        const captionRow = el('div', { class: 'media-call__caption' });
        captionRow.appendChild(el('span', { class: 'media-call__path', title: relPath }, relPath));
        // "Reveal in file manager" button — calls the existing
        // reveal_in_file_manager Tauri command with the absolute path.
        // This matters most in Global mode, where outputs live under
        // <app_data>/global_scope/.rustic/... and aren't easy to find in
        // Explorer without a shortcut.
        if (projectRoot) {
          const revealBtn = el('button', {
            class: 'media-call__reveal',
            type: 'button',
            title: 'Reveal in file manager',
          });
          revealBtn.appendChild(icon('M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z', 12));
          revealBtn.addEventListener('click', async (ev) => {
            ev.stopPropagation();
            const sep = projectRoot.includes('\\') && !projectRoot.includes('/') ? '\\' : '/';
            const trimmedRoot = projectRoot.replace(/[\\/]+$/, '');
            const absPath = `${trimmedRoot}${sep}${relPath.replace(/\//g, sep)}`;
            try {
              await api.revealInFileManager(absPath);
            } catch (err) {
              console.warn('reveal_in_file_manager failed:', err);
            }
          });
          captionRow.appendChild(revealBtn);
        }
        tile.appendChild(captionRow);
        gallery.appendChild(tile);
      }
      card.appendChild(gallery);
    }
  } else if (result && isError) {
    // Surface the error message in-card.
    const errBox = el('div', { class: 'media-call__error' });
    errBox.textContent = String(result.content || 'Generation failed.').slice(0, 800);
    card.appendChild(errBox);
  }

  return card;
}

/// Parse the saved file paths out of the ```media-output JSON block produced
/// by the media tools. Returns an empty array if the block is missing or
/// malformed.
function parseMediaOutputPaths(content) {
  return parseMediaOutput(content).paths;
}

/// Parse the full media-output envelope (paths + cost). Returned shape:
/// `{ paths: string[], cost_usd: number | null }`. Empty / malformed
/// content yields zeroed defaults so callers don't need to guard.
function parseMediaOutput(content) {
  const empty = { paths: [], cost_usd: null };
  if (!content) return empty;
  const m = String(content).match(/```media-output\s*\n([\s\S]*?)\n```/);
  if (!m) return empty;
  try {
    const data = JSON.parse(m[1]);
    return {
      paths: Array.isArray(data.paths) ? data.paths.filter((p) => typeof p === 'string') : [],
      cost_usd: typeof data.cost_usd === 'number' ? data.cost_usd : null,
    };
  } catch {
    return empty;
  }
}

/// Resolve a project-relative media path to an absolute path, load the file
/// as base64 via the Tauri preview command, and set it as a data URL on the
/// given <img>/<video> element. Falls back silently if the load fails.
async function attachMediaSource(el, projectRoot, relPath, defaultMime) {
  try {
    const sep = projectRoot.includes('\\') && !projectRoot.includes('/') ? '\\' : '/';
    const trimmedRoot = projectRoot.replace(/[\\/]+$/, '');
    const absPath = `${trimmedRoot}${sep}${relPath.replace(/\//g, sep)}`;
    const resp = await import('../../lib/tauri-api.js').then((m) => m.readFileBase64(absPath));
    const mime = guessMimeFromPath(relPath, defaultMime);
    el.src = `data:${mime};base64,${resp.data}`;
  } catch (err) {
    console.warn('media-call: failed to load', relPath, err);
  }
}

function guessMimeFromPath(p, fallback) {
  const ext = p.split('.').pop()?.toLowerCase() || '';
  switch (ext) {
    case 'png': return 'image/png';
    case 'jpg':
    case 'jpeg': return 'image/jpeg';
    case 'webp': return 'image/webp';
    case 'gif': return 'image/gif';
    case 'mp4': return 'video/mp4';
    case 'webm': return 'video/webm';
    case 'mov': return 'video/quicktime';
    default: return fallback || 'application/octet-stream';
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

  // ── Special rendering for media-generation tools ────────────────
  // image_create / video_create / animate emit a fenced ```media-output
  // JSON block listing the saved paths; render those inline above the
  // standard tool card.
  if (name === 'image_create' || name === 'video_create' || name === 'animate') {
    return renderMediaToolCard(block, result);
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

  // While the tool is still streaming args or executing (no result yet),
  // skip the chevron + dropdown body entirely. The header alone (icon +
  // name + live summary + spinner) is enough to communicate "in flight".
  // The whole card gets rebuilt when the result arrives — the cache
  // version flips from `pending:*` to `done:*`, which triggers a full
  // render where the chevron + body are added below.
  if (isPending) {
    card.appendChild(header);
    return card;
  }

  // Chevron — start in the final rotation so it doesn't animate on re-render
  const chevron = el('span', { class: 'tool-call__chevron' });
  chevron.appendChild(icon('M19 9l-7 7-7-7', 10));
  if (wasOpen) chevron.style.transform = 'rotate(180deg)';
  header.appendChild(chevron);

  card.appendChild(header);

  // ── Expandable body: clickable Input / Output buttons with preview ──
  const body = el('div', { class: `tool-call__body${wasOpen ? '' : ' tool-call__body--hidden'}` });

  const inputText = formatToolInput(name, input);
  logBigString(`tool-call[${name}].input`, inputText);
  if (result?.content) logBigString(`tool-call[${name}].output`, result.content);

  // Input button — click to open in scratch editor
  const inputBtn = el('button', { class: 'tool-call__action-btn' });
  inputBtn.appendChild(el('span', { class: 'tool-call__action-label' }, 'Input'));
  const inputPreview = inputText.split('\n').slice(0, 3).join('\n');
  if (inputPreview.trim()) {
    const inputPre = el('pre', { class: 'tool-call__preview' });
    inputPre.textContent = inputPreview;
    inputBtn.appendChild(inputPre);
  }
  // Edit-shaped tool inputs are now just a path (the diff has moved to the
  // OUTPUT card), so they open as plain text. Other native tools whose input
  // is structured JSON still open with `'json'` syntax for readability.
  const inputLang = DIFF_TOOL_NAMES.has(name) ? 'text' : 'json';
  inputBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    openScratchInEditor(`[Input] ${label}`, inputText, inputLang);
  });
  body.appendChild(inputBtn);

  // Output button — click to open in scratch editor, show 3-line preview.
  // web_search / web_fetch results come back as JSON with encrypted fields;
  // format them before display so the user sees titles + URLs, not blobs.
  //
  // For edit-shaped tools the harness's reply is a one-line summary that
  // hides the actual change (Codex returns "1 file(s) changed: …"; Claude
  // Code returns a small snippet). We render the diff synthesised from the
  // tool input there instead — the INPUT card already shows just the path,
  // so the OUTPUT card carries the full edit.
  const isDiffTool = DIFF_TOOL_NAMES.has(name);
  const haveResult = result && result.content != null;
  if (isDiffTool || haveResult) {
    const content = isDiffTool
      ? formatEditDiffForOutput(name, input, haveResult ? result.content : '')
      : formatToolOutput(name, result.content);
    const outputLang = isDiffTool ? 'diff' : 'text';
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
      openScratchInEditor(`[Output] ${label}`, content, outputLang);
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

/// Render the friendlier error bubble for a failed agent send. Reads the
/// errorMeta classification produced by classifySendError() and shows the
/// appropriate primary action (Retry / Open AI settings) plus a collapsed
/// "show details" expander with the raw provider message.
function renderErrorBubble(meta) {
  const card = el('div', { class: `chat-error-bubble chat-error-bubble--${meta.kind}` });

  const head = el('div', { class: 'chat-error-bubble__head' });
  // Triangle-with-! icon — same shape used by the approval widget for
  // sensitive operations, so error visuals stay consistent.
  head.appendChild(icon('M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z', 16));
  head.appendChild(el('span', { class: 'chat-error-bubble__title' }, meta.title || 'Request failed'));
  card.appendChild(head);

  if (meta.detail) {
    card.appendChild(el('div', { class: 'chat-error-bubble__detail' }, meta.detail));
  }

  // Collapsible raw text — useful when the user wants to copy/paste the
  // exact provider error into a bug report.
  if (meta.raw && meta.raw !== meta.title) {
    const det = el('details', { class: 'chat-error-bubble__raw' });
    det.appendChild(el('summary', {}, 'Show provider error'));
    det.appendChild(el('pre', {}, meta.raw));
    card.appendChild(det);
  }

  const actions = el('div', { class: 'chat-error-bubble__actions' });

  if (meta.action !== 'open_ai_settings') {
    const retryBtn = el('button', { class: 'chat-error-bubble__btn chat-error-bubble__btn--primary' }, 'Retry');
    retryBtn.addEventListener('click', () => {
      if (meta.retry) retrySendMessage(meta.retry);
    });
    actions.appendChild(retryBtn);
  }

  if (meta.action === 'open_ai_settings' || meta.kind === 'auth' || meta.kind === 'provider_missing') {
    const settingsBtn = el('button', { class: 'chat-error-bubble__btn' }, 'Open AI settings');
    settingsBtn.addEventListener('click', () => {
      setSettingsCategory('agent');
      openSettings();
    });
    actions.appendChild(settingsBtn);
  }

  card.appendChild(actions);
  return card;
}

function renderModelSwitchSeparator(toModel, thinkEffort, thinkBudget, providerType) {
  const sep = el('div', { class: 'chat-model-switch' });
  sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
  // Subscription harnesses get a prefix so the user can tell which CLI is
  // driving — e.g. "Claude Code · sonnet" vs the bare "sonnet" that an
  // Anthropic-API task would show. The CLI name matters because the same
  // model id behaves differently under each harness (toolset, system prompt,
  // billing).
  const harnessLabel = providerType === 'ClaudeCode' ? 'Claude Code'
    : providerType === 'Codex' ? 'Codex'
    : '';
  let label = harnessLabel
    ? `${harnessLabel} · Model: ${toModel}`
    : `Model: ${toModel}`;
  if (thinkEffort) label += ` · thinking: ${thinkEffort}`;
  else if (thinkBudget > 0) label += ` · thinking: ${thinkBudget} tokens`;
  sep.appendChild(el('span', { class: 'chat-model-switch__label' }, label));
  sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
  return sep;
}
function formatText(text) {
  return renderMarkdown(text);
}

// attachCodeCopyButtons, openImageLightbox extracted to ./chat-view/ — see imports.
