import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { uiStore } from './ui.js';
import { refreshAffectedDirectory, refreshProject, workspaceStore } from './workspace.js';

// Synthetic project id used when the user starts a chat in "Global" mode —
// no specific project scope. Treated as a normal project id across storage
// and history lookups so we don't need a nullable column. The orchestrator
// behavior (read-only, cross-project tools) is layered on top in Phase 2.
export const GLOBAL_PROJECT_ID = '__global__';

// Storage key for the welcome-screen project picker so the last choice
// persists across app restarts.
const PENDING_PROJECT_STORAGE_KEY = 'rustic_pending_project_id';
const PENDING_MODEL_STORAGE_KEY = 'rustic_pending_model_choice';
const PENDING_PERMISSION_STORAGE_KEY = 'rustic_pending_permission_level';
const PENDING_SENSITIVE_STORAGE_KEY = 'rustic_pending_sensitive_access';
const PENDING_THINKING_STORAGE_KEY = 'rustic_pending_thinking';

function loadPendingProjectId() {
  // Default the welcome-screen scope to Global. This matches user expectation
  // ("home = no specific project") and avoids the previous fallback that
  // silently picked `projects[0]`, which surprised users who didn't notice
  // their first project had been auto-selected.
  try {
    return localStorage.getItem(PENDING_PROJECT_STORAGE_KEY) || GLOBAL_PROJECT_ID;
  } catch {
    return GLOBAL_PROJECT_ID;
  }
}

function loadPendingModelChoice() {
  try {
    const raw = localStorage.getItem(PENDING_MODEL_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (parsed && parsed.providerId && parsed.modelId) return parsed;
    return null;
  } catch {
    return null;
  }
}

function loadPendingPermissionLevel() {
  try {
    return localStorage.getItem(PENDING_PERMISSION_STORAGE_KEY) || null;
  } catch {
    return null;
  }
}

function loadPendingSensitiveAccess() {
  try {
    return localStorage.getItem(PENDING_SENSITIVE_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

function loadPendingThinking() {
  try {
    const raw = localStorage.getItem(PENDING_THINKING_STORAGE_KEY);
    if (!raw) return null;
    const p = JSON.parse(raw);
    if (p && typeof p.enabled === 'boolean') return p;
    return null;
  } catch {
    return null;
  }
}

export const agentStore = createStore({
  tasks: {},            // taskId -> { id, projectId, title, status, messages: [], isStreaming, cost }
  activeTaskId: null,
  // Project chosen for the next new chat from the welcome screen. When an
  // active task exists this is ignored — the task's own project_id wins.
  pendingProjectId: loadPendingProjectId(),
  // Model chosen on the welcome screen for the next new chat.
  // Shape: { providerId: string, modelId: string } | null.
  pendingModelChoice: loadPendingModelChoice(),
  // Permission level + sensitive-access preselected on the welcome screen.
  // Applied to the new task right after createTask.
  pendingPermissionLevel: loadPendingPermissionLevel(),
  pendingSensitiveAccess: loadPendingSensitiveAccess(),
  // Thinking effort preselected on the welcome screen. Shape:
  // { enabled: bool, effort?: 'low'|'medium'|'high'|..., budget?: number } | null
  pendingThinking: loadPendingThinking(),
  permissionRequests: {}, // taskId -> [{ request_id, operation, description, preview, countdown }]
  subagents: {},          // taskId -> { agentId -> { agentId, model, status, output } }
  toolProgress: {},       // tool_use_id -> { progress_text }
  // Buffer of in-progress tool_use input JSON during streaming. The provider
  // emits ToolUseInputDelta fragments which we concatenate here keyed by
  // tool_use_id. On each delta we attempt a tolerant JSON.parse — when it
  // succeeds (rare mid-stream, but free) the parsed object is mirrored
  // onto the message's tool_use block so the user sees the input fill in
  // live. Cleared when ToolUseStop fires (or on the canonical ToolUse
  // event that follows from the executor with the authoritative parse).
  streamingToolInputs: {}, // tool_use_id -> raw partial JSON string
  todos: {},              // taskId -> [{ content, status }]
  pendingQuestions: {},   // taskId -> { request_id, question }
  // Mid-turn steering (plan §14): messages typed while the task is `Running`
  // get queued client-side and auto-flushed when the task flips out of
  // Running. Shape: taskId -> [{ text, images }]. Cleared per-task as the
  // queue is drained. Not persisted across reload — losing in-flight queued
  // input on a crash is acceptable; persistence would surprise the user.
  pendingUserInput: {},
});

export function setPendingProjectId(projectId) {
  agentStore.setState({ pendingProjectId: projectId });
  try {
    if (projectId) localStorage.setItem(PENDING_PROJECT_STORAGE_KEY, projectId);
    else localStorage.removeItem(PENDING_PROJECT_STORAGE_KEY);
  } catch {}
}

/// Model choice persisted until a task is created from the welcome screen.
/// Passing null clears it.
export function setPendingModelChoice(choice) {
  agentStore.setState({ pendingModelChoice: choice });
  try {
    if (choice) localStorage.setItem(PENDING_MODEL_STORAGE_KEY, JSON.stringify(choice));
    else localStorage.removeItem(PENDING_MODEL_STORAGE_KEY);
  } catch {}
}

/// Permission level preselected on the welcome screen. Applied to the new
/// task immediately after `createTask` in the send handler.
export function setPendingPermissionLevel(level) {
  agentStore.setState({ pendingPermissionLevel: level });
  try {
    if (level) localStorage.setItem(PENDING_PERMISSION_STORAGE_KEY, level);
    else localStorage.removeItem(PENDING_PERMISSION_STORAGE_KEY);
  } catch {}
}

/// Sensitive-file access preselected on the welcome screen.
export function setPendingSensitiveAccess(allowed) {
  agentStore.setState({ pendingSensitiveAccess: !!allowed });
  try {
    localStorage.setItem(PENDING_SENSITIVE_STORAGE_KEY, allowed ? '1' : '0');
  } catch {}
}

/// Thinking-effort choice persisted for the welcome screen. Survives
/// restarts so the Global chat reopens with the same effort the user set.
export function setPendingThinking(thinking) {
  agentStore.setState({ pendingThinking: thinking });
  try {
    if (thinking) localStorage.setItem(PENDING_THINKING_STORAGE_KEY, JSON.stringify(thinking));
    else localStorage.removeItem(PENDING_THINKING_STORAGE_KEY);
  } catch {}
}

// Initialize event listeners
let eventsInitialized = false;

export async function initAgentEvents() {
  if (eventsInitialized) return;
  eventsInitialized = true;

  api.onAgentStream((payload) => {
    const { task_id, text } = payload;
    appendStreamText(task_id, text);
  });

  // Global orchestrator created a sub-task in a project. Insert it into the
  // local task store (so the sidebar shows it) and fire the first
  // send_message so it actually starts running. Fire-and-forget — the
  // orchestrator doesn't wait for the sub-task to finish.
  api.onOrchestratorSpawnedTask(async (payload) => {
    const {
      task_id,
      project_id,
      title,
      prompt,
      model,
      provider_type,
      permission_level,
      sensitive_files_allowed,
    } = payload || {};
    if (!task_id || !prompt) return;
    try {
      const tasks = { ...agentStore.getState('tasks') };
      if (!tasks[task_id]) {
        const nowIso = new Date().toISOString();
        tasks[task_id] = {
          id: task_id,
          project_id,
          projectId: project_id,
          title: title || 'Subtask',
          status: 'Completed',
          messages: [],
          isStreaming: false,
          // Carried over from the orchestrator so the chat toolbar's model
          // pill reads the inherited id instead of falling back to "Select
          // model". Both keys are populated because downstream code reads
          // either shape.
          model: model || '',
          provider_type: provider_type || '',
          // Local-time approximation so the welcome-screen history list
          // can sort this task and render a relative "just now" label.
          // The authoritative timestamps come from the DB on next
          // listTasks refresh.
          created_at: nowIso,
          updated_at: nowIso,
          info: {
            id: task_id,
            model: model || '',
            provider_type: provider_type || '',
            created_at: nowIso,
            updated_at: nowIso,
          },
          // Spawned subtasks are hardwired to FullAuto on the backend.
          // Reflect that in the UI so the permission pill matches reality.
          permissionLevel: permission_level || 'FullAuto',
          sensitiveFilesAllowed: !!sensitive_files_allowed,
        };
        agentStore.setState({ tasks });
      }
      await sendMessage(task_id, prompt, undefined, undefined);
    } catch (e) {
      console.error('Failed to dispatch orchestrator-spawned task:', e);
    }
  });

  api.onAgentToolUseStart((payload) => {
    const { task_id, tool_use_id, tool_name } = payload;
    if (typeof window !== 'undefined' && window.__rusticDebugSubs) {
      console.log(`[event] tool-use-start task=${task_id.slice(0,8)} id=${tool_use_id?.slice(0,12) || '?'} name=${tool_name}`);
    }
    // Append a placeholder tool_use with empty input + streaming flag.
    // The card renders immediately with name + spinner; the input will
    // be filled in by the InputDelta events that follow.
    appendToolUse(task_id, tool_use_id, tool_name, {}, /* streaming */ true);
  });

  api.onAgentToolUseInputDelta((payload) => {
    const { task_id, tool_use_id, partial_json } = payload;
    accumulateToolInputDelta(task_id, tool_use_id, partial_json);
  });

  api.onAgentToolUseStop((payload) => {
    const { task_id, tool_use_id } = payload;
    finalizeToolInputStreaming(task_id, tool_use_id);
  });

  api.onAgentToolUse((payload) => {
    const { task_id, tool_use_id, tool_name, tool_input } = payload;
    if (typeof window !== 'undefined' && window.__rusticDebugSubs) {
      console.log(`[event] tool-use task=${task_id.slice(0,8)} id=${tool_use_id?.slice(0,12) || '?'} name=${tool_name}`);
    }
    // Idempotent: if streaming already placed this tool_use into messages
    // (matched by id), update its input with the canonical, fully-parsed
    // value. Otherwise append fresh — covers non-streaming providers and
    // any case where streaming events were dropped/missed.
    appendToolUse(task_id, tool_use_id, tool_name, tool_input, /* streaming */ false);
  });

  api.onAgentToolResult((payload) => {
    const { task_id, tool_use_id, output, is_error } = payload;
    if (typeof window !== 'undefined' && window.__rusticDebugSubs) {
      console.log(`[event] tool-result task=${task_id.slice(0,8)} id=${tool_use_id?.slice(0,12) || '?'} err=${is_error?1:0} len=${(output||'').length}`);
    }
    appendToolResult(task_id, tool_use_id, output, is_error);
    // Clear progress when result arrives
    const progress = { ...agentStore.getState('toolProgress') };
    delete progress[tool_use_id];
    agentStore.setState('toolProgress', progress);
    _maybeRefreshFileTree(task_id, tool_use_id);
  });

  api.onAgentToolProgress((payload) => {
    const { tool_use_id, progress_text } = payload;
    if (typeof window !== 'undefined' && window.__rusticDebugSubs) {
      console.log(`[event] tool-progress id=${tool_use_id?.slice(0,12) || '?'} txt=${(progress_text||'').slice(0,40)}`);
    }
    const progress = { ...agentStore.getState('toolProgress') };
    progress[tool_use_id] = { progress_text };
    agentStore.setState('toolProgress', progress);
  });

  api.onAgentTaskStatus((payload) => {
    const { task_id, status } = payload;
    updateTaskStatus(task_id, status);
  });

  api.onAgentTaskComplete((payload) => {
    const { task_id, summary } = payload;
    appendTaskComplete(task_id, summary);
    _refreshProjectForTask(task_id);
  });

  api.onAgentPermissionRequest((payload) => {
    addPermissionRequest(payload);
  });

  api.onAgentCostUpdate((payload) => {
    const { task_id, cost } = payload;
    updateTaskCost(task_id, cost);
  });

  api.onAgentRequestUsage((payload) => {
    // The Rust-side event struct has `#[serde(rename_all = "camelCase")]`,
    // so the payload key is `taskId`, not `task_id`. Destructuring the
    // snake_case name silently produced `undefined`, which made every
    // downstream lookup (lastRequestUsage[task_id], tasks[task_id]) a no-op
    // and was the reason the per-user-message cost pill never updated.
    const { taskId: task_id, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, costUsd } = payload;
    const lastRequestUsage = { ...(agentStore.getState('lastRequestUsage') || {}) };
    lastRequestUsage[task_id] = {
      input: inputTokens,
      output: outputTokens,
      cacheRead: cacheReadTokens,
      cacheWrite: cacheWriteTokens,
      ts: Date.now(),
    };
    agentStore.setState({ lastRequestUsage });

    // Accumulate this request into the CURRENT user turn's bucket so the UI
    // can show the total cost of answering that specific user message (as
    // opposed to the per-request snapshot above, or the cumulative task total).
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    let landedIdx = -1;
    let landedRole = null;
    let landedBefore = null;
    let landedAfter = null;
    let landedContentPreview = null;
    if (task && task.messages && task.messages.length > 0) {
      const msgs = [...task.messages];
      for (let i = msgs.length - 1; i >= 0; i--) {
        // Only count real user-authored messages. Injected markers (e.g.
        // the model_switch row inserted with role:'user' above) would
        // otherwise absorb request usage and leave the real question's
        // badge frozen at zero.
        const firstBlockType = msgs[i].content?.[0]?.type;
        const isRealUserMsg = msgs[i].role === 'user' && (firstBlockType === 'text' || firstBlockType === 'image');
        if (isRealUserMsg) {
          const prev = msgs[i].turnUsage || { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 };
          const next = {
            input: prev.input + (inputTokens || 0),
            output: prev.output + (outputTokens || 0),
            cacheRead: prev.cacheRead + (cacheReadTokens || 0),
            cacheWrite: prev.cacheWrite + (cacheWriteTokens || 0),
            cost: prev.cost + (costUsd || 0),
          };
          msgs[i] = { ...msgs[i], turnUsage: next };
          task.messages = msgs;
          agentStore.setState({ tasks: { ...tasks } });
          landedIdx = i;
          landedRole = msgs[i].role;
          landedBefore = prev;
          landedAfter = next;
          // Preview the content block so we can tell a real user-authored
          // message from an injected marker (model_switch, etc.).
          const firstBlock = (msgs[i].content && msgs[i].content[0]) || {};
          landedContentPreview = firstBlock.type === 'text'
            ? `text:"${String(firstBlock.text || '').slice(0, 40)}"`
            : `block_type:${firstBlock.type || 'unknown'}`;
          break;
        }
      }
    }

    console.log(
      `[agent:${task_id}] request — in=${inputTokens} out=${outputTokens} cache_read=${cacheReadTokens} cache_write=${cacheWriteTokens} cost=$${(costUsd || 0).toFixed(4)}`
    );
    // [debug badge] Who did the accumulator land on? If landedIdx is -1, the
    // event had no user message to attach to (spilled). If the content preview
    // is a non-text block type, we hit an injected marker (e.g. model_switch)
    // instead of the real user turn.
    console.log(
      `[debug badge] accum landed: idx=${landedIdx} role=${landedRole} preview=${landedContentPreview} before=${JSON.stringify(landedBefore)} after=${JSON.stringify(landedAfter)}`
    );
  });

  api.onAgentTodoUpdated((payload) => {
    const { task_id, todos: items } = payload;
    const todos = { ...agentStore.getState('todos') };
    todos[task_id] = items;
    agentStore.setState({ todos });
  });

  api.onAgentTitleChanged((payload) => {
    const { task_id, title } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      tasks[task_id] = { ...task, title };
      agentStore.setState({ tasks });
    }
  });

  api.onAgentQuestionRequest((payload) => {
    const { task_id, request_id, question, choices } = payload;
    handleQuestionRequest(task_id, request_id, question, choices || []);
  });

  api.onAgentMemoryUpdated(() => {
    showMemoryToast();
  });

  api.onAgentThinkingDelta((payload) => {
    const { task_id, text } = payload;
    appendThinkingDelta(task_id, text);
  });

  api.onAgentThinkingDone((payload) => {
    const { task_id, duration_secs } = payload;
    stampThinkingDuration(task_id, duration_secs);
  });

  api.onAgentContextCondenseStarted((payload) => {
    const { task_id } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      task.messages = [
        ...task.messages,
        { role: 'system', content: [{ type: 'context_condense', status: 'running' }] },
      ];
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  api.onAgentContextCondenseCompleted((payload) => {
    const { task_id, original_messages, condensed_to } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      // Find the running condense marker and update it
      const msgs = [...task.messages];
      let found = false;
      for (let i = msgs.length - 1; i >= 0; i--) {
        const b = msgs[i].content?.[0];
        if (b?.type === 'context_condense' && b.status === 'running') {
          msgs[i] = {
            ...msgs[i],
            content: [{ type: 'context_condense', status: 'completed', original_messages, condensed_to }],
          };
          found = true;
          break;
        }
      }
      if (!found) {
        msgs.push({
          role: 'system',
          content: [{ type: 'context_condense', status: 'completed', original_messages, condensed_to }],
        });
      }
      task.messages = msgs;
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  api.onAgentModelSwitched((payload) => {
    const { task_id, from_model, to_model, provider_type } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      task.model = to_model;
      task.provider_type = provider_type;
      // Append the ModelSwitch marker to the local message list so the chat view re-renders
      task.messages = [
        ...task.messages,
        {
          role: 'user',
          content: [{ type: 'model_switch', from_model, to_model, provider_type }],
        },
      ];
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  initSubagentEvents();
  initInputQueueEvents();
}

/// Multi-client queue events (plan §B.9). Today's single-window Tauri build
/// gets these events back from its own `notify_input_*` calls — the local
/// state already reflects the change so the listeners no-op. The wiring
/// exists so a future multi-window or remote-viewer build can drop in
/// state synchronisation here without touching the producers.
async function initInputQueueEvents() {
  api.onAgentInputQueued((payload) => {
    void payload; // forward-compat: secondary viewer would mirror here
  });
  api.onAgentInputDelivered((payload) => {
    void payload; // forward-compat: secondary viewer would clear here
  });
}

async function getListenDirect() {
  try {
    const mod = await import('@tauri-apps/api/event');
    return mod.listen;
  } catch {
    return async () => () => {};
  }
}

async function initSubagentEvents() {
  api.onAgentSubagentSpawned((payload) => {
    const { task_id, agent_id, model, prompt } = payload;
    console.log('[subagent] spawned:', agent_id, 'model:', model, 'task:', task_id);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    // Merge with any partial entry the cost-update handler may have created
    // if a SubagentCostUpdate event raced ahead of SubagentSpawned. We
    // overwrite the static fields (agentId/model/status/prompt) but
    // preserve any `cost` and `output` that landed first.
    const existing = taskAgents[agent_id] || {};
    taskAgents[agent_id] = {
      ...existing,
      agentId: agent_id,
      model,
      status: 'running',
      output: existing.output || '',
      prompt: prompt || existing.prompt || '',
    };
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentCompleted((payload) => {
    const { task_id, agent_id, summary } = payload;
    console.log('[subagent] completed:', agent_id, 'summary_len:', summary?.length);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      // Keep summary as its own field so the card can surface it as a final
      // report without the user having to dig past the streamed activity log.
      // `output` still gets the summary appended (with a clear marker) so
      // the existing "View output" scratch buffer remains self-contained.
      const sep = '\n\n━━━ FINAL REPORT ━━━\n\n';
      const newOutput = taskAgents[agent_id].output + (summary ? sep + summary : '');
      taskAgents[agent_id] = {
        ...taskAgents[agent_id],
        status: 'completed',
        summary: summary || '',
        output: newOutput,
      };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentFailed((payload) => {
    const { task_id, agent_id, error } = payload;
    console.log('[subagent] failed:', agent_id, 'error:', error);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], status: 'failed', output: taskAgents[agent_id].output + '\n\nError: ' + error };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentTextDelta((payload) => {
    const { task_id, agent_id, text } = payload;
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      const existing = taskAgents[agent_id];
      // Track an ordered event stream so the sub-agent view can render text
      // and tool_use blocks in the original interleaved order. The plain
      // `output` string still carries the full transcript for word counts
      // and the scratch-buffer scratch view.
      const events = [...(existing.events || [])];
      const last = events.length > 0 ? events[events.length - 1] : null;
      if (last && last.kind === 'text') {
        events[events.length - 1] = { ...last, text: last.text + text };
      } else {
        events.push({ kind: 'text', text });
      }
      taskAgents[agent_id] = { ...existing, output: existing.output + text, events };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentCostUpdate((payload) => {
    const { task_id, agent_id, cost } = payload;
    if (typeof window !== 'undefined' && window.__rusticDebugSubs) {
      console.log(
        `[event] subagent-cost-update task=${task_id?.slice(0, 8)} ` +
        `agent=${agent_id} in=${cost?.total_input_tokens || 0} ` +
        `out=${cost?.total_output_tokens || 0} ` +
        `cache_r=${cost?.total_cache_read_tokens || 0} ` +
        `cache_w=${cost?.total_cache_write_tokens || 0} ` +
        `usd=${(cost?.estimated_cost_usd || 0).toFixed(4)}`
      );
    }
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    // **Don't silently drop the update if the agent isn't in the store yet.**
    // SubagentSpawned and SubagentCostUpdate are emitted on the same channel
    // and should arrive in order, but a race can land an early cost update
    // before the spawn entry is fully wired (the executor's first API call
    // completes before the FE's spawn handler has run). If we drop the cost
    // here, the card stays at "0 / 0 / $0" until the *next* cost update —
    // and if that one's also early, you can lose the whole run.
    //
    // Insert a partial entry instead: just enough fields for the card to
    // render. The full state lands when SubagentSpawned arrives later (the
    // spawn handler's `taskAgents[agent_id] = { ... }` overwrites this entry
    // — see below). Either way, no cost update is ever dropped.
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], cost };
    } else {
      taskAgents[agent_id] = {
        agentId: agent_id,
        model: '',
        status: 'running',
        output: '',
        prompt: '',
        cost,
      };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  const listenDirect = await getListenDirect();

  listenDirect('agent-subagent-tool-use', (event) => {
    const { task_id, agent_id, tool_name, tool_use_id, input } = event.payload || {};
    if (!task_id || !agent_id || !tool_use_id) return;
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (!taskAgents[agent_id]) {
      taskAgents[agent_id] = {
        agentId: agent_id,
        model: '',
        status: 'running',
        output: '',
        prompt: '',
        toolCalls: [],
        events: [],
      };
    }
    const existing = taskAgents[agent_id];
    const calls = [...(existing.toolCalls || [])];
    calls.push({ tool_use_id, tool_name, input: input || {}, result: null, is_error: false });
    const events = [...(existing.events || [])];
    events.push({ kind: 'tool_use', tool_use_id, tool_name, input: input || {} });
    taskAgents[agent_id] = { ...existing, toolCalls: calls, events };
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  listenDirect('agent-subagent-tool-result', (event) => {
    const { task_id, agent_id, tool_use_id, content, is_error } = event.payload || {};
    if (!task_id || !agent_id || !tool_use_id) return;
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (!taskAgents[agent_id]) return;
    const existing = taskAgents[agent_id];
    const calls = (existing.toolCalls || []).map((c) =>
      c.tool_use_id === tool_use_id ? { ...c, result: content ?? null, is_error: !!is_error } : c
    );
    taskAgents[agent_id] = { ...existing, toolCalls: calls };
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });
}

function showMemoryToast() {
  const existing = document.getElementById('memory-toast');
  if (existing) {
    clearTimeout(existing._timeout);
    existing.remove();
  }
  const toast = document.createElement('div');
  toast.id = 'memory-toast';
  toast.className = 'memory-toast';
  toast.textContent = 'Memory updated';
  document.body.appendChild(toast);
  // Trigger animation
  requestAnimationFrame(() => toast.classList.add('memory-toast--visible'));
  toast._timeout = setTimeout(() => {
    toast.classList.remove('memory-toast--visible');
    setTimeout(() => toast.remove(), 300);
  }, 2500);
}

export async function createTask(projectId, projectName, projectRoot, title) {
  try {
    const info = await api.createTask(projectId, projectName, projectRoot, title);
    if (!info) return null;

    // Load project defaults (permission level is applied by the backend;
    // thinking effort needs to be applied on the frontend via projectDefaults)
    let projectDefaults = null;
    try {
      projectDefaults = await api.getProjectDefaults(projectId);
    } catch {}

    const tasks = { ...agentStore.getState('tasks') };
    tasks[info.id] = {
      ...info,
      messages: [],
      isStreaming: false,
      // Apply persisted permission level from project defaults
      permissionLevel: projectDefaults?.permission_level || undefined,
      // Attach project defaults so the chat-view can read thinking_effort
      projectDefaults: projectDefaults || null,
    };
    agentStore.setState({ tasks, activeTaskId: info.id });

    // Show secondary sidebar
    uiStore.setState({ secondarySidebarVisible: true });

    return info;
  } catch (e) {
    console.error('Failed to create task:', e);
    return null;
  }
}

/**
 * Queue a user message for delivery after the current turn ends. Used by the
 * chat input when the task is `Running` — see plan §14.
 *
 * The message is held in `pendingUserInput[taskId]` and drained by
 * `drainPendingUserInput` when `updateTaskStatus` sees the task transition
 * out of Running. Each queued entry fires as its own discrete turn — the
 * queue is FIFO, one-per-turn, never concatenated.
 */
export function queueMessage(taskId, text, images) {
  const trimmed = (text || '').trim();
  if (!trimmed) return;
  const all = { ...(agentStore.getState('pendingUserInput') || {}) };
  const list = all[taskId] ? [...all[taskId]] : [];
  const imgs = images || [];
  list.push({ text: trimmed, images: imgs });
  all[taskId] = list;
  agentStore.setState({ pendingUserInput: all });

  // Multi-client queue event (plan §B.9 — forward-compat). Round-trip
  // through the backend so any future second viewer of this task can
  // mirror the queue. `preview` ships a truncated copy only; full text
  // stays in this window.
  const preview = trimmed.length > 240 ? trimmed.slice(0, 240) + '…' : trimmed;
  api
    .notifyInputQueued(taskId, preview, imgs.length, list.length)
    .catch(() => {});
}

/**
 * Drop every queued message for `taskId` without sending. The chat-view
 * exposes this as a per-bubble dismiss so the user can take back a queued
 * entry before the current turn finishes.
 */
export function clearQueuedMessage(taskId, index) {
  const all = { ...(agentStore.getState('pendingUserInput') || {}) };
  if (!all[taskId]) return;
  if (index == null) {
    delete all[taskId];
  } else {
    const list = all[taskId].filter((_, i) => i !== index);
    if (list.length === 0) delete all[taskId];
    else all[taskId] = list;
  }
  agentStore.setState({ pendingUserInput: all });
}

export async function sendMessage(taskId, message, thinkingBudget, images) {
  const tasks = { ...agentStore.getState('tasks') };
  const oldTask = tasks[taskId];
  if (!oldTask) return;

  // Create a new task object to ensure the store detects the change
  const task = { ...oldTask };
  tasks[taskId] = task;

  // Auto-title from first user message. Store a generous prefix (not a
  // tight 60-char cap) so every rendering surface can let CSS ellipsify at
  // its own available width — .chat-empty__history-title,
  // .agent-task__title, and .history-modal__item-title all have
  // overflow:hidden/text-overflow:ellipsis. Pre-truncating to 60 chopped
  // the string mid-word before CSS had anything to work with, so wide
  // panels still saw the same short title with no ellipsis.
  const hasUserMessage = task.messages.some(m => m.role === 'user' && m.content?.some(c => c.type === 'text'));
  if (!hasUserMessage) {
    // Strip `<pasted-text id="N">…</pasted-text>` wrappers before deriving
    // the title — otherwise a paste-only first message names the task
    // "<pasted-text id=…" instead of using the actual content. The body
    // inside the tags is still considered, but the typed text (which comes
    // first in finalParts) takes precedence after collapse.
    const titleSource = message.replace(/<pasted-text id="\d+">\n?([\s\S]*?)\n?<\/pasted-text>/g, '$1');
    const autoTitle = titleSource.replace(/\s+/g, ' ').trim().slice(0, 200);
    if (autoTitle) {
      task.title = autoTitle;
      if (task.info) task.info = { ...task.info, title: autoTitle };
      api.renameTask(taskId, autoTitle).catch(() => {});
    }
  }

  // Build local user message content (text + images for display)
  const userContent = [{ type: 'text', text: message }];
  if (images?.length) {
    for (const img of images) {
      userContent.push({ type: 'image', media_type: img.media_type, data: img.data });
    }
  }

  // Add user message locally with an empty per-turn usage bucket. The
  // RequestUsage handler accumulates provider-call totals into this bucket
  // until the next user message opens a new turn.
  task.messages = [
    ...task.messages,
    {
      role: 'user',
      content: userContent,
      turnUsage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 },
    },
  ];
  task.isStreaming = true;
  task.status = 'Running';
  // Add placeholder for assistant response
  task.messages = [...task.messages, { role: 'assistant', content: [{ type: 'text', text: '' }] }];
  agentStore.setState({ tasks });

  try {
    await api.sendMessage(taskId, message, thinkingBudget, images);
  } catch (e) {
    console.error('Failed to send message:', e);
    task.isStreaming = false;
    task.status = 'Failed';

    const errText = typeof e === 'string' ? e : (e?.message || String(e));
    const meta = classifySendError(errText);
    // The block carries its own errorMeta — the chat-view renderer detects
    // that and swaps the plain text for a richer card with action buttons.
    task.messages = [
      ...task.messages,
      {
        role: 'assistant',
        content: [{
          type: 'text',
          text: meta.title,
          errorMeta: {
            ...meta,
            retry: {
              taskId,
              message,
              thinkingBudget,
              images,
            },
          },
        }],
      },
    ];

    agentStore.setState({ tasks: { ...tasks } });
  }
}

/// Classify a send-message error string into a structured shape so the chat
/// view can render an actionable bubble (Retry, Open settings) instead of a
/// raw stringified exception.
function classifySendError(errText) {
  const s = (errText || '').toLowerCase();

  // Auth: invalid / missing / revoked API key.
  if (
    s.includes('401') || s.includes('unauthorized') ||
    s.includes('invalid api key') || s.includes('incorrect api key') ||
    s.includes('invalid_api_key') || s.includes('authentication') ||
    s.includes('api key not found') || s.includes('no api key')
  ) {
    return {
      kind: 'auth',
      title: 'Authentication failed',
      detail: 'The provider rejected your API key. Open AI settings to re-enter or rotate it.',
      raw: errText,
      action: 'open_ai_settings',
    };
  }
  // Rate limit / quota.
  if (s.includes('429') || s.includes('rate limit') || s.includes('rate_limit') || s.includes('quota')) {
    return {
      kind: 'rate_limit',
      title: 'Rate limit hit',
      detail: 'The provider is throttling requests. Wait a moment, or switch model / provider, then retry.',
      raw: errText,
      action: 'retry',
    };
  }
  // Network / connectivity.
  if (
    s.includes('econnrefused') || s.includes('econnreset') || s.includes('etimedout') ||
    s.includes('enotfound') || s.includes('fetch failed') || s.includes('network') ||
    s.includes('timeout') || s.includes('timed out') || s.includes('dns') || s.includes('tls')
  ) {
    return {
      kind: 'network',
      title: 'Network error',
      detail: 'Could not reach the provider. Check your connection or proxy, then retry.',
      raw: errText,
      action: 'retry',
    };
  }
  // Provider config missing / removed.
  if (
    s.includes('provider not found') || s.includes('no provider') ||
    s.includes('provider has been removed') || s.includes('provider not configured')
  ) {
    return {
      kind: 'provider_missing',
      title: 'Provider not configured',
      detail: 'This task\'s provider is missing or its key was cleared. Open AI settings to set one up.',
      raw: errText,
      action: 'open_ai_settings',
    };
  }
  // Context / token-budget overflow.
  if (s.includes('context length') || s.includes('context_length') || s.includes('too many tokens') || s.includes('context window')) {
    return {
      kind: 'context_overflow',
      title: 'Context window full',
      detail: 'The conversation is too long for this model. Start a new chat or switch to a model with a larger context window.',
      raw: errText,
      action: 'retry',
    };
  }
  // Fallback.
  return {
    kind: 'generic',
    title: 'Request failed',
    detail: errText || 'An unknown error occurred.',
    raw: errText,
    action: 'retry',
  };
}

/// Re-send a previously-failed message. Used by the Retry button on the
/// in-chat error bubble. Does not retry permission/question replies — only
/// regular sendMessage calls.
export async function retrySendMessage(retry) {
  if (!retry?.taskId) return;
  await sendMessage(retry.taskId, retry.message || '', retry.thinkingBudget, retry.images);
}

/**
 * Return the index of the assistant message that should receive the next
 * streamed text/thinking/tool_use delta. If the most recent message is a
 * tool result, that means the previous assistant turn already ended and
 * this delta belongs to a NEW turn — so we push a fresh assistant message.
 *
 * Keeping each turn in its own assistant row matches how history is loaded
 * from the DB, and stops the parallel-group heuristic (which groups
 * tool-uses sharing a msgIdx) from treating sequential sibling tool calls
 * across turns as a single parallel group.
 *
 * Mutates `msgs` in place and returns the target index, or -1 if no
 * assistant anchor exists at all (shouldn't happen — sendMessage seeds one).
 */
function getOrOpenAssistantTurn(msgs) {
  if (msgs.length === 0) return -1;
  const last = msgs[msgs.length - 1];
  if (last.role === 'assistant') return msgs.length - 1;
  // Last message is a tool result, user, system, etc. — open a fresh turn.
  // Any role other than 'assistant' here signals the previous assistant
  // turn already closed (tool results always arrive between turns).
  msgs.push({ role: 'assistant', content: [] });
  return msgs.length - 1;
}

function appendStreamText(taskId, text) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  const idx = getOrOpenAssistantTurn(msgs);
  if (idx < 0) return;
  const content = [...msgs[idx].content];
  const lastBlock = content[content.length - 1];
  if (lastBlock && lastBlock.type === 'text') {
    content[content.length - 1] = { ...lastBlock, text: lastBlock.text + text };
  } else {
    content.push({ type: 'text', text });
  }
  msgs[idx] = { ...msgs[idx], content };

  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function appendThinkingDelta(taskId, text) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  const idx = getOrOpenAssistantTurn(msgs);
  if (idx < 0) return;
  const content = [...msgs[idx].content];
  const lastBlock = content[content.length - 1];
  if (lastBlock && lastBlock.type === 'thinking') {
    content[content.length - 1] = { ...lastBlock, thinking: lastBlock.thinking + text };
  } else {
    content.push({ type: 'thinking', thinking: text });
  }
  msgs[idx] = { ...msgs[idx], content };

  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function stampThinkingDuration(taskId, durationSecs) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  // Find the last thinking block without a duration in the last assistant message
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      const content = [...msgs[i].content];
      for (let j = content.length - 1; j >= 0; j--) {
        if (content[j].type === 'thinking' && !content[j].duration_secs) {
          content[j] = { ...content[j], duration_secs: durationSecs };
          msgs[i] = { ...msgs[i], content };
          task.messages = msgs;
          agentStore.setState({ tasks: { ...tasks } });
          return;
        }
      }
      break;
    }
  }
}

function appendToolUse(taskId, toolUseId, toolName, toolInput, isStreaming) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  const idx = getOrOpenAssistantTurn(msgs);
  if (idx < 0) return;

  // Idempotent: if a tool_use block with this id already exists in the
  // current assistant turn (placed there earlier by ToolUseStart streaming),
  // update it in place rather than appending a duplicate. This is the
  // bridge between the streaming path and the canonical post-stream emit.
  const existing = msgs[idx].content;
  const existingPos = existing.findIndex(b => b.type === 'tool_use' && b.id === toolUseId);
  if (existingPos >= 0) {
    const updated = [...existing];
    updated[existingPos] = {
      ...updated[existingPos],
      // Don't downgrade an already-populated name with an empty string —
      // streaming providers always supply name on Start, but defend against
      // an out-of-order canonical emit just in case.
      name: toolName || updated[existingPos].name,
      // Always trust the canonical input on the non-streaming path; on the
      // streaming path we accept the latest tolerant parse.
      input: toolInput || updated[existingPos].input || {},
      _streaming: !!isStreaming,
    };
    msgs[idx] = { ...msgs[idx], content: updated };
  } else {
    msgs[idx] = {
      ...msgs[idx],
      content: [
        ...existing,
        { type: 'tool_use', id: toolUseId, name: toolName, input: toolInput || {}, _streaming: !!isStreaming },
      ],
    };
  }
  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function accumulateToolInputDelta(taskId, toolUseId, partialJson) {
  if (!partialJson) return;
  const buffers = { ...agentStore.getState('streamingToolInputs') };
  const next = (buffers[toolUseId] || '') + partialJson;
  buffers[toolUseId] = next;
  agentStore.setState({ streamingToolInputs: buffers });

  // Try a tolerant parse and mirror the result onto the message's tool_use
  // block so the input section fills in as args arrive. JSON.parse fails
  // mid-stream (most fragments are partial), and that's fine — we just
  // wait for the next delta. No best-effort partial-JSON parsing here:
  // worst case the user sees the full input arrive in one update at the
  // first chunk that happens to close the JSON, which still feels live.
  let parsed = null;
  try { parsed = JSON.parse(next); } catch { return; }
  if (parsed == null || typeof parsed !== 'object') return;

  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;
  const msgs = [...task.messages];
  // Walk back to find the most-recent assistant message that owns this id.
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i];
    if (m.role !== 'assistant' || !Array.isArray(m.content)) continue;
    const pos = m.content.findIndex(b => b.type === 'tool_use' && b.id === toolUseId);
    if (pos < 0) continue;
    const updated = [...m.content];
    updated[pos] = { ...updated[pos], input: parsed };
    msgs[i] = { ...m, content: updated };
    task.messages = msgs;
    agentStore.setState({ tasks: { ...tasks } });
    return;
  }
}

function finalizeToolInputStreaming(taskId, toolUseId) {
  const buffers = { ...agentStore.getState('streamingToolInputs') };
  if (toolUseId in buffers) {
    delete buffers[toolUseId];
    agentStore.setState({ streamingToolInputs: buffers });
  }
  // Clear the _streaming flag on the message block. The canonical
  // `agent-tool-use` event (from the executor) will overwrite `input`
  // with the authoritative parse shortly after.
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;
  const msgs = [...task.messages];
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i];
    if (m.role !== 'assistant' || !Array.isArray(m.content)) continue;
    const pos = m.content.findIndex(b => b.type === 'tool_use' && b.id === toolUseId);
    if (pos < 0) continue;
    if (!m.content[pos]._streaming) return; // already finalized
    const updated = [...m.content];
    updated[pos] = { ...updated[pos], _streaming: false };
    msgs[i] = { ...m, content: updated };
    task.messages = msgs;
    agentStore.setState({ tasks: { ...tasks } });
    return;
  }
}

function appendToolResult(taskId, toolUseId, output, isError) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  task.messages = [
    ...task.messages,
    {
      role: 'tool',
      content: [{ type: 'tool_result', tool_use_id: toolUseId, content: output, is_error: isError }],
    },
  ];
  agentStore.setState({ tasks: { ...tasks } });
}

function updateTaskStatus(taskId, status) {
  const existingTasks = agentStore.getState('tasks');
  const existingTask = existingTasks[taskId];
  if (!existingTask) return;

  // **Bail early if status didn't actually change.** The backend's
  // cancellation/completion paths emit the terminal status from multiple
  // points (executor, mod.rs outer task, harness runtime end) — duplicates
  // are common. Each duplicate previously created a fresh `tasks` object,
  // fired the tasks subscriber, and triggered a full re-render even though
  // nothing visibly changed. With the keyed cache hitting 100% the
  // re-render was a no-op visually but the `replaceChildren` call still
  // moved every node into a fragment and back, which paints as flicker.
  if (existingTask.status === status &&
      existingTask.isStreaming === (status === 'Running')) {
    return;
  }

  const tasks = { ...existingTasks };
  const task = { ...existingTask };
  tasks[taskId] = task;

  // [debug badge] Snapshot every user-row turnUsage on each status flip so we
  // can see whether a transition (Running → Completed) coincides with a value
  // reset. Prints [msgIdx, turnUsage] for every user-role row.
  const snapshot = (task.messages || [])
    .map((m, i) => m.role === 'user' ? [i, m.turnUsage || null] : null)
    .filter(Boolean);
  console.log(
    `[debug badge] status: ${task.status || '(none)'} → ${status}  user_rows=${JSON.stringify(snapshot)}`
  );

  const wasRunning = task.status === 'Running';
  task.status = status;
  task.isStreaming = status === 'Running';

  // When the backend resumes (Running again), clear any pending question
  if (status === 'Running') {
    task.pendingQuestion = null;
    const questions = { ...agentStore.getState('pendingQuestions') };
    delete questions[taskId];
    agentStore.setState({ tasks: { ...tasks }, pendingQuestions: questions });
    return;
  }

  agentStore.setState({ tasks: { ...tasks } });

  // Mid-turn steering: when a task transitions out of Running, flush any
  // queued user input as the next turn. Multiple queued entries get
  // concatenated with double newlines (matches plan §14's "queue is the CLI"
  // semantic for harness providers and gives a sensible compound prompt for
  // native ones too). Only drained on the actual Running → not-Running
  // transition so we don't loop on already-Completed tasks.
  if (wasRunning && status !== 'Running') {
    drainPendingUserInput(taskId);
  }
}

function drainPendingUserInput(taskId) {
  const all = agentStore.getState('pendingUserInput') || {};
  const queue = all[taskId];
  if (!queue || queue.length === 0) return;

  // Pop ONE entry at a time and fire it as its own turn. The remaining
  // entries stay queued and drain on the next Running → not-Running
  // transition (sendMessage flips the task back to Running, so the cycle
  // self-perpetuates until the queue empties). This matches Claude Code's
  // interrupt-based model where each user message is a discrete turn,
  // never concatenated with siblings.
  const [head, ...rest] = queue;
  const next = { ...all };
  if (rest.length > 0) next[taskId] = rest;
  else delete next[taskId];
  agentStore.setState({ pendingUserInput: next });

  // Multi-client delivered event (plan §B.9). Count is 1 per drain pass.
  api.notifyInputDelivered(taskId, 1).catch(() => {});

  // Defer one tick so the UI shows the just-completed turn before the new
  // one starts streaming — feels less "warp-speed" than an immediate flip.
  setTimeout(() => {
    sendMessage(taskId, head.text, undefined, head.images).catch((e) => {
      console.error('Failed to flush queued message:', e);
    });
  }, 30);
}

function handleQuestionRequest(taskId, requestId, question, choices) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const questions = { ...agentStore.getState('pendingQuestions') };
  questions[taskId] = { request_id: requestId, question, choices: choices || [] };

  tasks[taskId] = {
    ...task,
    status: 'WaitingForInput',
    isStreaming: false,
    pendingQuestion: { request_id: requestId, question, choices: choices || [] },
  };
  agentStore.setState({ tasks, pendingQuestions: questions });
}

export async function respondToAgentQuestion(taskId, requestId, answer) {
  // Optimistically set task back to running
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    tasks[taskId] = {
      ...task,
      status: 'Running',
      isStreaming: true,
      pendingQuestion: null,
    };
    const questions = { ...agentStore.getState('pendingQuestions') };
    delete questions[taskId];
    agentStore.setState({ tasks, pendingQuestions: questions });
  }

  try {
    await api.respondToQuestion(taskId, requestId, answer);
  } catch (e) {
    console.error('Failed to respond to question:', e);
  }
}

function appendTaskComplete(taskId, summary) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  // Always stop streaming — this is the primary purpose of the call.
  task.isStreaming = false;
  task.status = 'Completed';

  // The outer-task and inner event-processor can both fire
  // agent-task-complete for the SAME turn. If two arrive close together and
  // the first lacked a summary, upgrade in place. But the dedup must be
  // scoped to the *current turn only* — across multi-turn conversations,
  // each completion needs its own card at its own position. Previously we
  // used `findIndex` (first occurrence) which made every later turn's
  // completion clobber the first turn's card in place at the first turn's
  // position, so the latest summary always appeared back near the top of
  // the chat instead of below the most recent assistant turn.
  //
  // Walk backwards: find the LAST task_complete. It's the dedup candidate
  // only if no user message has been added since it landed (which would
  // signal the user has continued the conversation past that completion,
  // making the next completion belong to a new turn).
  let dedupIdx = -1;
  let sawUserAfterTaskComplete = false;
  for (let i = task.messages.length - 1; i >= 0; i--) {
    const m = task.messages[i];
    if (m.role === 'task_complete') {
      if (!sawUserAfterTaskComplete) dedupIdx = i;
      break;
    }
    if (m.role === 'user') {
      sawUserAfterTaskComplete = true;
    }
  }

  if (dedupIdx === -1) {
    task.messages = [
      ...task.messages,
      {
        role: 'task_complete',
        content: [{ type: 'task_complete', summary: summary || null }],
      },
    ];
  } else if (summary) {
    const existing = task.messages[dedupIdx];
    const block = existing.content?.[0] || {};
    if (!block.summary) {
      const upgraded = {
        ...existing,
        content: [{ ...block, type: 'task_complete', summary }],
      };
      task.messages = [
        ...task.messages.slice(0, dedupIdx),
        upgraded,
        ...task.messages.slice(dedupIdx + 1),
      ];
    }
  }

  agentStore.setState({ tasks: { ...tasks } });
}

function updateTaskCost(taskId, cost) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;
  task.cost = cost;
  agentStore.setState({ tasks: { ...tasks } });
}

function addPermissionRequest(payload) {
  const { task_id, request_id, operation, description, preview } = payload;
  const requests = { ...agentStore.getState('permissionRequests') };
  const taskRequests = [...(requests[task_id] || [])];
  taskRequests.push({ request_id, operation, description, preview });
  requests[task_id] = taskRequests;
  agentStore.setState({ permissionRequests: requests });
}

export function removePermissionRequest(taskId, requestId) {
  const requests = { ...agentStore.getState('permissionRequests') };
  requests[taskId] = (requests[taskId] || []).filter((r) => r.request_id !== requestId);
  agentStore.setState({ permissionRequests: requests });
}

/**
 * @param {string} taskId
 * @param {string} requestId
 * @param {boolean | 'accept' | 'acceptForSession' | 'deny'} decision
 */
export async function respondToPermission(taskId, requestId, decision) {
  removePermissionRequest(taskId, requestId);
  try {
    await api.respondToPermission(taskId, requestId, decision);
  } catch (e) {
    console.error('Failed to respond to permission:', e);
  }
}

export function setActiveTask(taskId) {
  agentStore.setState({ activeTaskId: taskId });
  uiStore.setState({ secondarySidebarVisible: true });
  // Load history from DB if messages are empty (e.g. clicking a past task)
  loadTaskHistory(taskId);
}

/**
 * Fetch persisted messages and cost from the backend and hydrate the task.
 * Only fetches if the task currently has no messages loaded.
 */
export async function loadTaskHistory(taskId) {
  if (!taskId) return;
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return;
  if (task.messages && task.messages.length > 0) return; // already loaded

  try {
    // Load messages, cost, and sub-agent records in parallel. Sub-agent
    // records let the sub-agent cards in the chat show their prompt,
    // final answer, tokens and cost after a reload — the spawn_subagent
    // tool_result alone carries only a brief "spawned" acknowledgement.
    const [messages, cost, subagentRecords] = await Promise.all([
      api.getTaskMessages(taskId).catch(() => []),
      api.getTaskCost(taskId).catch(() => null),
      api.getSubagentRecords(taskId).catch(() => []),
    ]);
    // Map snake_case turn_usage from the backend to camelCase turnUsage so
    // the chat renderer can display per-message stats on history loads.
    const hydratedMessages = (messages || []).map(msg => {
      if (msg.turn_usage) {
        const tu = msg.turn_usage;
        return {
          ...msg,
          turnUsage: {
            input: tu.input || 0,
            output: tu.output || 0,
            cacheRead: tu.cache_read || 0,
            cacheWrite: tu.cache_write || 0,
            cost: tu.cost || 0,
          },
        };
      }
      return msg;
    });

    const updated = { ...agentStore.getState('tasks') };
    if (updated[taskId]) {
      const patch = { ...updated[taskId] };
      if (hydratedMessages.length > 0) patch.messages = hydratedMessages;
      if (cost) patch.cost = cost;
      updated[taskId] = patch;
      agentStore.setState({ tasks: updated });
    }
    if (Array.isArray(subagentRecords) && subagentRecords.length > 0) {
      const subagents = { ...(agentStore.getState('subagents') || {}) };
      const existing = { ...(subagents[taskId] || {}) };
      for (const rec of subagentRecords) {
        const agentId = rec.agent_id;
        // Prefer the live in-memory entry (if the task is actually running);
        // otherwise hydrate from the DB record. `output` isn't persisted, so
        // we leave the streamed activity log field empty on reload — the
        // "Final answer" button uses `summary` which is persisted.
        const live = existing[agentId];
        // Restore the streamed transcript and tool-call list that were
        // persisted incrementally by the Tauri event handler. Before this
        // those fields lived only in memory, so reopening a task showed an
        // empty sub-agent panel even when the run had finished.
        let restoredToolCalls = [];
        if (rec.tool_calls_json) {
          try {
            const parsed = JSON.parse(rec.tool_calls_json);
            if (Array.isArray(parsed)) restoredToolCalls = parsed;
          } catch {}
        }
        existing[agentId] = live && live.status === 'running' ? live : {
          agentId,
          model: rec.model || '',
          status: rec.status || 'completed',
          output: live?.output || rec.output_text || '',
          prompt: rec.prompt || '',
          summary: rec.summary || '',
          toolCalls: live?.toolCalls?.length ? live.toolCalls : restoredToolCalls,
          cost: {
            total_input_tokens: rec.input_tokens || 0,
            total_output_tokens: rec.output_tokens || 0,
            total_cache_read_tokens: rec.cache_read_tokens || 0,
            estimated_cost_usd: rec.cost_usd || 0,
          },
          error: rec.error || '',
        };
      }
      subagents[taskId] = existing;
      agentStore.setState({ subagents });
    }
  } catch (e) {
    console.error('Failed to load task history:', e);
  }
}

/**
 * Change permission mode for a task.
 * Returns true if the change was applied, false on error.
 */
export async function setTaskPermissions(taskId, level) {
  try {
    await api.setTaskPermissions(taskId, level);
    // When leaving FullAuto, reset sensitive access to off
    if (level !== 'FullAuto') {
      await api.setTaskSensitiveAccess(taskId, false);
    }
  } catch (e) {
    console.error('Failed to set permissions:', e);
    return false;
  }

  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    task.permissionLevel = level;
    if (level !== 'FullAuto') task.sensitiveAccess = false;
    agentStore.setState({ tasks: { ...tasks } });
  }
  return true;
}

/**
 * Toggle sensitive file access for a FullAuto task.
 */
export async function setTaskSensitiveAccess(taskId, allowed) {
  try {
    await api.setTaskSensitiveAccess(taskId, allowed);
  } catch (e) {
    console.error('Failed to set sensitive access:', e);
    return false;
  }
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    task.sensitiveAccess = allowed;
    agentStore.setState({ tasks: { ...tasks } });
  }
  return true;
}

export async function deleteTaskAction(taskId) {
  try {
    await api.deleteTask(taskId);
    const tasks = { ...agentStore.getState('tasks') };
    delete tasks[taskId];
    const activeId = agentStore.getState('activeTaskId');
    agentStore.setState({
      tasks,
      activeTaskId: activeId === taskId ? null : activeId,
    });
  } catch (e) {
    console.error('Failed to delete task:', e);
  }
}

const FILE_MUTATING_TOOLS = new Set([
  'create_file',
  'edit_file',
  'apply_patch',
]);

// Tools that may modify files but we can't determine which path was affected
const BROAD_MUTATING_TOOLS = new Set([
  'run_command',
]);

function _getTaskProjectRoot(taskId) {
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return null;
  const projectId = task.project_id || task.projectId;
  if (!projectId) return null;
  const projects = workspaceStore.getState('projects');
  const project = projects.find((p) => String(p.id) === String(projectId));
  return project ? project.root_path : null;
}

function _maybeRefreshFileTree(taskId, toolUseId) {
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return;

  for (let i = task.messages.length - 1; i >= 0; i--) {
    const msg = task.messages[i];
    if (msg.role !== 'assistant') continue;
    for (const block of msg.content || []) {
      if (block.type === 'tool_use' && block.id === toolUseId) {
        const root = _getTaskProjectRoot(taskId);
        if (!root) return;

        if (BROAD_MUTATING_TOOLS.has(block.name)) {
          // run_command etc. — can't know which files changed, do a full refresh
          refreshProject(root);
          return;
        }

        if (!FILE_MUTATING_TOOLS.has(block.name)) return;

        const relPath = block.input?.path;
        if (relPath) {
          const sep = root.includes('/') ? '/' : '\\';
          const absPath = root.replace(/[\\/]+$/, '') + sep + relPath.replace(/^[\\/]+/, '');
          refreshAffectedDirectory(absPath);
        } else {
          refreshProject(root);
        }
        return;
      }
    }
    break;
  }
}

function _refreshProjectForTask(taskId) {
  const root = _getTaskProjectRoot(taskId);
  if (root) refreshProject(root);
}
