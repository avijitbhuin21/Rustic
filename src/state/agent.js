import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';

export const PLACEHOLDER_PROJECT = {
  id: '',
  name: '',
  root: '',
};

// IMPORTANT: these names must match the strings the Rust backend passes to
// app.emit(...) in src-tauri/src/commands/agent/mod.rs (and stream_coalesce.rs).
// The earlier names `agent-stream-text`, `agent-status`, `agent-question-request`
// did NOT match anything the backend emits, so the chat silently stopped
// receiving streamed text, status updates, and ask-user requests.
const AGENT_EVENTS = [
  'agent-stream',
  'agent-tool-use',
  'agent-tool-result',
  'agent-cost-update',
  'agent-task-status',
  'agent-task-complete',
  'agent-permission-request',
  'agent-ask-user-request',
  'agent-thinking-delta',
  'agent-thinking-done',
  'agent-todo-updated',
  'agent-title-changed',
  // Sub-agent lifecycle. Each event is keyed by (task_id, agent_id) so the
  // store can keep an independent live transcript per spawned child for the
  // read-only sub-agent chat view. Missing these handlers is what makes
  // spawned sub-agents look "frozen" — the backend streams everything in
  // real time, the frontend just wasn't listening.
  'agent-subagent-spawned',
  'agent-subagent-text-delta',
  'agent-subagent-thinking-delta',
  'agent-subagent-tool-use',
  'agent-subagent-tool-result',
  'agent-subagent-cost-update',
  'agent-subagent-completed',
  'agent-subagent-failed',
];

function safeInvoke(cmd, args) {
  try {
    return invoke(cmd, args);
  } catch (e) {
    return Promise.reject(e);
  }
}

function isTauriAvailable() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

// Persist the user's last model pick across restarts. Stored as a single JSON
// blob so we can grow the schema later without churning two keys in lockstep.
const MODEL_PICK_KEY = 'rustic.agent.selectedModel';
const THINKING_TIER_KEY = 'rustic.agent.thinkingTier';
const PERMISSION_LEVEL_KEY = 'rustic.agent.permissionLevel';

const VALID_THINKING_TIERS = new Set(['off', 'low', 'medium', 'high', 'max']);
const VALID_PERMISSION_LEVELS = new Set(['Chat', 'ManualEdit', 'FullAuto']);

function loadPersistedModelPick() {
  if (typeof window === 'undefined' || !window.localStorage) return null;
  try {
    const raw = window.localStorage.getItem(MODEL_PICK_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return null;
    const { provider, modelId } = parsed;
    if (typeof provider !== 'string' || typeof modelId !== 'string') return null;
    return { provider, modelId };
  } catch {
    return null;
  }
}

function persistModelPick(provider, modelId) {
  if (typeof window === 'undefined' || !window.localStorage) return;
  try {
    if (!provider || !modelId) {
      window.localStorage.removeItem(MODEL_PICK_KEY);
      return;
    }
    window.localStorage.setItem(
      MODEL_PICK_KEY,
      JSON.stringify({ provider, modelId }),
    );
  } catch {}
}

function loadPersistedScalar(key, allowed) {
  if (typeof window === 'undefined' || !window.localStorage) return null;
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return null;
    return allowed.has(raw) ? raw : null;
  } catch {
    return null;
  }
}

function persistScalar(key, value) {
  if (typeof window === 'undefined' || !window.localStorage) return;
  try {
    if (!value) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {}
}

const PERSISTED_MODEL_PICK = loadPersistedModelPick();
const PERSISTED_THINKING_TIER = loadPersistedScalar(THINKING_TIER_KEY, VALID_THINKING_TIERS);
const PERSISTED_PERMISSION_LEVEL = loadPersistedScalar(PERMISSION_LEVEL_KEY, VALID_PERMISSION_LEVELS);

// Map a user-facing thinking tier to a token budget for the backend. These
// are conservative defaults — backend can clamp to model-specific maxima.
// 'off' returns null so the backend skips extended-thinking entirely.
export function thinkingTierToBudget(tier) {
  switch (tier) {
    case 'low':    return 1024;
    case 'medium': return 4096;
    case 'high':   return 16384;
    case 'max':    return 32768;
    default:       return null;
  }
}

// Which tiers a given model supports. We key on model id substring because the
// backend's model registry uses provider-prefixed ids we don't fully control;
// matching by family keeps this resilient to id format changes.
export function tiersForModel(modelId) {
  const id = (modelId || '').toLowerCase();
  if (id.includes('opus'))    return ['off', 'low', 'medium', 'high', 'max'];
  if (id.includes('sonnet'))  return ['off', 'low', 'medium', 'high'];
  if (id.includes('haiku'))   return ['off', 'low'];
  if (id.includes('gpt-5'))   return ['off', 'low', 'medium', 'high'];
  if (id.includes('gemini'))  return ['off', 'low', 'medium', 'high'];
  // Fall back to the four-tier shape; backend will ignore unsupported budgets.
  return ['off', 'low', 'medium', 'high'];
}

// Convert backend MessageDto[] (from get_task_messages) into the shape the
// frontend chat state uses. Two transforms matter:
//   1. Anthropic's API convention puts tool_result blocks inside *user* role
//      messages. The live event pipeline instead emits a synthetic role:'tool'
//      message (see addToolResult above). buildTurns / groupToolResults in
//      chat-view.jsx rely on that role:'tool' shape — without the remap, a
//      reloaded turn would render an empty user bubble for every tool result.
//   2. The DTO has no id/timestamp; we synthesize stable per-task ids so
//      React keys stay consistent across re-renders.
// Canned assistant reply paired with the [Project Memory] pseudo-user message
// injected by src-tauri/src/commands/agent/mod.rs. Filter both out of loaded
// history so they don't render as a real exchange. Keep in sync if the backend
// string changes.
const MEMORY_INJECT_ACK = "Memory loaded. I'll reference this context as needed.";

// Synthetic text blocks the executor injects into otherwise-tool-result user
// messages (sub-agent lifecycle notices, system nudges, etc.). They're not
// user-authored prompts — strip them before turn-building so they don't
// surface as empty user bubbles or merge into the real user text.
function isSyntheticInjection(text) {
  if (typeof text !== 'string') return false;
  return (
    text.startsWith('[Project Memory]') ||
    text.startsWith("[Sub-agent '") ||
    text.startsWith('[All sub-agents') ||
    text.startsWith('[SYSTEM NUDGE') ||
    text.startsWith('[Messages from orchestrator]') ||
    text.startsWith('SYSTEM: one or more background terminals') ||
    text.startsWith('<project_structure>')
  );
}

function normalizeLoadedMessages(taskId, dtos) {
  if (!Array.isArray(dtos)) return [];
  const out = [];
  for (let idx = 0; idx < dtos.length; idx++) {
    const m = dtos[idx];
    const rawContent = Array.isArray(m.content) ? m.content : [];

    // Strip the canned "Memory loaded" assistant reply that pairs with the
    // [Project Memory] user message. If the only assistant text is that line,
    // drop the whole message.
    if (m.role === 'assistant') {
      const onlyText = rawContent.length === 1 && rawContent[0]?.type === 'text';
      if (onlyText && rawContent[0].text === MEMORY_INJECT_ACK) continue;
    }

    // For user messages, strip synthetic text blocks block-by-block — the
    // executor pushes [Sub-agent ...] / [SYSTEM NUDGE] into the same user
    // message that carries tool_result blocks, so we can't filter at the
    // whole-message level.
    let content = rawContent;
    if (m.role === 'user') {
      content = rawContent.filter((b) => {
        if (b?.type !== 'text') return true;
        return !isSyntheticInjection(b.text || '');
      });
      if (content.length === 0) continue;
    }

    // Bring DB block shapes into line with the frontend's live-event shape:
    //   - thinking blocks ship `{ thinking, duration_secs }` on the wire;
    //     ThinkingRow reads `text`, `durationSecs`, `done`. Without this fix
    //     reloaded thinking rows render blank and stuck on "Thinking…".
    //   - tool_result blocks ship `{ tool_use_id, content, is_error }`;
    //     groupToolResults / ToolCallCard read `output`. Without renaming,
    //     reloaded tool calls never see their result and stay "running".
    content = content.map((b) => {
      if (!b || typeof b !== 'object') return b;
      if (b.type === 'thinking') {
        return {
          ...b,
          text: b.text ?? b.thinking ?? '',
          durationSecs: b.durationSecs ?? b.duration_secs ?? 0,
          done: b.done === undefined ? true : b.done,
        };
      }
      if (b.type === 'tool_result' && b.output === undefined) {
        return { ...b, output: b.content ?? '' };
      }
      return b;
    });

    const isToolResultOnly =
      m.role === 'user' &&
      content.length > 0 &&
      content.every((b) => b && b.type === 'tool_result');
    const role = isToolResultOnly ? 'tool' : m.role;
    out.push({
      id: `hist-${taskId}-${idx}`,
      role,
      content,
      timestamp: 0,
      ...(m.turn_usage ? { turnUsage: m.turn_usage } : {}),
    });
  }
  return out;
}

export const useAgent = create((set, get) => ({
  activeProject: { id: '', name: '', root: '' },
  tasks: [],
  // tasksByProject: { [projectId]: Task[] } — persistent cache for the task
  // tree on the left in agent mode. Survives project switches so the tree
  // doesn't have to refetch every time the user clicks around.
  tasksByProject: {},
  // expandedProjects: { [projectId]: true } — which project nodes are open
  // in the agent task tree. Defaults to expanded for the active project.
  expandedProjects: {},
  // historyLimitByProject: { [projectId]: number } — paginated "Load more"
  // counter for non-running tasks. Defaults to 5.
  historyLimitByProject: {},
  activeTaskId: null,
  messagesByTask: {},
  todosByTask: {},
  costByTask: {},
  statusByTask: {},
  streamingByTask: {},
  thinkingByTask: {},
  // Sub-agent records hydrated from the DB on task open. Keyed by taskId,
  // shape mirrors rustic-db SubagentRecord (model, prompt, summary, status,
  // costs, output_text, tool_calls_json). Lets activity-style panels show the
  // full sub-agent picture instead of just the brief spawn tool_result.
  subagentRecordsByTask: {},
  // Live, in-memory transcript of each sub-agent the active task has spawned.
  //   subagentsByTask: { [taskId]: { [agentId]: SubagentLive } }
  // SubagentLive = {
  //   agentId, model, prompt, status: 'running'|'completed'|'failed',
  //   summary, error, cost, createdAt, lastUpdate,
  //   messages: [...]   // same shape as messagesByTask so ChatTurn can render
  // }
  // Populated by the agent-subagent-* event handlers below. This is what
  // the read-only sub-agent chat sheet reads from when the user clicks a
  // spawn_subagent tool card.
  subagentsByTask: {},
  // Currently-opened sub-agent in the read-only chat sheet, or null.
  // Shape: { taskId, agentId }.
  openSubagent: null,
  // Per-task gate so we only hit the DB once per task open. Cleared on
  // project switch (alongside messagesByTask) so cross-project navigation
  // refetches. A task with an active live stream skips load entirely — its
  // in-memory messages are the source of truth.
  historyLoadedByTask: {},
  pendingPermission: null,
  pendingQuestion: null,
  models: [],
  // Provider entries from `get_ai_config`. Shape: [{ provider_type, name?,
  // default_model, base_url?, has_api_key }]. Used by the model picker to
  // know which provider groups to surface (and with which base_url to call
  // fetch_ai_models for the Compatible flavour).
  providersConfig: [],
  selectedModel: PERSISTED_MODEL_PICK?.modelId ?? null,
  selectedProvider: PERSISTED_MODEL_PICK?.provider ?? null,
  // thinkingTier: 'off' | 'low' | 'medium' | 'high' | 'max'. Determines the
  // thinking budget forwarded to send_message. Stored once at the user level
  // (not per task) because reasoning effort is a workflow preference, not a
  // per-conversation setting — users tend to pick a tier and stick with it.
  thinkingTier: PERSISTED_THINKING_TIER ?? 'off',
  // Per-tool on/off toggles surfaced from the chat-input "Tools" dropdown.
  // Keyed by stable tool id (mirrors the keys backend uses where applicable —
  // `web_search`, `web_fetch`, etc.). Defaults to all enabled. Frontend-only
  // for now; the backend doesn't yet accept a disabled-tools list per send,
  // so flipping these only affects the picker's UI state. Wiring to a real
  // gate is a follow-up.
  toolStates: {
    web_search: true,
    web_fetch: true,
    image_create: true,
    video_create: true,
    animate: true,
  },
  // Permission mode for the active task. Three user-facing modes mapped onto
  // the backend's four levels: Chat (read-only), ManualEdit (asks before each
  // write), FullAuto (bypass all prompts including shell + sub-agents). The
  // backend's AutoEdit tier is intentionally skipped from the picker — the
  // three-mode UX is cleaner. Persists across task switches as a workflow
  // preference; sync'd to the active task via `set_task_permissions` on
  // change and applied to fresh tasks at send_message time.
  permissionLevel: PERSISTED_PERMISSION_LEVEL ?? 'ManualEdit',
  listenersBound: false,
  initialized: false,
  // tasksLoadedByProject: { [projectId]: true } — gate so we don't re-fetch
  // the same project's task list every time the tree re-mounts.
  tasksLoadedByProject: {},

  setActiveTask: (taskId) => {
    set({ activeTaskId: taskId });
    // Fire-and-forget: hydrate chat history, todos, cost, and sub-agent
    // records from the backend. Skips work if we already have a live
    // in-memory transcript for this task (active stream is authoritative).
    if (taskId) {
      get().loadTaskHistory(taskId).catch((e) => {
        // eslint-disable-next-line no-console
        console.error('[agent.setActiveTask] loadTaskHistory failed', { taskId, error: e });
      });
    }
  },

  setActiveProject: (project) => {
    const next = project ?? { id: '', name: '', root: '' };
    const prev = get().activeProject;
    if (prev.id === next.id) return;
    set((s) => ({
      activeProject: next,
      // Mirror the cached tasks for this project into the flat `tasks` field
      // so the existing chat/task-switcher selectors keep working unchanged.
      tasks: s.tasksByProject[next.id] || [],
      activeTaskId: null,
      // Per-task transient state is cleared on project switch — we don't keep
      // every project's message history in memory, and the chat refetches
      // when a task is opened.
      messagesByTask: {},
      todosByTask: {},
      costByTask: {},
      statusByTask: {},
      streamingByTask: {},
      thinkingByTask: {},
      subagentRecordsByTask: {},
      subagentsByTask: {},
      openSubagent: null,
      historyLoadedByTask: {},
      initialized: false,
      // Default the new project to expanded in the task tree.
      expandedProjects: { ...s.expandedProjects, [next.id]: true },
    }));
  },

  toggleProjectExpanded: (projectId) =>
    set((s) => ({
      expandedProjects: {
        ...s.expandedProjects,
        [projectId]: !s.expandedProjects[projectId],
      },
    })),

  setProjectExpanded: (projectId, expanded) =>
    set((s) => ({
      expandedProjects: { ...s.expandedProjects, [projectId]: !!expanded },
    })),

  bumpHistoryLimit: (projectId, by = 5) =>
    set((s) => ({
      historyLimitByProject: {
        ...s.historyLimitByProject,
        [projectId]: (s.historyLimitByProject[projectId] || 5) + by,
      },
    })),

  upsertTaskInCache: (projectId, task) =>
    set((s) => {
      if (!projectId || !task?.id) return s;
      const list = s.tasksByProject[projectId] || [];
      const idx = list.findIndex((t) => t.id === task.id);
      const next = idx >= 0
        ? list.map((t, i) => (i === idx ? { ...t, ...task } : t))
        : [task, ...list];
      const patch = { tasksByProject: { ...s.tasksByProject, [projectId]: next } };
      if (s.activeProject.id === projectId) patch.tasks = next;
      return patch;
    }),

  removeTaskFromCache: (projectId, taskId) =>
    set((s) => {
      if (!projectId || !taskId) return s;
      const list = s.tasksByProject[projectId] || [];
      const next = list.filter((t) => t.id !== taskId);
      const patch = { tasksByProject: { ...s.tasksByProject, [projectId]: next } };
      if (s.activeProject.id === projectId) patch.tasks = next;
      return patch;
    }),

  appendUserMessage: (taskId, text, attachments = []) => {
    const msg = {
      id: `local-${Date.now()}`,
      role: 'user',
      content: [{ type: 'text', text }],
      attachments,
      timestamp: Date.now(),
    };
    set((s) => ({
      messagesByTask: {
        ...s.messagesByTask,
        [taskId]: [...(s.messagesByTask[taskId] || []), msg],
      },
    }));
  },

  appendAssistantText: (taskId, delta) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        const content = [...(last.content || [])];
        const lastBlock = content[content.length - 1];
        if (lastBlock && lastBlock.type === 'text') {
          content[content.length - 1] = { ...lastBlock, text: (lastBlock.text || '') + delta };
        } else {
          content.push({ type: 'text', text: delta });
        }
        list[list.length - 1] = { ...last, content };
      } else {
        list.push({
          id: `assist-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'text', text: delta }],
          streaming: true,
          timestamp: Date.now(),
        });
      }
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: true },
      };
    });
  },

  appendThinking: (taskId, delta) => {
    // Push thinking text as an inline content block on the assistant turn so
    // chat-turn.jsx can render it alongside the eventual response. Previously
    // we only stored it in a side `thinkingByTask` map that nothing rendered,
    // so the user had no visible signal the model was reasoning. We still
    // keep the side map for any consumer that wants the raw stream.
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        const content = [...(last.content || [])];
        const lastBlock = content[content.length - 1];
        if (lastBlock && lastBlock.type === 'thinking' && !lastBlock.done) {
          content[content.length - 1] = {
            ...lastBlock,
            text: (lastBlock.text || '') + delta,
          };
        } else {
          content.push({ type: 'thinking', text: delta });
        }
        list[list.length - 1] = { ...last, content };
      } else {
        list.push({
          id: `assist-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'thinking', text: delta }],
          streaming: true,
          timestamp: Date.now(),
        });
      }
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: true },
        thinkingByTask: {
          ...s.thinkingByTask,
          [taskId]: (s.thinkingByTask[taskId] || '') + delta,
        },
      };
    });
  },

  // Mark the most recent thinking block on a task as finalised. The backend
  // emits `agent-thinking-done` with the elapsed seconds once the model has
  // closed the thinking section; we stamp `done: true` + `durationSecs` so
  // chat-turn.jsx can flip from "Thinking…" to "Reasoned for Ns".
  markThinkingDone: (taskId, durationSecs) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      for (let i = list.length - 1; i >= 0; i--) {
        const m = list[i];
        if (m.role !== 'assistant') continue;
        const content = m.content || [];
        let touched = false;
        const nextContent = content.map((b) => {
          if (b.type === 'thinking' && !b.done) {
            touched = true;
            return { ...b, done: true, durationSecs };
          }
          return b;
        });
        if (touched) {
          list[i] = { ...m, content: nextContent };
          break;
        }
      }
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  addToolUse: (taskId, toolUseId, name, input) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant') {
        last.streaming = false;
      }
      list.push({
        id: `tool-${toolUseId}`,
        role: 'assistant',
        content: [{ type: 'tool_use', id: toolUseId, name, input }],
        timestamp: Date.now(),
      });
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  addToolResult: (taskId, toolUseId, output, isError) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      list.push({
        id: `tool-result-${toolUseId}`,
        role: 'tool',
        content: [{ type: 'tool_result', tool_use_id: toolUseId, output, is_error: !!isError }],
        timestamp: Date.now(),
      });
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  finishStream: (taskId) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.streaming) {
        list[list.length - 1] = { ...last, streaming: false };
      }
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: false },
      };
    });
  },

  // --- Sub-agent helpers ----------------------------------------------------
  // Mutating helpers that mirror the main-task ones (appendAssistantText,
  // appendThinking, addToolUse, addToolResult, finishStream) but operate on
  // a nested `subagentsByTask[taskId][agentId].messages` array. Kept here
  // alongside the main helpers so the same review can spot drift between
  // the two transcript shapes.
  //
  // Sub-agent message stream uses the same role + content shape as the main
  // chat (role:'user' with text, role:'assistant' with text/thinking/tool_use,
  // role:'tool' with tool_result). That way the existing <ChatTurn /> can
  // render a sub-agent's transcript without any branching.

  _ensureSubagent: (taskId, agentId, patch = {}) =>
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const existing = taskMap[agentId];
      if (existing && Object.keys(patch).length === 0) return s;
      const now = Date.now();
      const next = existing
        ? { ...existing, ...patch, lastUpdate: now }
        : {
            agentId,
            model: '',
            prompt: '',
            status: 'running',
            summary: '',
            error: '',
            cost: null,
            messages: [],
            createdAt: now,
            lastUpdate: now,
            ...patch,
          };
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: { ...taskMap, [agentId]: next },
        },
      };
    }),

  appendSubagentText: (taskId, agentId, delta) => {
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const sub = taskMap[agentId];
      if (!sub) return s;
      const list = [...sub.messages];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        const content = [...(last.content || [])];
        const lastBlock = content[content.length - 1];
        if (lastBlock && lastBlock.type === 'text') {
          content[content.length - 1] = {
            ...lastBlock,
            text: (lastBlock.text || '') + delta,
          };
        } else {
          content.push({ type: 'text', text: delta });
        }
        list[list.length - 1] = { ...last, content };
      } else {
        list.push({
          id: `sub-assist-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'text', text: delta }],
          streaming: true,
          timestamp: Date.now(),
        });
      }
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: {
            ...taskMap,
            [agentId]: { ...sub, messages: list, lastUpdate: Date.now() },
          },
        },
      };
    });
  },

  appendSubagentThinking: (taskId, agentId, delta) => {
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const sub = taskMap[agentId];
      if (!sub) return s;
      const list = [...sub.messages];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        const content = [...(last.content || [])];
        const lastBlock = content[content.length - 1];
        if (lastBlock && lastBlock.type === 'thinking' && !lastBlock.done) {
          content[content.length - 1] = {
            ...lastBlock,
            text: (lastBlock.text || '') + delta,
          };
        } else {
          content.push({ type: 'thinking', text: delta });
        }
        list[list.length - 1] = { ...last, content };
      } else {
        list.push({
          id: `sub-assist-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'thinking', text: delta }],
          streaming: true,
          timestamp: Date.now(),
        });
      }
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: {
            ...taskMap,
            [agentId]: { ...sub, messages: list, lastUpdate: Date.now() },
          },
        },
      };
    });
  },

  addSubagentToolUse: (taskId, agentId, toolUseId, name, input) => {
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const sub = taskMap[agentId];
      if (!sub) return s;
      const list = [...sub.messages];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant') {
        last.streaming = false;
        // Close any open thinking block so it stops showing "Thinking…"
        last.content = (last.content || []).map((b) =>
          b.type === 'thinking' && !b.done ? { ...b, done: true } : b,
        );
      }
      list.push({
        id: `sub-tool-${toolUseId}`,
        role: 'assistant',
        content: [{ type: 'tool_use', id: toolUseId, name, input }],
        timestamp: Date.now(),
      });
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: {
            ...taskMap,
            [agentId]: { ...sub, messages: list, lastUpdate: Date.now() },
          },
        },
      };
    });
  },

  addSubagentToolResult: (taskId, agentId, toolUseId, output, isError) => {
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const sub = taskMap[agentId];
      if (!sub) return s;
      const list = [...sub.messages];
      list.push({
        id: `sub-tool-result-${toolUseId}`,
        role: 'tool',
        content: [
          {
            type: 'tool_result',
            tool_use_id: toolUseId,
            output,
            is_error: !!isError,
          },
        ],
        timestamp: Date.now(),
      });
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: {
            ...taskMap,
            [agentId]: { ...sub, messages: list, lastUpdate: Date.now() },
          },
        },
      };
    });
  },

  finalizeSubagent: (taskId, agentId, finalPatch) => {
    set((s) => {
      const taskMap = s.subagentsByTask[taskId] || {};
      const sub = taskMap[agentId];
      if (!sub) return s;
      const list = (sub.messages || []).map((m) =>
        m.role === 'assistant' && m.streaming ? { ...m, streaming: false } : m,
      );
      // Append the final summary (or error) as a closing assistant turn so the
      // sub-agent's chat ends with a visible reply, not a half-streamed block.
      if (finalPatch && finalPatch.summary) {
        list.push({
          id: `sub-final-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'text', text: finalPatch.summary }],
          timestamp: Date.now(),
        });
      } else if (finalPatch && finalPatch.error) {
        list.push({
          id: `sub-error-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'text', text: `**Failed:** ${finalPatch.error}` }],
          timestamp: Date.now(),
        });
      }
      return {
        subagentsByTask: {
          ...s.subagentsByTask,
          [taskId]: {
            ...taskMap,
            [agentId]: {
              ...sub,
              ...finalPatch,
              messages: list,
              lastUpdate: Date.now(),
            },
          },
        },
      };
    });
  },

  openSubagentSheet: (taskId, agentId) =>
    set({ openSubagent: taskId && agentId ? { taskId, agentId } : null }),
  closeSubagentSheet: () => set({ openSubagent: null }),

  setCost: (taskId, cost) =>
    set((s) => ({ costByTask: { ...s.costByTask, [taskId]: cost } })),

  setStatus: (taskId, status) =>
    set((s) => {
      // Terminal statuses must also clear the streaming flag — otherwise a
      // task that ends via 'cancelled' / 'failed' (e.g. user clicked Stop, or
      // the backend errored out) leaves the prompt-box stuck in Stop-button
      // mode because only 'agent-task-complete' calls finishStream.
      const TERMINAL = new Set([
        'complete',
        'completed',
        'cancelled',
        'canceled',
        'aborted',
        'failed',
        'error',
      ]);
      const patch = { statusByTask: { ...s.statusByTask, [taskId]: status } };
      if (TERMINAL.has(String(status).toLowerCase())) {
        patch.streamingByTask = { ...s.streamingByTask, [taskId]: false };
      }
      return patch;
    }),

  setTodos: (taskId, todos) =>
    set((s) => ({ todosByTask: { ...s.todosByTask, [taskId]: todos } })),

  setTitle: (taskId, title) =>
    set((s) => {
      const updateList = (list) =>
        list.map((t) => (t.id === taskId ? { ...t, title } : t));
      const tasksByProject = Object.fromEntries(
        Object.entries(s.tasksByProject).map(([pid, list]) => [pid, updateList(list)]),
      );
      return {
        tasks: updateList(s.tasks),
        tasksByProject,
      };
    }),

  openPermission: (req) => set({ pendingPermission: req }),
  closePermission: () => set({ pendingPermission: null }),
  openQuestion: (req) => set({ pendingQuestion: req }),
  closeQuestion: () => set({ pendingQuestion: null }),

  setModels: (models) => set({ models }),
  setSelectedModel: (provider, modelId) => {
    persistModelPick(provider, modelId);
    set({ selectedProvider: provider, selectedModel: modelId });
  },
  setThinkingTier: (tier) => {
    const next = tier || 'off';
    persistScalar(THINKING_TIER_KEY, next);
    set({ thinkingTier: next });
  },

  toggleTool: (id) =>
    set((s) => ({
      toolStates: { ...s.toolStates, [id]: !s.toolStates[id] },
    })),

  setToolEnabled: (id, enabled) =>
    set((s) => ({
      toolStates: { ...s.toolStates, [id]: !!enabled },
    })),

  // Switch the permission mode. Pushes to the active task immediately so the
  // running executor sees the change (matches the old plan-mode toggle UX).
  // No-op on the backend if no task is active yet — the chosen level will be
  // applied to the next task by sendMessage.
  async setPermissionLevel(level) {
    persistScalar(PERMISSION_LEVEL_KEY, level);
    set({ permissionLevel: level });
    const taskId = get().activeTaskId;
    if (!taskId || !isTauriAvailable()) return;
    try {
      await safeInvoke('set_task_permissions', { taskId, level });
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Couldn't change mode: ${msg}`);
    }
  },

  async ensureTask() {
    const state = get();
    if (state.activeTaskId) return state.activeTaskId;
    const project = state.activeProject;
    const stamp = (task) => {
      const pid = project.id;
      set((s) => {
        const list = pid ? (s.tasksByProject[pid] || []) : s.tasks;
        const next = [...list, task];
        const patch = { activeTaskId: task.id };
        if (pid) {
          patch.tasksByProject = { ...s.tasksByProject, [pid]: next };
          if (s.activeProject.id === pid) patch.tasks = next;
        } else {
          patch.tasks = next;
        }
        return patch;
      });
    };
    if (!isTauriAvailable()) {
      const t = { id: `mock-${Date.now()}`, title: 'New Task' };
      stamp(t);
      return t.id;
    }
    if (!project.id) {
      const t = { id: `local-${Date.now()}`, title: 'New Task' };
      stamp(t);
      return t.id;
    }
    try {
      const task = await safeInvoke('create_task', {
        projectId: project.id,
        projectName: project.name,
        projectRoot: project.root,
        title: 'New Task',
      });
      stamp(task);
      return task.id;
    } catch (e) {
      // eslint-disable-next-line no-console
      console.error('[agent.ensureTask] create_task failed, falling back to local id', { project, error: e });
      const t = { id: `local-${Date.now()}`, title: 'New Task' };
      stamp(t);
      return t.id;
    }
  },

  // Create a fresh task explicitly bound to a given project (used by the
  // per-project "+" button in the task tree). Differs from ensureTask in that
  // it always creates a new task even if one is already active, and lets the
  // caller drive which project the task belongs to.
  async createTaskForProject(project) {
    const proj = project ?? get().activeProject;
    if (!proj?.id) {
      // eslint-disable-next-line no-console
      console.warn('[agent.createTaskForProject] no project.id, using local id', { project: proj });
      const t = { id: `local-${Date.now()}`, title: 'New Task' };
      get().upsertTaskInCache(proj?.id || '', t);
      set({ activeTaskId: t.id });
      return t.id;
    }
    if (!isTauriAvailable()) {
      const t = { id: `mock-${Date.now()}`, title: 'New Task' };
      get().upsertTaskInCache(proj.id, t);
      set({ activeTaskId: t.id });
      return t.id;
    }
    if (!proj.root) {
      const msg = `Can't create task: project "${proj.name || proj.id}" has no root path. Try removing and re-adding the project.`;
      // eslint-disable-next-line no-console
      console.error('[agent.createTaskForProject] missing project.root', { project: proj });
      toast.error(msg);
      throw new Error(msg);
    }
    try {
      const task = await safeInvoke('create_task', {
        projectId: proj.id,
        projectName: proj.name,
        projectRoot: proj.root,
        title: 'New Task',
      });
      get().upsertTaskInCache(proj.id, task);
      set({ activeTaskId: task.id });
      return task.id;
    } catch (e) {
      // eslint-disable-next-line no-console
      console.error('[agent.createTaskForProject] create_task failed', { project: proj, error: e });
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Couldn't create task: ${msg}`);
      throw e;
    }
  },

  async sendMessage(text, attachments = []) {
    const state = get();
    const taskId = await state.ensureTask();
    state.appendUserMessage(taskId, text, attachments);
    if (!isTauriAvailable()) {
      toast.error('Tauri runtime unavailable — open this in the desktop app to talk to the agent.');
      return;
    }
    // Push the current mode at every send. Cheap on the backend (just sets
    // an Arc) and guarantees a freshly-created task picks up the user's
    // chosen mode even though create_task didn't know about it.
    try {
      await safeInvoke('set_task_permissions', {
        taskId,
        level: state.permissionLevel,
      });
    } catch (e) { /* non-fatal — surfaces via send_message error if it matters */ }
    try {
      await safeInvoke('send_message', {
        taskId,
        message: text,
        thinkingBudget: thinkingTierToBudget(state.thinkingTier),
        images: attachments ?? [],
      });
    } catch (e) {
      // Surface backend rejections — silently swallowing them made the chat
      // appear broken (message visible, no reply) with no clue why.
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`send_message failed: ${msg}`);
      // eslint-disable-next-line no-console
      console.error('[agent] send_message failed', e);
      set((s) => ({
        streamingByTask: { ...s.streamingByTask, [taskId]: false },
      }));
    }
  },

  async abortActive() {
    const taskId = get().activeTaskId;
    if (!taskId || !isTauriAvailable()) return;
    // Optimistically clear the streaming flag so the Stop button immediately
    // flips back to "Send" mode. The backend will also emit a terminal status
    // (cancelled/failed/complete) which setStatus handles below, but it can
    // arrive slowly when the model is mid-token — without this, the button
    // keeps pulsing for several seconds after the click.
    set((s) => ({
      streamingByTask: { ...s.streamingByTask, [taskId]: false },
    }));
    try {
      await safeInvoke('abort_task', { taskId });
    } catch (e) {}
  },

  async respondPermission(approved) {
    const req = get().pendingPermission;
    if (!req) return;
    set({ pendingPermission: null });
    if (!isTauriAvailable()) return;
    try {
      await safeInvoke('respond_to_permission', {
        taskId: req.task_id,
        requestId: req.request_id,
        approved,
      });
    } catch (e) {}
  },

  async respondQuestion(userInput, opts = {}) {
    const req = get().pendingQuestion;
    if (!req) return;
    set({ pendingQuestion: null });
    if (!isTauriAvailable()) return;
    const cancelled = !!opts.cancelled;
    try {
      await safeInvoke('respond_to_ask_user', {
        requestId: req.request_id,
        // `answers` is a serde_json::Value on the Rust side: passing the raw
        // string is accepted as a JSON string scalar; null when cancelled.
        answers: cancelled ? null : userInput,
        cancelled,
      });
    } catch (e) {}
  },

  async loadInitial() {
    if (get().initialized) return;
    set({ initialized: true });
    if (!isTauriAvailable()) return;
    try {
      const projectId = get().activeProject.id;
      if (!projectId) {
        set({ tasks: [] });
      } else {
        await get().loadTasksForProject(projectId);
      }
    } catch (e) {}
    try {
      const known = await safeInvoke('list_known_models');
      set({ models: Array.isArray(known) ? known : [] });
    } catch (e) {}
    try {
      const cfg = await safeInvoke('get_ai_config');
      const providers = Array.isArray(cfg?.providers) ? cfg.providers : [];
      // Normalise: backend returns the api_key field redacted (empty) for the
      // webview, but the presence of a non-empty value indicates configuration.
      // We also surface a has_api_key boolean derived from `default_model` —
      // configured providers always have a default_model set in the config.
      set({
        providersConfig: providers.map((p) => ({
          provider_type: p.provider_type,
          name: p.name || null,
          default_model: p.default_model || null,
          base_url: p.base_url || null,
          has_api_key: !!p.has_api_key || (!!p.default_model && p.provider_type !== ''),
        })),
      });
    } catch (e) {}
  },

  // Refresh the provider config from disk. Called after the settings panel
  // saves an ai_config change so the model picker reflects new providers
  // without an app restart. Idempotent + safe to call from any mount.
  async refreshProvidersConfig() {
    if (!isTauriAvailable()) return;
    try {
      const cfg = await safeInvoke('get_ai_config');
      const providers = Array.isArray(cfg?.providers) ? cfg.providers : [];
      set({
        providersConfig: providers.map((p) => ({
          provider_type: p.provider_type,
          name: p.name || null,
          default_model: p.default_model || null,
          base_url: p.base_url || null,
          has_api_key: !!p.has_api_key || (!!p.default_model && p.provider_type !== ''),
        })),
      });
    } catch (e) {}
  },

  // Lazy per-project task loader. The task tree calls this when a project node
  // is first expanded; results are cached in tasksByProject. Pass force=true to
  // refetch (e.g. after delete/rename).
  async loadTasksForProject(projectId, opts = {}) {
    if (!projectId) return [];
    const { force = false } = opts;
    const state = get();
    if (!force && state.tasksLoadedByProject[projectId]) {
      return state.tasksByProject[projectId] || [];
    }
    if (!isTauriAvailable()) {
      set((s) => ({
        tasksLoadedByProject: { ...s.tasksLoadedByProject, [projectId]: true },
      }));
      return [];
    }
    try {
      const tasks = await safeInvoke('list_tasks', { projectId });
      const list = Array.isArray(tasks) ? tasks : [];
      set((s) => {
        const patch = {
          tasksByProject: { ...s.tasksByProject, [projectId]: list },
          tasksLoadedByProject: { ...s.tasksLoadedByProject, [projectId]: true },
        };
        if (s.activeProject.id === projectId) patch.tasks = list;
        return patch;
      });
      return list;
    } catch (e) {
      set((s) => ({
        tasksLoadedByProject: { ...s.tasksLoadedByProject, [projectId]: true },
      }));
      return [];
    }
  },

  // Hydrate a task's full state from the backend: chat messages (incl. tool
  // calls + results embedded as content blocks), todos, cost, and sub-agent
  // records. Idempotent via `historyLoadedByTask` so reopening the same task
  // doesn't re-hit the DB. Skips entirely when the task already has live
  // in-memory messages — the active stream is authoritative and we must not
  // clobber partially-streamed turns with a stale DB snapshot.
  async loadTaskHistory(taskId) {
    if (!taskId) return;
    const state = get();
    if (state.historyLoadedByTask[taskId]) return;
    const existing = state.messagesByTask[taskId];
    if (Array.isArray(existing) && existing.length > 0) {
      // Already populated (live stream or prior load). Mark loaded so we
      // don't keep retrying.
      set((s) => ({
        historyLoadedByTask: { ...s.historyLoadedByTask, [taskId]: true },
      }));
      return;
    }
    if (!isTauriAvailable()) return;

    // Mark loaded eagerly so concurrent setActiveTask calls don't double-fetch.
    // On failure below we clear the flag so a manual retry works.
    set((s) => ({
      historyLoadedByTask: { ...s.historyLoadedByTask, [taskId]: true },
    }));

    try {
      const [messages, todos, cost, subagents] = await Promise.all([
        safeInvoke('get_task_messages', { taskId }).catch((e) => {
          // eslint-disable-next-line no-console
          console.error('[agent.loadTaskHistory] get_task_messages failed', { taskId, error: e });
          return [];
        }),
        safeInvoke('get_task_todos', { taskId }).catch(() => []),
        safeInvoke('get_task_cost', { taskId }).catch(() => null),
        safeInvoke('get_subagent_records', { taskId }).catch(() => []),
      ]);

      const normalized = normalizeLoadedMessages(taskId, messages);

      set((s) => {
        // Re-check: a live stream may have started while we were awaiting. If
        // messages appeared in the interim, don't clobber them.
        const inMem = s.messagesByTask[taskId];
        if (Array.isArray(inMem) && inMem.length > 0) return s;
        return {
          messagesByTask: { ...s.messagesByTask, [taskId]: normalized },
          todosByTask: { ...s.todosByTask, [taskId]: Array.isArray(todos) ? todos : [] },
          costByTask: cost ? { ...s.costByTask, [taskId]: cost } : s.costByTask,
          subagentRecordsByTask: {
            ...s.subagentRecordsByTask,
            [taskId]: Array.isArray(subagents) ? subagents : [],
          },
        };
      });
    } catch (e) {
      // Roll back the gate so the next attempt retries.
      set((s) => {
        const next = { ...s.historyLoadedByTask };
        delete next[taskId];
        return { historyLoadedByTask: next };
      });
      // eslint-disable-next-line no-console
      console.error('[agent.loadTaskHistory] hydrate failed', { taskId, error: e });
      throw e;
    }
  },

  async bindListeners() {
    // True singleton. Previously every caller (AgentPanel, AgentTaskTree)
    // tried to bind+cleanup, but only one got the real cleanup function —
    // the others got a no-op. The moment the cleanup-owning component
    // unmounted, all listeners were torn down and any still-mounted callers
    // were left receiving no events (the "agent response doesn't show in
    // UI" bug). Listeners are now bound once for the lifetime of the page;
    // we return a no-op so component effects can't tear them down.
    if (get().listenersBound) return () => {};
    set({ listenersBound: true });
    if (!isTauriAvailable()) return () => {};

    const handlers = {
      'agent-stream': (p) => get().appendAssistantText(p.task_id, p.text || ''),
      'agent-thinking-delta': (p) => get().appendThinking(p.task_id, p.text || ''),
      'agent-thinking-done': (p) =>
        get().markThinkingDone(p.task_id, p.duration_secs ?? 0),
      'agent-tool-use': (p) =>
        get().addToolUse(p.task_id, p.tool_use_id, p.tool_name, p.tool_input),
      'agent-tool-result': (p) =>
        get().addToolResult(p.task_id, p.tool_use_id, p.output, p.is_error),
      'agent-cost-update': (p) => get().setCost(p.task_id, p.cost),
      'agent-task-status': (p) => get().setStatus(p.task_id, p.status),
      'agent-task-complete': (p) => {
        get().finishStream(p.task_id);
        get().setStatus(p.task_id, 'complete');
      },
      'agent-permission-request': (p) => get().openPermission(p),
      'agent-ask-user-request': (p) => get().openQuestion(p),
      'agent-todo-updated': (p) => get().setTodos(p.task_id, p.todos || []),
      'agent-title-changed': (p) => get().setTitle(p.task_id, p.title),
      // Sub-agent stream. Each event includes both task_id and agent_id; the
      // store keeps an independent live transcript per spawned child so the
      // user can click into a sub-agent and watch it work in real time.
      'agent-subagent-spawned': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id, {
          model: p.model || '',
          prompt: p.prompt || '',
          status: 'running',
        });
        // Seed the sub-agent's transcript with the original prompt as the
        // opening user message so the chat view doesn't start blank.
        if (p.prompt) {
          set((s) => {
            const taskMap = s.subagentsByTask[p.task_id] || {};
            const sub = taskMap[p.agent_id];
            if (!sub || sub.messages.length > 0) return s;
            return {
              subagentsByTask: {
                ...s.subagentsByTask,
                [p.task_id]: {
                  ...taskMap,
                  [p.agent_id]: {
                    ...sub,
                    messages: [
                      {
                        id: `sub-prompt-${p.agent_id}`,
                        role: 'user',
                        content: [{ type: 'text', text: p.prompt }],
                        timestamp: Date.now(),
                      },
                    ],
                  },
                },
              },
            };
          });
        }
      },
      'agent-subagent-text-delta': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id);
        get().appendSubagentText(p.task_id, p.agent_id, p.text || '');
      },
      'agent-subagent-thinking-delta': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id);
        get().appendSubagentThinking(p.task_id, p.agent_id, p.text || '');
      },
      'agent-subagent-tool-use': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id);
        get().addSubagentToolUse(
          p.task_id,
          p.agent_id,
          p.tool_use_id,
          p.tool_name,
          p.input,
        );
      },
      'agent-subagent-tool-result': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id);
        get().addSubagentToolResult(
          p.task_id,
          p.agent_id,
          p.tool_use_id,
          p.content,
          p.is_error,
        );
      },
      'agent-subagent-cost-update': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id, { cost: p.cost });
      },
      'agent-subagent-completed': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id, {
          model: p.model || undefined,
        });
        get().finalizeSubagent(p.task_id, p.agent_id, {
          status: 'completed',
          summary: p.summary || '',
        });
      },
      'agent-subagent-failed': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id);
        get().finalizeSubagent(p.task_id, p.agent_id, {
          status: 'failed',
          error: p.error || 'Sub-agent failed',
        });
      },
    };

    await Promise.all(
      AGENT_EVENTS.map((name) =>
        listen(name, (evt) => {
          const handler = handlers[name];
          if (handler) handler(evt.payload || {});
        })
      )
    );

    return () => {};
  },
}));
