import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isTauriAvailable } from '@/lib/platform';
import { toast } from 'sonner';
import { confirm } from '@/components/confirm-dialog';

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
  'agent-tool-use-start',
  'agent-tool-use-input-delta',
  'agent-tool-use-stop',
  'agent-tool-use',
  'agent-tool-result',
  'agent-cost-update',
  'agent-request-usage',
  'agent-task-status',
  'agent-task-complete',
  'agent-permission-request',
  'agent-ask-user-request',
  'agent-thinking-delta',
  'agent-thinking-done',
  'agent-todo-updated',
  'agent-goal-update',
  'agent-title-changed',
  'agent-context-condense-started',
  'agent-context-condense-completed',
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
  'agent-stream-retry',
  'agent-file-tracked',
  // Claude Fable 5-class safety classifier declined the request. Payload:
  // { task_id, model, category, fallback_model }. Handler stores it so the UI
  // can offer a one-click retry on the fallback model (e.g. Opus 4.8).
  'agent-refusal',
  // Deterministic provider 4xx (bad request the provider will always reject).
  // Payload: { task_id, error }. Handler stores it so the UI can offer a
  // one-click "Repair & continue" that stubs the offending history content.
  'agent-provider-error',
  // Per-turn checkpoint anchor. Fires once per send_message once the
  // file-history snapshot has been captured. The handler tags the originating
  // user message with the snapshot_message_id.
  'agent-turn-started',
];

function safeInvoke(cmd, args) {
  try {
    return invoke(cmd, args);
  } catch (e) {
    return Promise.reject(e);
  }
}

// --- Streaming delta batching ----------------------------------------------
// Providers emit text/thinking deltas token-by-token; applying each one via a
// zustand `set` made every subscriber (chat-view's transcript useMemos,
// chat-turn's marked.parse + DOMPurify over the whole accumulated block, …)
// re-run per token. Instead, deltas are accumulated here and flushed to the
// store on a short trailing timer (~30-50ms ≈ still every frame or two, so
// streaming looks identical). Ordering is preserved by flushing eagerly
// whenever the delta KIND changes (text ↔ thinking) and before any event that
// appends other block types (tool_use, tool_result, ask_user, done markers) —
// see the flushPendingDeltas calls inside the store actions below. The timer
// always fires even without an explicit flush, so no text is ever lost on
// stream end, cancel, or error.
const DELTA_FLUSH_MS = 40;
const pendingDeltas = new Map(); // taskId -> { kind: 'text' | 'thinking', buf, timer }

function flushPendingDeltas(taskId) {
  const p = pendingDeltas.get(taskId);
  if (!p) return;
  pendingDeltas.delete(taskId);
  if (p.timer) clearTimeout(p.timer);
  if (!p.buf) return;
  const s = useAgent.getState();
  if (p.kind === 'thinking') s.appendThinking(taskId, p.buf);
  else s.appendAssistantText(taskId, p.buf);
}

function queueStreamDelta(taskId, kind, delta) {
  if (!taskId || !delta) return;
  const existing = pendingDeltas.get(taskId);
  // A kind switch (thinking → text or back) must not be coalesced across —
  // flush the old buffer first so block ordering in the transcript is exact.
  if (existing && existing.kind !== kind) flushPendingDeltas(taskId);
  let p = pendingDeltas.get(taskId);
  if (!p) {
    p = { kind, buf: '', timer: null };
    p.timer = setTimeout(() => flushPendingDeltas(taskId), DELTA_FLUSH_MS);
    pendingDeltas.set(taskId, p);
  }
  p.buf += delta;
}

// Persist the user's last model pick across restarts. Stored as a single JSON
// blob so we can grow the schema later without churning two keys in lockstep.
const MODEL_PICK_KEY = 'rustic.agent.selectedModel';
const THINKING_TIER_KEY = 'rustic.agent.thinkingTier';
const PERMISSION_LEVEL_KEY = 'rustic.agent.permissionLevel';
// Per-task model-change history. The backend doesn't persist per-turn model
// metadata, so we keep our own localStorage record of (a) every mid-chat model/
// effort switch as a divider marker, and (b) the model the most recent turn was
// sent with, used to detect the next change. Both are keyed by taskId.
const MODEL_MARKERS_KEY = 'rustic.agent.modelMarkers';
const LAST_TURN_MODEL_KEY = 'rustic.agent.lastTurnModel';
// The FreeBuff model the most recent FreeBuff turn was sent with (process-global,
// not per-task — FreeBuff binds one model per token). Used to detect when a send
// will switch models and thus spin up a NEW session, which costs one of the
// limited tier's ~5 requests/model/24h.
const LAST_FREEBUFF_MODEL_KEY = 'rustic.agent.lastFreebuffModel';

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

// Generic JSON-map load/save for the per-task model-change records. Both keys
// hold a `{ [taskId]: ... }` object; we tolerate any parse failure by falling
// back to an empty map so a corrupted entry never blocks the chat from loading.
function loadJsonMap(key) {
  if (typeof window === 'undefined' || !window.localStorage) return {};
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch {
    return {};
  }
}

function saveJsonMap(key, value) {
  if (typeof window === 'undefined' || !window.localStorage) return;
  try {
    window.localStorage.setItem(key, JSON.stringify(value || {}));
  } catch {}
}

const PERSISTED_MODEL_PICK = loadPersistedModelPick();
const PERSISTED_THINKING_TIER = loadPersistedScalar(THINKING_TIER_KEY, VALID_THINKING_TIERS);
const PERSISTED_PERMISSION_LEVEL = loadPersistedScalar(PERMISSION_LEVEL_KEY, VALID_PERMISSION_LEVELS);

// Map a user-facing thinking tier to a token budget for the backend. These
// are conservative defaults — backend can clamp to model-specific maxima.
// 'off' returns 0 so the backend explicitly disables thinking.
export function thinkingTierToBudget(tier) {
  switch (tier) {
    case 'off':    return 0;  // Explicitly disable thinking
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
  if (id.includes('fable') || id.includes('mythos')) return ['off', 'low', 'medium', 'high', 'max'];
  if (id.includes('opus'))    return ['off', 'low', 'medium', 'high', 'max'];
  if (id.includes('sonnet'))  return ['off', 'low', 'medium', 'high'];
  if (id.includes('haiku'))   return ['off', 'low'];
  if (id.includes('gpt-5.6-sol')) return ['off', 'low', 'medium', 'high', 'max'];
  if (id.includes('gpt-5'))   return ['off', 'low', 'medium', 'high'];
  if (id.includes('gemini'))  return ['off', 'low', 'medium', 'high'];
  // Fall back to the four-tier shape; backend will ignore unsupported budgets.
  return ['off', 'low', 'medium', 'high'];
}

// Best-effort human-readable name for a model id. Prefers the display name from
// the loaded `models` list; otherwise derives a readable label from the id.
export function modelDisplayName(modelId) {
  if (!modelId) return null;
  try {
    const models = useAgent.getState().models || [];
    const hit = models.find((m) => m.id === modelId || m.modelId === modelId);
    if (hit && (hit.name || hit.displayName)) return hit.name || hit.displayName;
  } catch { /* store may not be ready during module init */ }
  return modelId
    .replace(/^claude-/, 'Claude ')
    .replace(/^gpt-/, 'GPT-')
    .replace(/^gemini-/, 'Gemini ')
    .replace(/-/g, ' ')
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

// Friendly label for a Fable 5 refusal classifier category. Categories come
// straight from Anthropic's stop_details; unknown ones pass through as-is.
export function refusalCategoryLabel(category) {
  if (!category) return null;
  const c = String(category).toLowerCase();
  if (c.includes('cyber')) return 'offensive cybersecurity';
  if (c.includes('bio') || c.includes('chem')) return 'biology / chemistry';
  if (c.includes('reasoning') || c.includes('extraction') || c.includes('distill')) {
    return 'reasoning extraction';
  }
  if (c.includes('child') || c.includes('csam')) return 'child safety';
  return category;
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
// history so they don't render as a real exchange.
//
// Match by prefix across known variants rather than one exact string: the
// backend wording has changed over time (the memory-fragment split reworded
// it from "Memory loaded…" to "Memory index loaded…"), and tasks created
// before a rewording still carry the older ack persisted in their DB. A
// prefix check filters both the historical and current acks and is robust to
// future minor edits of the trailing sentence — the previous exact-match
// constant silently drifted out of sync and let the ack leak into the UI.
const MEMORY_INJECT_ACK_PREFIXES = [
  'Memory index loaded.',
  'Memory loaded.',
];

function isMemoryInjectAck(text) {
  if (typeof text !== 'string') return false;
  return MEMORY_INJECT_ACK_PREFIXES.some((p) => text.startsWith(p));
}

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
      if (onlyText && isMemoryInjectAck(rawContent[0].text)) continue;
    }

    // Pull image content blocks out of user messages and surface them as a
    // separate `attachments` array — chat-turn.jsx renders attachments via
    // <ImageAttachment> while content blocks would otherwise show up as raw
    // unhandled JSON. Without this lift, images attached to past messages
    // disappear from the chat on reload (the data is in the DB, but nothing
    // renders it).
    let attachments = [];
    let content = rawContent;
    if (m.role === 'user') {
      const textParts = [];
      const passthrough = [];
      for (const b of rawContent) {
        if (!b || typeof b !== 'object') {
          passthrough.push(b);
          continue;
        }
        if (b.type === 'image' && typeof b.data === 'string' && b.data.length > 0) {
          const mediaType = b.media_type || b.mediaType || 'image/png';
          attachments.push({
            id: `hist-att-${taskId}-${idx}-${attachments.length}`,
            name: b.name || `image-${attachments.length + 1}`,
            url: `data:${mediaType};base64,${b.data}`,
            mediaType,
            // Carry the raw base64 forward so a chat+files revert can hand the
            // attachment to PromptBox, which will pass it back to send_message
            // intact instead of having to re-encode from the data URL.
            base64Data: b.data,
          });
          continue;
        }
        if (b.type === 'text') {
          if (isSyntheticInjection(b.text || '')) continue;
          textParts.push(b);
          continue;
        }
        // `model_switch` blocks are UI-only markers the backend persists into
        // the transcript when the model is changed mid-task (switch_model in
        // runtime.rs). They're stripped before the API ever sees them, and the
        // live chat draws its "switched to X" rule from the separate
        // modelMarkers store — so on reload they carry no renderable content
        // and would otherwise surface as an empty user bubble. Drop them, and
        // skip them from the user-turn count so divider anchoring stays aligned.
        if (b.type === 'model_switch') continue;
        passthrough.push(b);
      }
      content = [...textParts, ...passthrough];
      if (content.length === 0 && attachments.length === 0) continue;
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
      ...(attachments.length > 0 ? { attachments } : {}),
      ...(m.turn_usage ? { turnUsage: m.turn_usage } : {}),
      // Carry the original backend index forward so the per-message Revert
      // button can pass the correct `keepCount` to `truncate_task_messages`
      // after a reload. The DB's `sort_order` matches `task.messages` indexes
      // including synthetic injections, so this stays accurate even when the
      // normalized frontend list is shorter than the persisted backend list.
      ...(typeof m.sort_order === 'number' ? { sortOrder: m.sort_order } : {}),
    });
  }
  return out;
}

// Annotate user messages with their file-history snapshot ids after a task is
// loaded from disk. Without this, hydrated chats lose their Revert buttons
// (the `agent-turn-started` event that normally tags them only fires for
// live turns, not for reloads). Snapshots are paired with user messages by
// position: the N-th snapshot (sorted by `sequence`) belongs to the N-th
// non-tool, non-synthetic user message in the loaded list — that's the same
// ordering the executor uses when it opens snapshots.
function applySnapshotAnchors(messages, snapshots) {
  if (!Array.isArray(messages) || !Array.isArray(snapshots) || snapshots.length === 0) {
    // eslint-disable-next-line no-console
    console.warn('[applySnapshotAnchors] No snapshots to apply', {
      messagesIsArray: Array.isArray(messages),
      messagesCount: Array.isArray(messages) ? messages.length : 0,
      snapshotsIsArray: Array.isArray(snapshots),
      snapshotsCount: Array.isArray(snapshots) ? snapshots.length : 0,
    });
    return messages;
  }
  // Snapshots arrive sorted by sequence from `fh_list_snapshots` already, but
  // sort defensively in case the backend changes its ordering — we'd rather
  // recover gracefully than mis-anchor.
  const sorted = [...snapshots].sort((a, b) => (a.sequence || 0) - (b.sequence || 0));
  let snapIdx = 0;
  const result = messages.map((m) => {
    if (snapIdx >= sorted.length) return m;
    // Only count real user turns — the same condition that the executor
    // uses when deciding to open a snapshot in `send_message`.
    if (m.role !== 'user') return m;
    const hasRealText = (m.content || []).some(
      (b) => b && b.type === 'text' && (b.text || '').trim().length > 0,
    );
    if (!hasRealText) return m;
    const snap = sorted[snapIdx++];
    return {
      ...m,
      snapshotMessageId: snap.message_id,
      // sortOrder was attached by normalizeLoadedMessages from the DTO's
      // `sort_order` (the row's position in the backend's task.messages list).
      // That's exactly what `truncate_task_messages` expects as `keepCount`.
      userMessageIndex: typeof m.sortOrder === 'number' ? m.sortOrder : undefined,
    };
  });
  
  // eslint-disable-next-line no-console
  console.log('[applySnapshotAnchors] Applied snapshots', {
    messagesCount: messages.length,
    snapshotsCount: snapshots.length,
    userMessagesWithSnapshots: result.filter(m => m.role === 'user' && m.snapshotMessageId).length,
    userMessagesTotal: result.filter(m => m.role === 'user').length,
  });

  return result;
}

// Convert a persisted SubagentRecord (from `get_subagent_records`) into the
// SubagentLive shape that `subagentsByTask` holds. Used on task open so the
// SubagentInlineView can replay the sub-agent's run after a restart.
//
// Known reconstruction limitation: `output_text` is a single accumulated
// blob of every text-delta the sub-agent emitted, and `tool_calls_json` is a
// flat list of tool_use+tool_result pairs in arrival order. We don't keep the
// precise interleaving of text vs. tool calls — so the rebuilt transcript
// shows the assistant's accumulated text first, then all tool calls, then
// the closing summary. Reading order is preserved, but the visual timeline
// won't perfectly mirror what the user saw while it was streaming live. Live
// streams (still in subagentsByTask before reload) keep the exact ordering.
function subagentRecordToLive(record) {
  const tsCreated = Date.parse(record.created_at) || Date.now();
  const tsUpdated = Date.parse(record.updated_at) || tsCreated;
  const messages = [];

  if (record.prompt) {
    messages.push({
      id: `sub-user-${record.agent_id}`,
      role: 'user',
      content: [{ type: 'text', text: record.prompt }],
      timestamp: tsCreated,
    });
  }

  if (record.output_text) {
    messages.push({
      id: `sub-assist-text-${record.agent_id}`,
      role: 'assistant',
      content: [{ type: 'text', text: record.output_text }],
      timestamp: tsCreated,
    });
  }

  let toolCalls = [];
  if (record.tool_calls_json) {
    try {
      const parsed = JSON.parse(record.tool_calls_json);
      if (Array.isArray(parsed)) toolCalls = parsed;
    } catch {
      // Corrupted JSON — skip the tool-call replay rather than blow up the
      // whole hydration. The text + summary still render.
    }
  }
  for (const tc of toolCalls) {
    messages.push({
      id: `sub-tool-${tc.tool_use_id}`,
      role: 'assistant',
      content: [
        {
          type: 'tool_use',
          id: tc.tool_use_id,
          name: tc.tool_name,
          input: tc.input,
        },
      ],
      timestamp: tsUpdated,
    });
    if (tc.result !== null && tc.result !== undefined) {
      messages.push({
        id: `sub-tool-result-${tc.tool_use_id}`,
        role: 'tool',
        content: [
          {
            type: 'tool_result',
            tool_use_id: tc.tool_use_id,
            output: tc.result,
            is_error: !!tc.is_error,
          },
        ],
        timestamp: tsUpdated,
      });
    }
  }

  if (record.summary) {
    messages.push({
      id: `sub-final-${record.agent_id}`,
      role: 'assistant',
      content: [{ type: 'text', text: record.summary }],
      timestamp: tsUpdated,
    });
  } else if (record.error) {
    messages.push({
      id: `sub-error-${record.agent_id}`,
      role: 'assistant',
      content: [{ type: 'text', text: `**Failed:** ${record.error}` }],
      timestamp: tsUpdated,
    });
  }

  const hasCost =
    (record.cost_usd || 0) > 0 ||
    (record.input_tokens || 0) > 0 ||
    (record.output_tokens || 0) > 0;

  return {
    agentId: record.agent_id,
    model: record.model || '',
    prompt: record.prompt || '',
    name: record.name || '',
    status: record.status || 'completed',
    summary: record.summary || '',
    error: record.error || '',
    cost: hasCost
      ? {
          // Match the TaskCost field names — CostIndicator reads
          // `total_input_tokens` / `total_output_tokens` etc., so we map the
          // record's per-token columns into that shape rather than passing
          // them through unchanged.
          total_input_tokens: record.input_tokens || 0,
          total_output_tokens: record.output_tokens || 0,
          total_cache_read_tokens: record.cache_read_tokens || 0,
          estimated_cost_usd: record.cost_usd || 0,
        }
      : null,
    messages,
    createdAt: tsCreated,
    lastUpdate: tsUpdated,
  };
}

export const useAgent = create((set, get) => ({
  activeProject: { id: '', name: '', root: '' },
  tasks: [],
  // tasksByProject: { [projectId]: Task[] } — persistent cache for the task
  // tree on the left in agent mode. Survives project switches so the tree
  // doesn't have to refetch every time the user clicks around.
  tasksByProject: {},
  // goalByTask: { [taskId]: { condition, status, turns, reason } } — live
  // /goal loop state. status: active | continuing | evaluating | unmet |
  // error. Entries are removed when the goal is met or cleared.
  goalByTask: {},
  // expandedProjects: { [projectId]: true } — which project nodes are open
  // in the agent task tree. Defaults to expanded for the active project.
  expandedProjects: { left: {}, right: {} },
  // historyLimitByProject: { [projectId]: number } — paginated "Load more"
  // counter for non-running tasks. Defaults to 5.
  historyLimitByProject: {},
  activeTaskId: null,
  // When a user reverts a chat to a checkpoint, we seed the prompt box with
  // the original message + attachments so the user can edit and resend
  // without retyping. Shape: { taskId, text, attachments } or null.
  // PromptBox watches this slot and clears it after applying.
  pendingDraft: null,
  promptDrafts: {},
  messagesByTask: {},
  // modelMarkersByTask: { [taskId]: Marker[] }, Marker = { id, turnIndex,
  // provider, modelId, thinkingTier }. Each marker renders as a divider in the
  // chat just before the user-turn at `turnIndex`, showing which model/effort
  // the conversation switched to. Persisted to localStorage (survives reload);
  // anchored by user-turn index because live and reloaded message ids differ.
  modelMarkersByTask: loadJsonMap(MODEL_MARKERS_KEY),
  // lastTurnModelByTask: { [taskId]: { provider, modelId, thinkingTier } }. The
  // model the most recent sent turn used, so the next send can tell whether the
  // model/effort changed and a divider is warranted. Persisted alongside the
  // markers so the comparison survives a reload.
  lastTurnModelByTask: loadJsonMap(LAST_TURN_MODEL_KEY),
  // The FreeBuff model the last FreeBuff turn ran on (global). `null` until the
  // first FreeBuff send. Drives the "switching models spends a request" warning.
  lastFreebuffModel:
    (typeof window !== 'undefined' && window.localStorage.getItem(LAST_FREEBUFF_MODEL_KEY)) || null,
  todosByTask: {},
  costByTask: {},
  // Per-task live context usage from the last provider request. Shape:
  // { tokens, contextWindow, threshold } where `tokens` is the prompt size of
  // the most recent API call (input + cache read + cache write) and
  // `threshold` is where auto-condense triggers. Fed by 'agent-request-usage'.
  contextUsageByTask: {},
  statusByTask: {},
  streamingByTask: {},
  // Per-task condensing state. Set when context condensing starts and cleared
  // when it completes. Shape: { original_messages, condensed_to } or null.
  // The UI shows a "Compacting context..." indicator while this is set.
  condensingByTask: {},
  // Per-task queued message. When the user sends a message while condensing
  // is active, we store it here and auto-send after condensing completes.
  // Shape: { text, attachments, thinkingBudget } or null.
  queuedMessageByTask: {},
  // Per-task retry state. Set when the executor emits agent-stream-retry
  // (rate-limit, network blip, stalled stream, etc.) and cleared when the
  // next stream chunk arrives or the task ends. Shape:
  //   { attempt, max_attempts, waiting_ms, error, started_at_ms }
  // The UI renders a countdown banner above the prompt box while this is
  // set so the user knows the agent isn't frozen — it's just waiting.
  retryByTask: {},
  // Per-task refusal state. Set when a Claude Fable 5-class safety classifier
  // declines the request (agent-refusal). Shape:
  //   { model, category, fallback_model } or null.
  // The UI renders a dialog offering to retry on the fallback model.
  refusalByTask: {},
  // Per-task deterministic provider error (4xx). Set by agent-provider-error
  // when a turn fails with a request the provider permanently rejects.
  // Shape: { error, at } or absent. The UI renders a "Repair & continue"
  // banner; cleared on the next send or successful repair.
  providerErrorByTask: {},
  // Buffer of in-progress tool_use input JSON during streaming. The provider
  // emits tool-use-input-delta fragments which we concatenate here keyed by
  // tool_use_id. On each delta we attempt a tolerant JSON.parse — when it
  // succeeds (rare mid-stream, but free) the parsed object is mirrored
  // onto the message's tool_use block so the user sees the input fill in
  // live. Cleared when tool-use-stop fires (or on the canonical tool-use
  // event that follows from the executor with the authoritative parse).
  streamingToolInputs: {}, // tool_use_id -> raw partial JSON string
  // Per-task accumulated set of files the agent has touched. Live updates
  // arrive via agent-file-tracked (just paths, no stats yet); the richer
  // hydration call (fh_list_task_net_changes) populates the full per-entry
  // stats when the task is opened. Shape:
  //   { [taskId]: {
  //       entries: [
  //         { path, kind, binary, additions, deletions,
  //           anchor_message_id, is_dir }
  //       ],
  //       lastMessageId: string|null,
  //     }
  //   }
  // Entries are deduped on `path`; first-seen order otherwise. Live
  // updates arrive with kind='modified', additions=0, deletions=0,
  // is_dir=false, anchor_message_id=event.message_id — the next
  // background refresh corrects these to the real per-file stats.
  filesByTask: {},
  // Sensitive-file access: global persisted toggle; frontend is the source
  // of truth and pushes it per task on change and before every send.
  sensitiveAccess:
    typeof window !== 'undefined' && window.localStorage.getItem('rustic.agent.sensitiveAccess') === '1',
  setSensitiveAccess: (v) => {
    try {
      window.localStorage.setItem('rustic.agent.sensitiveAccess', v ? '1' : '0');
    } catch {}
    set({ sensitiveAccess: !!v });
    get().pushSensitiveAccess(get().activeTaskId);
  },
  pushSensitiveAccess(taskId) {
    if (!taskId || !isTauriAvailable()) return;
    if (taskId.startsWith('local-') || taskId.startsWith('mock-')) return;
    Promise.resolve(
      safeInvoke('set_task_sensitive_access', { taskId, allowed: get().sensitiveAccess }),
    ).catch(() => {});
  },
  // agent-tool-dock state lifted out of the component so the user's tab
  // selection survives across chat-view remounts. The dock unmounts /
  // remounts whenever the editor area shifts (e.g. opening a diff or a
  // terminal from one of its tabs), which used to reset both fields to
  // their defaults — so clicking a file in the Files tab would
  // immediately bounce the dock back to Plan.
  //   dockActiveByTask:   { [taskId]: 'plan' | 'files' | 'terminals' | null }
  //   dockAutoOpenedByTask: { [taskId]: true }
  dockActiveByTask: {},
  dockAutoOpenedByTask: {},
  setDockActiveTab: (taskId, val) => {
    if (!taskId) return;
    set((s) => ({
      dockActiveByTask: { ...s.dockActiveByTask, [taskId]: val },
    }));
  },
  markDockAutoOpened: (taskId) => {
    if (!taskId) return;
    set((s) => ({
      dockAutoOpenedByTask: { ...s.dockAutoOpenedByTask, [taskId]: true },
    }));
  },
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
  // Per-task "always allow" tool grants from the permission prompt's checkbox.
  // Shape: { [taskId]: { [toolName]: true } }. Session-only by design — a
  // grant should not outlive the task it was given for.
  permissionAllowlistByTask: {},
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

  setPendingDraft: (draft) => set({ pendingDraft: draft }),
  clearPendingDraft: () => set({ pendingDraft: null }),

  // Session-only composer drafts keyed `task:<id>` / `new:<projectId>` so the
  // prompt box survives remounts when the chat panel moves between layout
  // slots (hero ↔ main ↔ side dock) and across task/project switches.
  setPromptDraft: (key, draft) =>
    set((s) => ({ promptDrafts: { ...s.promptDrafts, [key]: draft } })),
  clearPromptDraft: (key) =>
    set((s) => {
      if (!(key in s.promptDrafts)) return {};
      const next = { ...s.promptDrafts };
      delete next[key];
      return { promptDrafts: next };
    }),

  setActiveTask: (taskId) => {
    set({ activeTaskId: taskId });
    // Fire-and-forget: hydrate chat history, todos, cost, and sub-agent
    // records from the backend. Skips work if we already have a live
    // in-memory transcript for this task (active stream is authoritative).
    if (taskId) {
      get()._hydrateTaskModelPick(taskId);
      get().loadTaskHistory(taskId).catch((e) => {
        // eslint-disable-next-line no-console
        console.error('[agent.setActiveTask] loadTaskHistory failed', { taskId, error: e });
      });
    }
  },

  _patchTaskRecord(taskId, patch) {
    set((s) => {
      const apply = (list) => list.map((t) => (t.id === taskId ? { ...t, ...patch } : t));
      const out = {};
      if (s.tasks.some((t) => t.id === taskId)) out.tasks = apply(s.tasks);
      const nextByProject = { ...s.tasksByProject };
      let touched = false;
      for (const [pid, list] of Object.entries(s.tasksByProject)) {
        if (list.some((t) => t.id === taskId)) {
          nextByProject[pid] = apply(list);
          touched = true;
        }
      }
      if (touched) out.tasksByProject = nextByProject;
      return out;
    });
  },

  _hydrateTaskModelPick(taskId) {
    const s = get();
    const findIn = (list) => (list || []).find((t) => t.id === taskId);
    const task = findIn(s.tasks) || findIn(Object.values(s.tasksByProject).flat());
    if (!task) return;
    if (
      task.provider_type &&
      task.model &&
      (task.provider_type !== s.selectedProvider || task.model !== s.selectedModel)
    ) {
      persistModelPick(task.provider_type, task.model);
      set({ selectedProvider: task.provider_type, selectedModel: task.model });
    }
    const tier = task.thinking_tier;
    if (tier && VALID_THINKING_TIERS.has(tier) && tier !== s.thinkingTier) {
      persistScalar(THINKING_TIER_KEY, tier);
      set({ thinkingTier: tier });
    }
  },

  setActiveProject: (project) => {
    const next = project ?? { id: '', name: '', root: '' };
    const prev = get().activeProject;
    if (prev.id === next.id) return;

    console.log('[agent.setActiveProject] switching projects', {
      from: prev.id,
      to: next.id,
      preservingTasks: Object.keys(get().messagesByTask),
    });

    // Preserve state for tasks in both the previous AND next project when switching.
    // This keeps the chat history in memory so we don't have to reload from DB.
    const preserveTaskIds = new Set();
    for (const [projId, tasks] of Object.entries(get().tasksByProject)) {
      if (projId === prev.id || projId === next.id) {
        for (const task of tasks) {
          preserveTaskIds.add(task.id);
        }
      }
    }

    // CRITICAL: never drop a task that's still live (streaming or running),
    // regardless of which project it belongs to. A background task's in-memory
    // transcript is the authoritative copy — its in-flight messages haven't
    // been committed to the DB yet, so filtering it out here permanently loses
    // history. Worse, the stream keeps firing events for the dropped task, so
    // they append to a freshly-emptied array and only the post-switch activity
    // survives (the "background chat lost its history" bug).
    const { streamingByTask, statusByTask } = get();
    for (const taskId of Object.keys(streamingByTask)) {
      if (streamingByTask[taskId]) preserveTaskIds.add(taskId);
    }
    for (const taskId of Object.keys(statusByTask)) {
      if (statusByTask[taskId] === 'running') preserveTaskIds.add(taskId);
    }

    const filterState = (stateMap) => {
      const filtered = {};
      for (const taskId of preserveTaskIds) {
        if (stateMap[taskId] !== undefined) {
          filtered[taskId] = stateMap[taskId];
        }
      }
      return filtered;
    };

    console.log('[agent.setActiveProject] preserving state for tasks', {
      preservedTaskIds: Array.from(preserveTaskIds),
      messagesKept: Object.keys(filterState(get().messagesByTask)),
    });

    set((s) => ({
      activeProject: next,
      // Mirror the cached tasks for this project into the flat `tasks` field
      // so the existing chat/task-switcher selectors keep working unchanged.
      tasks: s.tasksByProject[next.id] || [],
      activeTaskId: null,
      // Preserve state for tasks in other projects (not prev or next),
      // especially if they're still running. Only clear state for the
      // project we're switching away from and the one we're switching to
      // (which will reload fresh from DB when a task is opened).
      messagesByTask: filterState(s.messagesByTask),
      todosByTask: filterState(s.todosByTask),
      costByTask: filterState(s.costByTask),
      contextUsageByTask: filterState(s.contextUsageByTask),
      statusByTask: filterState(s.statusByTask),
      streamingByTask: filterState(s.streamingByTask),
      subagentRecordsByTask: filterState(s.subagentRecordsByTask),
      subagentsByTask: filterState(s.subagentsByTask),
      openSubagent: null,
      historyLoadedByTask: filterState(s.historyLoadedByTask),
      initialized: false,
      // Preserve each project's expand/collapse state across switches. Projects
      // are collapsed by default (undefined === collapsed), like the Explorer,
      // so switching to a project no longer force-expands its tasks.
      expandedProjects: s.expandedProjects,
    }));
  },

  toggleProjectExpanded: (side, projectId) =>
    set((s) => ({
      expandedProjects: {
        ...s.expandedProjects,
        [side]: {
          ...s.expandedProjects[side],
          [projectId]: !s.expandedProjects[side]?.[projectId],
        },
      },
    })),

  setProjectExpanded: (side, projectId, expanded) =>
    set((s) => ({
      expandedProjects: {
        ...s.expandedProjects,
        [side]: { ...s.expandedProjects[side], [projectId]: !!expanded },
      },
    })),

  collapseAllProjects: (side) =>
    set((s) => ({ expandedProjects: { ...s.expandedProjects, [side]: {} } })),

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

  // Sticky-note pin: optimistically flips the task's pinned flag in the cache
  // (the tree re-sorts immediately), then persists via the backend. Rolls the
  // flag back if the invoke fails.
  setTaskPinned: async (projectId, taskId, pinned) => {
    get().upsertTaskInCache(projectId, { id: taskId, pinned });
    try {
      await safeInvoke('set_task_pinned', { taskId, pinned });
    } catch (e) {
      get().upsertTaskInCache(projectId, { id: taskId, pinned: !pinned });
      throw e;
    }
  },

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

  // Anchor the most recent user message in this task to a file-history
  // checkpoint. The backend `agent-turn-started` event carries the
  // snapshot_message_id captured by `open_snapshot` and the index of the user
  // message in the backend's task.messages list — we stash both on the
  // matching user message so the chat UI can render its per-message Revert
  // button and pass the right keep_count to `truncate_task_messages`.
  anchorCheckpoint: (taskId, snapshotMessageId, userMessageIndex) => {
    if (!taskId || !snapshotMessageId) return;
    set((s) => {
      const list = s.messagesByTask[taskId];
      if (!list || list.length === 0) return s;
      // Find the most recent user message that doesn't already carry a
      // checkpoint. Walking from the end is correct because the snapshot is
      // emitted right after the user just sent — that user message is the tail
      // user entry.
      let lastUserIdx = -1;
      for (let i = list.length - 1; i >= 0; i--) {
        if (list[i].role === 'user') {
          lastUserIdx = i;
          break;
        }
      }
      if (lastUserIdx < 0) return s;
      const next = list.slice();
      next[lastUserIdx] = {
        ...next[lastUserIdx],
        snapshotMessageId,
        userMessageIndex,
      };
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: next },
      };
    });
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
        // Skip the identity churn when the flag is already true — otherwise
        // every delta re-renders every subscriber of the whole map.
        ...(s.streamingByTask[taskId] === true
          ? {}
          : { streamingByTask: { ...s.streamingByTask, [taskId]: true } }),
      };
    });
  },

  appendThinking: (taskId, delta) => {
    // Push thinking text as an inline content block on the assistant turn so
    // chat-turn.jsx can render it alongside the eventual response.
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
        // Same no-op-write skip as appendAssistantText.
        ...(s.streamingByTask[taskId] === true
          ? {}
          : { streamingByTask: { ...s.streamingByTask, [taskId]: true } }),
      };
    });
  },

  // Mark the most recent thinking block on a task as finalised. The backend
  // emits `agent-thinking-done` with the elapsed seconds once the model has
  // closed the thinking section; we stamp `done: true` + `durationSecs` so
  // chat-turn.jsx can flip from "Thinking…" to "Reasoned for Ns".
  markThinkingDone: (taskId, durationSecs) => {
    // Apply any buffered thinking deltas first so the block is complete
    // before it's stamped done.
    flushPendingDeltas(taskId);
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

  addToolUse: (taskId, toolUseId, name, input, streaming = false) => {
    // Buffered text/thinking must land before the tool_use block so the
    // transcript order matches what the model actually emitted.
    flushPendingDeltas(taskId);
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant') {
        last.streaming = false;
      }
      
      // Idempotent: if streaming already placed this tool_use into messages
      // (matched by id), update its input with the canonical, fully-parsed
      // value. Otherwise append fresh — covers non-streaming providers and
      // any case where streaming events were dropped/missed.
      const existingIdx = list.findIndex((m) => m.id === `tool-${toolUseId}`);
      if (existingIdx >= 0) {
        // Update in place with final parsed input
        list[existingIdx] = {
          ...list[existingIdx],
          content: [{ type: 'tool_use', id: toolUseId, name, input }],
          streaming: false,
        };
      } else {
        // Fresh tool_use
        list.push({
          id: `tool-${toolUseId}`,
          role: 'assistant',
          content: [{ type: 'tool_use', id: toolUseId, name, input }],
          timestamp: Date.now(),
          streaming,
        });
      }
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  addToolResult: (taskId, toolUseId, output, isError) => {
    flushPendingDeltas(taskId);
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

  // --- Streaming tool call helpers -------------------------------------------
  // These are called by the event handlers for agent-tool-use-start,
  // agent-tool-use-input-delta, and agent-tool-use-stop to build up the
  // tool_use block incrementally as the model streams it.

  appendToolUse: (taskId, toolUseId, toolName, toolInput, streaming) => {
    const { addToolUse } = get();
    addToolUse(taskId, toolUseId, toolName, toolInput, streaming);
  },

  accumulateToolInputDelta: (taskId, toolUseId, partialJson) => {
    set((s) => {
      const buffer = s.streamingToolInputs[toolUseId] || '';
      const updated = buffer + partialJson;
      
      // Optimistic parse: if it succeeds, update the message's tool_use block
      // immediately so the user sees the input fill in live.
      let parsed = null;
      try {
        parsed = JSON.parse(updated);
      } catch {
        // Incomplete JSON, leave it buffered
      }
      
      if (parsed) {
        // Update the tool_use message in place
        const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
        const idx = list.findIndex((m) => m.id === `tool-${toolUseId}`);
        if (idx >= 0) {
          const msg = list[idx];
          const block = msg.content[0];
          if (block && block.type === 'tool_use') {
            list[idx] = {
              ...msg,
              content: [{ ...block, input: parsed }],
            };
          }
        }
        return {
          streamingToolInputs: { ...s.streamingToolInputs, [toolUseId]: updated },
          messagesByTask: { ...s.messagesByTask, [taskId]: list },
        };
      }
      
      return {
        streamingToolInputs: { ...s.streamingToolInputs, [toolUseId]: updated },
      };
    });
  },

  finalizeToolInputStreaming: (taskId, toolUseId) => {
    set((s) => {
      const next = { ...s.streamingToolInputs };
      delete next[toolUseId];
      
      // Mark the tool_use message as no longer streaming
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const idx = list.findIndex((m) => m.id === `tool-${toolUseId}`);
      if (idx >= 0) {
        list[idx] = { ...list[idx], streaming: false };
      }
      
      return {
        streamingToolInputs: next,
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
      };
    });
  },

  // Finalize any dangling streaming/animation state for a task WITHOUT
  // clearing the `streamingByTask` flag. Used when a new user message is sent
  // mid-generation: the previous run is cancelled on the backend, but its
  // in-flight "Thinking…"/"Preparing…" indicators would otherwise spin
  // forever because no `agent-thinking-done`/`agent-task-complete` event ever
  // arrives for the abandoned run. We close every open thinking block and
  // stop every streaming message so the prior turn renders as settled.
  settleStreamAnimations: (taskId) => {
    // Land any buffered deltas before settling so the abandoned turn keeps
    // every token it actually received.
    flushPendingDeltas(taskId);
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      let touched = false;
      const nextList = list.map((m) => {
        const content = m.content || [];
        let blockTouched = false;
        const nextContent = content.map((b) =>
          b && b.type === 'thinking' && !b.done
            ? ((blockTouched = true), { ...b, done: true })
            : b,
        );
        if (m.streaming || blockTouched) {
          touched = true;
          return { ...m, streaming: false, content: nextContent };
        }
        return m;
      });
      if (!touched) return s;
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: nextList },
      };
    });
  },

  finishStream: (taskId) => {
    // Stream is over — apply whatever is still buffered so no trailing text
    // is lost, then mark the message settled.
    flushPendingDeltas(taskId);
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.streaming) {
        list[list.length - 1] = { ...last, streaming: false };
      }
      // Clear any pending retry banner — the stream finished one way or
      // another (success, error, or cancel), so a stale "retrying in 60s"
      // shouldn't keep showing.
      const nextRetry = { ...s.retryByTask };
      delete nextRetry[taskId];
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: false },
        retryByTask: nextRetry,
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
            name: '',
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

  // Toggle the sub-agent view inside ChatView. Single-level only — main agent
  // is the only spawner, so back always returns to the main chat. Setting
  // null exits the sub-agent view and restores the main chat.
  openSubagentView: (taskId, agentId) =>
    set({ openSubagent: taskId && agentId ? { taskId, agentId } : null }),
  closeSubagentView: () => set({ openSubagent: null }),

  setCost: (taskId, cost) =>
    set((s) => ({ costByTask: { ...s.costByTask, [taskId]: cost } })),

  setStatus: (taskId, status) => {
    // Terminal statuses can arrive with deltas still buffered (cancel/error
    // paths never emit agent-task-complete) — apply them first.
    flushPendingDeltas(taskId);
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
      // Parked on sub-agents: the turn is over (stop button + timer clear);
      // the backend auto-resumes the task when a child reports back.
      'waitingonsubagents',
    ]);
    const isTerminal = TERMINAL.has(String(status).toLowerCase());
    set((s) => {
      const patch = { statusByTask: { ...s.statusByTask, [taskId]: status } };
      if (isTerminal && s.streamingByTask[taskId] !== false) {
        patch.streamingByTask = { ...s.streamingByTask, [taskId]: false };
      }
      return patch;
    });
    // A terminal status while the condensing flag is still set means the
    // condense-completed event was lost (run cancelled / superseded mid-
    // condense). Without this, the stuck flag makes every subsequent send
    // queue forever instead of reaching the backend.
    if (isTerminal && get().condensingByTask[taskId]) {
      get()._flushCondenseQueue(taskId);
    }
  },

  // Clears the condensing flag and auto-sends any message the user queued
  // while compacting was active. Shared by the condense-completed handler and
  // the terminal-status fallback above.
  _flushCondenseQueue(taskId) {
    set((s) => {
      const next = { ...s.condensingByTask };
      delete next[taskId];
      return { condensingByTask: next };
    });
    const queued = get().queuedMessageByTask[taskId];
    if (!queued) return;
    set((s) => {
      const next = { ...s.queuedMessageByTask };
      delete next[taskId];
      return { queuedMessageByTask: next };
    });
    // eslint-disable-next-line no-console
    console.log('[agent] Sending queued message after condensing');
    get()._sendMessageDirect(taskId, queued.text, queued.attachments, queued.thinkingBudget, queued.extras || {});
  },

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

  openPermission: (req) => {
    const allowed =
      req?.task_id &&
      req?.operation &&
      get().permissionAllowlistByTask[req.task_id]?.[req.operation];
    if (allowed && isTauriAvailable()) {
      safeInvoke('respond_to_permission', {
        taskId: req.task_id,
        requestId: req.request_id,
        approved: true,
      }).catch(() => {});
      return;
    }
    set({ pendingPermission: req });
  },
  closePermission: () => set({ pendingPermission: null }),
  openQuestion: (req) => set({ pendingQuestion: req }),
  closeQuestion: () => set({ pendingQuestion: null }),

  // Append an `ask_user` block as a synthetic assistant message in the chat
  // for `taskId`. Replaces the old modal dialog flow — the inline chat
  // renderer (AskUserInline via chat-turn) reads the block and shows the
  // form there, so multiple concurrent task chats no longer collide on a
  // single global popup.
  appendAskUserBlock: (taskId, requestId, questions) => {
    if (!taskId || !requestId) return;
    // Any streamed preamble must render above the question block.
    flushPendingDeltas(taskId);
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      // De-dupe: if this request_id was already injected (event re-delivered,
      // hot-reload, etc.) don't append a second copy.
      const dupe = list.some((m) =>
        (m.content || []).some(
          (b) => b && b.type === 'ask_user' && b.request_id === requestId,
        ),
      );
      if (dupe) return s;
      // Close any open thinking/streaming on the prior assistant block so the
      // chat reads as "agent paused to ask" rather than "still thinking".
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        list[list.length - 1] = {
          ...last,
          streaming: false,
          content: (last.content || []).map((b) =>
            b && b.type === 'thinking' && !b.done ? { ...b, done: true } : b,
          ),
        };
      }
      list.push({
        id: `ask-${requestId}`,
        role: 'assistant',
        content: [
          {
            type: 'ask_user',
            request_id: requestId,
            questions: Array.isArray(questions) ? questions : [],
            answered: false,
            cancelled: false,
          },
        ],
        timestamp: Date.now(),
      });
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  setModels: (models) => set({ models }),
  setSelectedModel: (provider, modelId) => {
    persistModelPick(provider, modelId);
    set({ selectedProvider: provider, selectedModel: modelId });
    const taskId = get().activeTaskId;
    if (taskId) {
      get()._patchTaskRecord(taskId, { provider_type: provider, model: modelId });
    }
  },
  setThinkingTier: (tier) => {
    const next = tier || 'off';
    persistScalar(THINKING_TIER_KEY, next);
    set({ thinkingTier: next });
    const taskId = get().activeTaskId;
    if (taskId) {
      get()._patchTaskRecord(taskId, { thinking_tier: next });
      if (isTauriAvailable()) {
        safeInvoke('set_task_thinking_tier', { taskId, tier: next }).catch(() => {});
      }
    }
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
    // Reuse the active task — UNLESS it's a placeholder id that was never
    // created on the backend. A `local-`/`mock-` id under a live Tauri
    // runtime can never reach send_message (it rejects with "Task not
    // found: local-..."), so fall through and create a real task instead of
    // staying permanently stuck on the placeholder.
    if (
      state.activeTaskId &&
      !(isTauriAvailable() && /^(local|mock)-/.test(state.activeTaskId))
    ) {
      return state.activeTaskId;
    }
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
    if (!project?.id) {
      // No real project is selected. Fabricating a `local-` task here only
      // produces a doomed send (send_message → "Task not found: local-..."),
      // which is exactly the confusing failure this used to cause. Surface
      // the real problem and abort so the caller stops cleanly.
      toast.error('No project selected — pick a project before sending.');
      throw new Error('ensureTask: no active project');
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
      // Don't fall back to a fabricated local id — that just defers the
      // failure to send_message with a more confusing message. Surface the
      // real backend error and abort.
      // eslint-disable-next-line no-console
      console.error('[agent.ensureTask] create_task failed', { project, error: e });
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Couldn't create task: ${msg}`);
      throw e;
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

  async sendMessage(text, attachments = [], extras = {}) {
    const state = get();
    let taskId;
    try {
      taskId = await state.ensureTask();
    } catch (e) {
      // ensureTask already surfaced a toast explaining why (no project, or
      // create_task failed). Abort the send cleanly rather than appending a
      // user message into a task that doesn't exist on the backend.
      return;
    }
    if (!taskId) return;
    state.pushSensitiveAccess(taskId);

    // FreeBuff serves one model per token from a single process-global session.
    // Switching models ends the current session and starts a NEW one, which on
    // the free "limited" tier spends one of only ~5 requests per model per 24h —
    // and if other chats are mid-run on the old model, it tears their sessions
    // down too. Warn before either happens. We warn when this send would start a
    // new session (model changed) OR would disrupt a running chat; on confirm we
    // stop any conflicting chats first, on cancel nothing is sent.
    const fbConflicts = state._freebuffModelConflicts(taskId);
    const fbNewSession = state._freebuffWillStartNewSession();
    if (fbConflicts.length || fbNewSession) {
      const nextModel = get().selectedModel;
      const quotaLine =
        `FreeBuff starts a new session when you switch models, and the free "limited" tier allows ` +
        `only ~5 requests per model per 24h. Sending on ${nextModel} now will start a new session ` +
        `and use one of those requests.`;
      const conflictLine = fbConflicts.length
        ? `\n\nIt will also stop ${fbConflicts.length} other running chat` +
          `${fbConflicts.length > 1 ? 's' : ''} on a different FreeBuff model:\n` +
          fbConflicts.map((c) => `• ${c.title} (${c.modelId})`).join('\n')
        : '';
      const ok = await confirm({
        title: 'Switch FreeBuff model?',
        description: `${quotaLine}${conflictLine}`,
        confirmLabel: fbConflicts.length ? 'Stop them & continue' : 'Use a request & continue',
        cancelLabel: "Don't send",
        destructive: true,
      });
      if (!ok) return;
      await Promise.all(fbConflicts.map((c) => state._abortTaskById(c.id)));
    }

    // If condensing is active for this task, queue the message instead of
    // sending it. The condense-completed handler will auto-send it.
    if (get().condensingByTask[taskId]) {
      set((s) => ({
        queuedMessageByTask: {
          ...s.queuedMessageByTask,
          [taskId]: {
            text,
            attachments,
            extras,
            thinkingBudget: thinkingTierToBudget(state.thinkingTier),
          },
        },
      }));
      toast.info('Message queued — will send after context compacting completes');
      return;
    }

    // Delegate to the actual send logic
    await state._sendMessageDirect(taskId, text, attachments, thinkingTierToBudget(state.thinkingTier), extras);
  },

  // Set a /goal on the active task (creating one if needed): persists the
  // condition, then sends the kickoff message that starts the loop.
  async setGoal(condition) {
    const state = get();
    const trimmed = (condition || '').trim();
    if (!trimmed) return;
    let taskId;
    try {
      taskId = await state.ensureTask();
    } catch (e) {
      return;
    }
    if (!taskId) return;
    let kickoff;
    try {
      kickoff = await safeInvoke('set_task_goal', { taskId, condition: trimmed });
    } catch (e) {
      toast.error(`Could not set goal: ${typeof e === 'string' ? e : e?.message || String(e)}`);
      return;
    }
    set((s) => ({
      goalByTask: {
        ...s.goalByTask,
        [taskId]: { condition: trimmed, status: 'active', turns: 0, reason: null },
      },
    }));
    get()._patchTaskRecord(taskId, { goal: trimmed });
    if (kickoff) await get().sendMessage(kickoff);
  },

  // Clear the active /goal. Safe mid-run — the executor checks the shared
  // slot at the next turn boundary and simply stops looping.
  async clearGoal(taskId = get().activeTaskId) {
    if (!taskId) return;
    try {
      await safeInvoke('set_task_goal', { taskId, condition: null });
    } catch (e) {
      toast.error(`Could not clear goal: ${typeof e === 'string' ? e : e?.message || String(e)}`);
      return;
    }
    set((s) => {
      const next = { ...s.goalByTask };
      delete next[taskId];
      return { goalByTask: next };
    });
    get()._patchTaskRecord(taskId, { goal: null });
    toast.info('Goal cleared');
  },

  // Running FreeBuff chats (across every project) bound to a *different* model
  // than the one this task's next turn will use. FreeBuff serves one model per
  // token from a single shared session, so sending now would end those sessions
  // mid-run. Returns [] unless the pending turn itself targets FreeBuff — other
  // providers issue independent requests and aren't affected. A running chat on
  // the *same* FreeBuff model is fine: it shares the session, so it's excluded.
  _freebuffModelConflicts(taskId) {
    const s = get();
    if (s.selectedProvider !== 'FreeBuff') return [];
    const nextModel = s.selectedModel || null;
    if (!nextModel) return [];
    // Title lookup across the active project list + the cross-project cache, so
    // a conflicting chat in another project still shows a readable name.
    const titleById = new Map();
    for (const t of s.tasks) titleById.set(t.id, t.title);
    for (const list of Object.values(s.tasksByProject)) {
      for (const t of list) if (!titleById.has(t.id)) titleById.set(t.id, t.title);
    }
    const out = [];
    for (const [id, rec] of Object.entries(s.lastTurnModelByTask)) {
      if (id === taskId) continue;
      if (!rec || rec.provider !== 'FreeBuff') continue;
      if (rec.modelId === nextModel) continue; // same model shares the session
      // Mirror agent-task-tree's isRunning: trust the streaming flag first, then
      // fall back to the persisted backend status (covers projects not visited
      // this session, whose streaming flag may not be live).
      const status = s.statusByTask[id];
      const running =
        s.streamingByTask[id] === true ||
        status === 'streaming' ||
        status === 'running' ||
        status === 'working' ||
        status === 'preparing';
      if (!running) continue;
      out.push({ id, title: titleById.get(id) || 'Untitled chat', modelId: rec.modelId });
    }
    return out;
  },

  // True when sending now would make FreeBuff spin up a NEW session — i.e. the
  // pending turn targets FreeBuff on a *different* model than the last FreeBuff
  // turn used. FreeBuff binds one model per session, so a switch ends the old
  // session and creates a new one, and on the free "limited" tier a new session
  // spends one of only ~5 requests per model per 24h. First-ever FreeBuff send
  // (no prior model recorded) is not treated as a switch.
  _freebuffWillStartNewSession() {
    const s = get();
    if (s.selectedProvider !== 'FreeBuff') return false;
    const nextModel = s.selectedModel || null;
    if (!nextModel) return false;
    return !!s.lastFreebuffModel && s.lastFreebuffModel !== nextModel;
  },

  // Abort an arbitrary task by id (vs `abortActive`, which only targets the open
  // chat). Optimistically clears the streaming flag so any visible Stop button
  // flips back immediately; the backend still emits a terminal status that
  // setStatus reconciles.
  async _abortTaskById(taskId) {
    if (!taskId || !isTauriAvailable()) return;
    set((s) => {
      const nextRetry = { ...s.retryByTask };
      delete nextRetry[taskId];
      return {
        streamingByTask: { ...s.streamingByTask, [taskId]: false },
        retryByTask: nextRetry,
      };
    });
    try {
      await safeInvoke('abort_task', { taskId });
    } catch (e) {}
  },

  // Record which model + reasoning effort this task's next turn will run with,
  // and — when it differs from the previous turn's — drop a divider marker so
  // the chat shows a labelled "switched to X" rule between the two turns. Call
  // BEFORE appending the new user message: the marker is anchored to the
  // user-turn index this message is about to occupy, computed from the current
  // count of user messages in the transcript. No-op divider on the first turn
  // (nothing to switch from) or when nothing changed.
  _recordTurnModelChange(taskId) {
    const s = get();
    const modelId = s.selectedModel || null;
    if (!modelId) return; // no model selected yet — nothing to record
    const current = {
      provider: s.selectedProvider || null,
      modelId,
      thinkingTier: s.thinkingTier || 'off',
    };
    const prev = s.lastTurnModelByTask[taskId];
    const changed =
      !!prev &&
      (prev.provider !== current.provider ||
        prev.modelId !== current.modelId ||
        prev.thinkingTier !== current.thinkingTier);
    const list = s.messagesByTask[taskId] || [];
    const turnIndex = list.filter((m) => m.role === 'user').length;

    // Remember the FreeBuff model this turn runs on so the next send can tell
    // whether it's switching models (→ new session, spends a request).
    if (current.provider === 'FreeBuff' && modelId) {
      try {
        if (typeof window !== 'undefined') {
          window.localStorage.setItem(LAST_FREEBUFF_MODEL_KEY, modelId);
        }
      } catch { /* ignore quota/serialization errors */ }
    }

    set((st) => {
      const nextLast = { ...st.lastTurnModelByTask, [taskId]: current };
      saveJsonMap(LAST_TURN_MODEL_KEY, nextLast);
      const patch = {
        lastTurnModelByTask: nextLast,
        ...(current.provider === 'FreeBuff' && modelId
          ? { lastFreebuffModel: modelId }
          : {}),
      };
      // Only emit a divider for a real mid-chat switch (a prior turn exists and
      // the model/effort actually changed).
      if (changed && turnIndex > 0) {
        const existing = st.modelMarkersByTask[taskId] || [];
        // De-dupe on turnIndex so a resend after an edit/revert replaces the
        // marker at that position rather than stacking duplicates.
        const filtered = existing.filter((mk) => mk.turnIndex !== turnIndex);
        const marker = {
          id: `mk-${taskId}-${turnIndex}`,
          turnIndex,
          provider: current.provider,
          modelId: current.modelId,
          thinkingTier: current.thinkingTier,
        };
        const nextMarkers = {
          ...st.modelMarkersByTask,
          [taskId]: [...filtered, marker],
        };
        saveJsonMap(MODEL_MARKERS_KEY, nextMarkers);
        patch.modelMarkersByTask = nextMarkers;
      }
      return patch;
    });
  },

  /** Repairs a history the provider permanently rejects (4xx), then resumes the task. */
  async repairAndContinue(taskId) {
    const entry = get().providerErrorByTask[taskId];
    if (!entry) return;
    let outcome;
    try {
      outcome = await safeInvoke('repair_task_history', {
        taskId,
        error: entry.error,
      });
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Repair failed: ${msg}`);
      return;
    }
    // Nothing was repairable — resuming would replay the exact same request
    // and hit the exact same 4xx. Don't burn a turn (and don't grow history
    // with synthetic continue messages); tell the user instead.
    if (!outcome || outcome.stubbed === 0) {
      toast.error(
        outcome?.summary ||
          'No repairable content found in history — continuing would fail with the same error.',
      );
      return;
    }
    toast.success(outcome.summary);
    // Force a transcript refetch so stubbed blocks render as text notes.
    set((s) => {
      const nextLoaded = { ...s.historyLoadedByTask };
      delete nextLoaded[taskId];
      return { historyLoadedByTask: nextLoaded };
    });
    await get().loadTaskHistory(taskId).catch(() => {});
    await get()._sendMessageDirect(
      taskId,
      'The previous request failed with a provider error. The offending content in history has been converted to text notes. Continue from where you left off.',
      [],
      thinkingTierToBudget(get().thinkingTier),
    );
  },

  async _sendMessageDirect(taskId, text, attachments, thinkingBudget, extras = {}) {
    const state = get();
    // A fresh send invalidates any stale "Repair & continue" banner — the
    // user chose to move on (or the repair action itself is resuming).
    if (get().providerErrorByTask[taskId]) {
      set((s) => {
        const next = { ...s.providerErrorByTask };
        delete next[taskId];
        return { providerErrorByTask: next };
      });
    }
    // If a run is still in flight for this task, the backend cancels it
    // automatically when the new send_message lands (it signals the previous
    // run's cancel token before starting the fresh turn — see
    // commands/agent/mod.rs). The in-flight response / current tool call is
    // allowed to settle at the next cancellation checkpoint; we don't kill it
    // mid-token. All we must do on the frontend is finalize the abandoned
    // turn's animation so the old "Thinking…"/"Preparing…" indicator stops
    // spinning forever instead of hanging.
    if (get().streamingByTask[taskId]) {
      state.settleStreamAnimations(taskId);
    }
    // Note the model/effort this turn runs with (and drop a divider marker if
    // it changed mid-chat) BEFORE appending the user message, so the marker
    // anchors to the correct user-turn index.
    state._recordTurnModelChange(taskId);
    // Stash the attachments on the user message so the chat UI can render
    // their previews (chat-turn reads `att.url`). Path + media type are also
    // kept so the same record can rehydrate from disk if needed.
    state.appendUserMessage(taskId, text, attachments);
    // Flip streaming on synchronously so the chat shows its "Preparing…"
    // indicator (and the stop button) the instant the user hits send — even
    // before the set_task_permissions round-trip below. Cold-start of the
    // backend session on the first send can take a few seconds; without an
    // early indicator the chat looks frozen.
    set({ streamingByTask: { ...get().streamingByTask, [taskId]: true } });
    if (!isTauriAvailable()) {
      toast.error('Tauri runtime unavailable — open this in the desktop app to talk to the agent.');
      set((s) => ({ streamingByTask: { ...s.streamingByTask, [taskId]: false } }));
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
    // Push the selected model the SAME way we push the mode above. A freshly
    // created task is born with the project/global DEFAULT model — create_task
    // never saw the user's pick — and a session-restored pick only lives in
    // frontend state until the user re-clicks it. Without this sync the backend
    // runs the default (for OpenRouter that's `openrouter/auto`, which routes to
    // PAID models) even though the picker shows e.g. "OpenRouter Free".
    // switch_model is a no-op when the model already matches, and skips the
    // "switched to X" marker on a fresh (empty) task, so syncing every send is
    // safe and won't litter the transcript with dividers.
    if (state.selectedProvider && state.selectedModel) {
      try {
        await safeInvoke('switch_model', {
          taskId,
          providerType: state.selectedProvider,
          model: state.selectedModel,
        });
      } catch (e) { /* non-fatal — surfaces via send_message error if it matters */ }
    }
    // Record the reasoning tier on the task the same way — a freshly created
    // task has no tier yet, and this is what per-task model memory hydrates
    // from when the user reopens the task later.
    if (state.thinkingTier) {
      safeInvoke('set_task_thinking_tier', { taskId, tier: state.thinkingTier }).catch(() => {});
    }
    const recordPatch = { thinking_tier: state.thinkingTier || 'off' };
    if (state.selectedProvider && state.selectedModel) {
      recordPatch.provider_type = state.selectedProvider;
      recordPatch.model = state.selectedModel;
    }
    get()._patchTaskRecord(taskId, recordPatch);
    // Backend `send_message` expects images as { media_type, data } where
    // data is base64 (no data URL prefix). PromptBox stores attachments with
    // a richer shape for previews, so peel out just the bits send_message
    // needs here.
    const images = (attachments || [])
      .filter((a) => a && a.base64Data && a.mediaType)
      .map((a) => ({ media_type: a.mediaType, data: a.base64Data }));
    // Append the on-disk paths to the message body so the model sees both
    // the image (via inline vision) AND its file path — that way it can
    // reference the file with Read/edit tools in follow-up turns instead of
    // re-uploading. Kept as a short footer to avoid clobbering short prompts.
    let messageForBackend = text;
    const pathFooter = (attachments || [])
      .map((a) => a?.relativePath || a?.path)
      .filter(Boolean);
    if (pathFooter.length > 0) {
      const header = messageForBackend.trim().length > 0 ? `${messageForBackend}\n\n` : '';
      messageForBackend = `${header}[Attached images]\n${pathFooter
        .map((p) => `- ${p}`)
        .join('\n')}`;
    }
    // @-mentioned files are passed by REFERENCE only — we append their paths so
    // the model knows which files the user means and reads them itself via
    // read_file (windowed), rather than us dumping contents into context.
    const fileRefs = (extras?.fileTags || [])
      .map((f) => f?.relativePath || f?.path)
      .filter(Boolean);
    if (fileRefs.length > 0) {
      const header = messageForBackend.trim().length > 0 ? `${messageForBackend}\n\n` : '';
      messageForBackend = `${header}[Referenced files — read them with read_file as needed]\n${fileRefs
        .map((p) => `- ${p}`)
        .join('\n')}`;
    }
    try {
      await safeInvoke('send_message', {
        taskId,
        message: messageForBackend,
        thinkingBudget,
        images,
        injectedSkills: extras?.skills || [],
        injectedWorkflows: extras?.workflows || [],
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
    // Optimistically clears the active task's streaming flag so the Stop button
    // immediately flips back to "Send" mode. The backend will also emit a
    // terminal status (cancelled/failed/complete) which setStatus reconciles,
    // but it can arrive slowly when the model is mid-token.
    await get()._abortTaskById(get().activeTaskId);
  },

  async respondPermission(approved, opts = {}) {
    const req = get().pendingPermission;
    if (!req) return;
    set({ pendingPermission: null });
    if (approved && opts.alwaysAllow && req.task_id && req.operation) {
      set((s) => ({
        permissionAllowlistByTask: {
          ...s.permissionAllowlistByTask,
          [req.task_id]: {
            ...(s.permissionAllowlistByTask[req.task_id] || {}),
            [req.operation]: true,
          },
        },
      }));
    }
    if (!isTauriAvailable()) return;
    try {
      await safeInvoke('respond_to_permission', {
        taskId: req.task_id,
        requestId: req.request_id,
        approved,
      });
    } catch (e) {}
  },

  // Forward the user's answers back to the parked `ask_user` tool and mark
  // the inline ask_user block in the chat as resolved (so the form turns
  // into a read-only summary). `answers` is the `{ [questionId]: value }`
  // map built by AskUserInline; pass `{ cancelled: true }` to dismiss.
  async respondQuestion(requestId, answers, opts = {}) {
    if (!requestId) return;
    const cancelled = !!opts.cancelled;
    // Images the user attached to their answer, in the same { media_type, data }
    // base64 shape send_message uses. Dropped when the dialog was cancelled.
    const images = (!cancelled && Array.isArray(opts.images))
      ? opts.images
          .filter((a) => a && a.base64Data && a.mediaType)
          .map((a) => ({ media_type: a.mediaType, data: a.base64Data }))
      : [];
    // Patch the matching ask_user block in messagesByTask. We don't know
    // which task owns this request without scanning, so walk all tasks —
    // request_ids are uuid v4 so the scan is cheap and unambiguous.
    set((s) => {
      const nextByTask = { ...s.messagesByTask };
      for (const [tid, list] of Object.entries(s.messagesByTask)) {
        let touched = false;
        const nextList = list.map((m) => {
          const blocks = m.content || [];
          let blockTouched = false;
          const nextBlocks = blocks.map((b) => {
            if (b && b.type === 'ask_user' && b.request_id === requestId) {
              blockTouched = true;
              return {
                ...b,
                answered: !cancelled,
                cancelled,
                answers: cancelled ? null : (answers || {}),
              };
            }
            return b;
          });
          if (blockTouched) {
            touched = true;
            return { ...m, content: nextBlocks };
          }
          return m;
        });
        if (touched) nextByTask[tid] = nextList;
      }
      return { messagesByTask: nextByTask };
    });
    if (!isTauriAvailable()) return;
    try {
      await safeInvoke('respond_to_ask_user', {
        requestId,
        answers: cancelled ? null : (answers || {}),
        cancelled,
        images,
      });
    } catch (e) {
      // eslint-disable-next-line no-console
      console.error('[agent.respondQuestion] respond_to_ask_user failed', { requestId, error: e });
    }
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
        // Hydrate goal pills from the persisted tasks.goal column, but never
        // clobber a live entry the event stream is already maintaining.
        const goals = { ...s.goalByTask };
        let goalsTouched = false;
        for (const t of list) {
          if (t.goal) {
            const cur = goals[t.id];
            if (!cur || cur.condition !== t.goal) {
              goals[t.id] = { condition: t.goal, status: 'active', turns: 0, reason: null };
              goalsTouched = true;
            }
          } else if (goals[t.id]) {
            delete goals[t.id];
            goalsTouched = true;
          }
        }
        if (goalsTouched) patch.goalByTask = goals;
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
  // Authoritative refresh of the Files panel for one task. Pulls the
  // backend's net-change diff (`fh_list_task_net_changes`) and REPLACES the
  // task's entry set wholesale — it does NOT merge with the existing
  // entries.
  //
  // Why replace, not merge: the live `agent-file-tracked` event handler can
  // only ever append paths (edit-tool captures + post-bash sweeps), and it
  // never removes them. A path that was touched once — even transiently, or
  // later reverted back to its baseline content — would otherwise linger in
  // the set forever, inflating the count well past the real net change (the
  // "145 files vs 21 in git" report). The backend diff is the source of
  // truth for "what did this task actually change", so we let it overwrite.
  //
  // An empty result is meaningful: it clears the panel. Callers run this on
  // task open and on turn completion so a long-running task converges back
  // to the truth after each turn instead of drifting upward all session.
  async refreshTaskFiles(taskId) {
    if (!taskId) return;
    if (!isTauriAvailable()) return;
    // Project root comes from the active project — net-change diffs are only
    // ever requested for the active task's project. Missing root → skip;
    // the live-event path keeps the panel populated until the next refresh.
    const projectRoot = get().activeProject?.root || null;
    if (!projectRoot) return;
    try {
      const rows = await safeInvoke('fh_list_task_net_changes', {
        projectRoot,
        taskId,
      });
      const list = Array.isArray(rows) ? rows : [];
      set((s) => {
        const prev = s.filesByTask[taskId] || { entries: [], lastMessageId: null };
        const entries = list.map((row) => ({
          path: row.path,
          kind: row.kind || 'modified',
          binary: !!row.binary,
          additions: row.additions || 0,
          deletions: row.deletions || 0,
          anchor_message_id: row.anchor_message_id || '',
          is_dir: !!row.is_dir,
          // Default to true so old payloads without the field still render —
          // the row only gets hidden when the backend explicitly reports
          // `exists_on_disk: false`.
          exists_on_disk: row.exists_on_disk !== false,
        }));
        return {
          filesByTask: {
            ...s.filesByTask,
            [taskId]: { entries, lastMessageId: prev.lastMessageId },
          },
        };
      });
    } catch (e) {
      // Stringify so the console shows the real reason instead of "[object
      // Object]". This path is non-fatal — the live agent-file-tracked events
      // keep the panel populated — so log it as a warning, not an error.
      // eslint-disable-next-line no-console
      console.warn(
        '[agent.refreshTaskFiles] fh_list_task_net_changes failed:',
        e instanceof Error ? e.message : String(e),
        { taskId },
      );
    }
  },

  async loadTaskHistory(taskId) {
    if (!taskId) return;
    const state = get();
    if (state.historyLoadedByTask[taskId]) {
      return;
    }
    if (!isTauriAvailable()) return;

    // Mark loaded eagerly so concurrent setActiveTask calls don't double-fetch.
    // On failure below we clear the flag so a manual retry works.
    set((s) => ({
      historyLoadedByTask: { ...s.historyLoadedByTask, [taskId]: true },
    }));

    // Reconcile the Files panel against the backend's authoritative
    // net-change diff. Fire-and-forget so it doesn't block chat render —
    // the chat itself is ready in milliseconds and this full-walk diff can
    // take ~150-250ms on a large worktree.
    //
    // `refreshTaskFiles` REPLACES the per-task entry set (it does not merge
    // with whatever live `agent-file-tracked` stubs accumulated). That
    // replacement is what keeps the Files count honest: live events only
    // ever grow the set, so without an authoritative replace the count
    // drifts far above the real net change over a long task.
    get().refreshTaskFiles(taskId);

    try {
      const [messages, todos, cost, subagents, snapshots] = await Promise.all([
        safeInvoke('get_task_messages', { taskId }).catch((e) => {
          // eslint-disable-next-line no-console
          console.error('[agent.loadTaskHistory] get_task_messages failed', { taskId, error: e });
          return [];
        }),
        safeInvoke('get_task_todos', { taskId }).catch(() => []),
        safeInvoke('get_task_cost', { taskId }).catch(() => null),
        safeInvoke('get_subagent_records', { taskId }).catch(() => []),
        // Pull the per-task snapshot rows so reloaded user messages can carry
        // their checkpoint anchors. Empty list is fine — older tasks predating
        // the file-history tracker just won't show Revert buttons.
        safeInvoke('fh_list_snapshots', { taskId }).catch((e) => {
          // eslint-disable-next-line no-console
          console.error('[agent.loadTaskHistory] fh_list_snapshots failed', { taskId, error: e });
          return [];
        }),
      ]);

      const normalized = applySnapshotAnchors(
        normalizeLoadedMessages(taskId, messages),
        Array.isArray(snapshots) ? snapshots : [],
      );

      set((s) => {
        // Re-check: a live stream may have started while we were awaiting. If
        // the task is ACTIVELY streaming and messages appeared in the interim,
        // don't clobber them (the live stream is authoritative). But if it's
        // not streaming, always write the DB data to clear any stale partial
        // state from earlier sessions.
        const inMem = s.messagesByTask[taskId];
        const isActivelyStreaming = s.streamingByTask[taskId];

        if (isActivelyStreaming && Array.isArray(inMem) && inMem.length > 0) {
          return s;
        }

        // Hydrate sub-agent transcripts into the live map keyed by agentId so
        // SubagentInlineView and SpawnedSubagentRow work after a reload. Only
        // seed agentIds that aren't already present from a live stream — an
        // in-flight sub-agent's in-memory state is authoritative because it
        // preserves the exact text↔tool-call ordering that the DB record
        // can't (see subagentRecordToLive's note).
        const existingSubMap = s.subagentsByTask[taskId] || {};
        const nextSubMap = { ...existingSubMap };
        if (Array.isArray(subagents)) {
          for (const rec of subagents) {
            if (!rec || !rec.agent_id) continue;
            if (nextSubMap[rec.agent_id]) continue;
            nextSubMap[rec.agent_id] = subagentRecordToLive(rec);
          }
        }

        return {
          messagesByTask: { ...s.messagesByTask, [taskId]: normalized },
          todosByTask: { ...s.todosByTask, [taskId]: Array.isArray(todos) ? todos : [] },
          costByTask: cost ? { ...s.costByTask, [taskId]: cost } : s.costByTask,
          subagentRecordsByTask: {
            ...s.subagentRecordsByTask,
            [taskId]: Array.isArray(subagents) ? subagents : [],
          },
          subagentsByTask: {
            ...s.subagentsByTask,
            [taskId]: nextSubMap,
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

    // Clear a pending retry banner the moment ANY stream activity resumes —
    // a token, a thinking delta, or a tool call. The agent is demonstrably
    // back online, so a stale "Retrying… (attempt N of M)" banner should not
    // linger while it's already thinking again (the recovery never emits a
    // text token first, so clearing only on `agent-stream` left it stuck).
    const clearRetry = (taskId) => {
      if (!taskId) return;
      if (get().retryByTask[taskId]) {
        set((s) => {
          const next = { ...s.retryByTask };
          delete next[taskId];
          return { retryByTask: next };
        });
      }
    };

    const handlers = {
      'agent-stream': (p) => {
        clearRetry(p.task_id);
        // Batched: buffered and applied on a ~40ms trailing timer instead of
        // one store write per token (see queueStreamDelta at module top).
        queueStreamDelta(p.task_id, 'text', p.text || '');
      },
      'agent-thinking-delta': (p) => {
        clearRetry(p.task_id);
        queueStreamDelta(p.task_id, 'thinking', p.text || '');
      },
      'agent-thinking-done': (p) =>
        get().markThinkingDone(p.task_id, p.duration_secs ?? 0),
      'agent-tool-use-start': (p) => {
        clearRetry(p.task_id);
        // Flush any pending text/thinking buffers before tool use starts
        const { appendToolUse } = get();
        appendToolUse(p.task_id, p.tool_use_id, p.tool_name, {}, /* streaming */ true);
      },
      'agent-tool-use-input-delta': (p) => {
        get().accumulateToolInputDelta(p.task_id, p.tool_use_id, p.partial_json);
      },
      'agent-tool-use-stop': (p) => {
        get().finalizeToolInputStreaming(p.task_id, p.tool_use_id);
      },
      'agent-tool-use': (p) => {
        clearRetry(p.task_id);
        get().addToolUse(p.task_id, p.tool_use_id, p.tool_name, p.tool_input);
      },
      'agent-tool-result': (p) => {
        clearRetry(p.task_id);
        get().addToolResult(p.task_id, p.tool_use_id, p.output, p.is_error);
      },
      'agent-cost-update': (p) => get().setCost(p.task_id, p.cost),
      'agent-request-usage': (p) => {
        const taskId = p?.taskId;
        if (!taskId) return;
        const tokens =
          (p.inputTokens || 0) + (p.cacheReadTokens || 0) + (p.cacheWriteTokens || 0);
        set((s) => ({
          contextUsageByTask: {
            ...s.contextUsageByTask,
            [taskId]: {
              tokens,
              contextWindow: p.contextWindow || 0,
              threshold: p.condenseThreshold || 0,
            },
          },
        }));
      },
      'agent-task-status': (p) => get().setStatus(p.task_id, p.status),
      'agent-task-complete': (p) => {
        get().finishStream(p.task_id);
        get().setStatus(p.task_id, 'complete');
        // Reconcile the Files panel against the backend's authoritative
        // net-change diff now that the turn is done. Live agent-file-tracked
        // events only grow the entry set during a turn; this replaces it with
        // the real net change so the count converges instead of drifting up.
        get().refreshTaskFiles(p.task_id);
      },
      'agent-permission-request': (p) => get().openPermission(p),
      'agent-ask-user-request': (p) =>
        get().appendAskUserBlock(p?.task_id, p?.request_id, p?.questions),
      'agent-todo-updated': (p) => get().setTodos(p.task_id, p.todos || []),
    'agent-goal-update': (p) => {
      const { task_id: taskId, status, condition, turns, reason } = p;
      set((s) => {
        const next = { ...s.goalByTask };
        if (status === 'met') {
          delete next[taskId];
        } else {
          next[taskId] = { condition, status, turns: turns || 0, reason: reason || null };
        }
        return { goalByTask: next };
      });
      if (status === 'met') {
        toast.success('Goal met — the loop has finished.', { duration: 8000 });
        get()._patchTaskRecord(taskId, { goal: null });
      } else if (status === 'error') {
        toast.error(`Goal loop stopped: ${reason || 'evaluator failed'}`, { duration: 8000 });
      }
    },
      'agent-title-changed': (p) => get().setTitle(p.task_id, p.title),
      'agent-context-condense-started': (p) => {
        const taskId = p.task_id;
        if (!taskId) return;
        set((s) => {
          const patch = {
            condensingByTask: {
              ...s.condensingByTask,
              [taskId]: {
                started_at_ms: Date.now(),
                estimated_tokens: p.estimated_tokens || 0,
                threshold: p.threshold || 0,
              },
            },
          };
          // Sync the context meter to the trigger estimate so the pill never
          // sits at a low % while the compacting banner is showing.
          if (p.estimated_tokens > 0) {
            const prev = s.contextUsageByTask[taskId] || {};
            patch.contextUsageByTask = {
              ...s.contextUsageByTask,
              [taskId]: {
                ...prev,
                tokens: p.estimated_tokens,
                threshold: p.threshold || prev.threshold || 0,
                contextWindow: prev.contextWindow || 0,
              },
            };
          }
          return patch;
        });
      },
      'agent-context-condense-completed': (p) => {
        const taskId = p.task_id;
        if (!taskId) return;

        // Log for debugging
        if (p.original_messages && p.condensed_to) {
          // eslint-disable-next-line no-console
          console.log(`[agent] Context condensed: ${p.original_messages} → ${p.condensed_to} messages`);
        }

        // Clear the condensing flag and send any queued message
        get()._flushCondenseQueue(taskId);
      },
      'agent-turn-started': (p) =>
        get().anchorCheckpoint(
          p.task_id,
          p.snapshot_message_id,
          p.user_message_index,
        ),
      // Sub-agent stream. Each event includes both task_id and agent_id; the
      // store keeps an independent live transcript per spawned child so the
      // user can click into a sub-agent and watch it work in real time.
      'agent-subagent-spawned': (p) => {
        get()._ensureSubagent(p.task_id, p.agent_id, {
          model: p.model || '',
          prompt: p.prompt || '',
          name: p.name || '',
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
      'agent-provider-error': (p) => {
        // A deterministic 4xx — retrying can't fix it. Store the error so
        // <ProviderErrorBanner> can offer a one-click repair + resume.
        const taskId = p.task_id;
        if (!taskId) return;
        set((s) => ({
          providerErrorByTask: {
            ...s.providerErrorByTask,
            [taskId]: { error: p.error || 'Provider rejected the request', at: Date.now() },
          },
        }));
      },
      'agent-stream-retry': (p) => {
        // Backend is about to wait `waiting_ms` then retry. Store the
        // info so <StreamRetryBanner> can render a countdown.
        const taskId = p.task_id;
        if (!taskId) return;
        set((s) => ({
          retryByTask: {
            ...s.retryByTask,
            [taskId]: {
              attempt: p.attempt,
              max_attempts: p.max_attempts,
              waiting_ms: p.waiting_ms,
              error: p.error || null,
              started_at_ms: Date.now(),
            },
          },
        }));
      },
      'agent-refusal': (p) => {
        // A Claude Fable 5-class safety classifier declined the request. Store
        // it (for any inline UI) and prompt the user to switch to the fallback
        // model (e.g. Opus 4.8). We do NOT auto-resend: the refused message is
        // still in history, so a blind retry on the same model bounces again;
        // switching the model lets the user edit + resend deliberately.
        const taskId = p.task_id;
        if (!taskId) return;
        set((s) => ({
          refusalByTask: {
            ...s.refusalByTask,
            [taskId]: {
              model: p.model || null,
              category: p.category || null,
              fallback_model: p.fallback_model || null,
            },
          },
        }));

        const fallback = p.fallback_model;
        const modelLabel = modelDisplayName(p.model) || 'The current model';
        const reason = refusalCategoryLabel(p.category);
        const fallbackLabel = fallback ? (modelDisplayName(fallback) || fallback) : null;

        confirm({
          title: 'Request blocked by the model',
          description:
            `${modelLabel} declined this request${reason ? ` (${reason})` : ''} via its safety classifier `
            + `and ended the turn.`
            + (fallbackLabel
              ? `\n\nSwitch to ${fallbackLabel} to continue? You may need to edit and re-send your message.`
              : `\n\nTry editing your message, or switch to a different model.`),
          confirmLabel: fallbackLabel ? `Switch to ${fallbackLabel}` : 'OK',
          cancelLabel: fallbackLabel ? 'Stay on current model' : 'Dismiss',
        }).then((ok) => {
          // Clear the stored refusal regardless of choice.
          set((s) => {
            const next = { ...s.refusalByTask };
            delete next[taskId];
            return { refusalByTask: next };
          });
          if (ok && fallback) {
            get().setSelectedModel('claude', fallback);
          }
        });
      },
      'agent-file-tracked': (p) => {
        // Either Edit-tool (synchronous capture before a Write/Edit) or
        // Bash-sweep (post-bash full-walk diff) — both populate the same
        // per-task entries list. Live events only carry paths (no stats);
        // entries get stubbed with kind='modified' and zero counts, then
        // the next background fh_list_task_net_changes refresh corrects
        // them to the real per-file values.
        const taskId = p.task_id;
        const paths = Array.isArray(p.paths) ? p.paths : [];
        
        // eslint-disable-next-line no-console
        console.log('[agent] agent-file-tracked event', { taskId, pathCount: paths.length, paths });
        
        if (!taskId || paths.length === 0) return;
        set((s) => {
          const prev = s.filesByTask[taskId] || { entries: [], lastMessageId: null };
          const seen = new Set(prev.entries.map((e) => e.path));
          const merged = [...prev.entries];
          for (const path of paths) {
            if (!seen.has(path)) {
              seen.add(path);
              merged.push({
                path,
                kind: 'modified',
                binary: false,
                additions: 0,
                deletions: 0,
                anchor_message_id: p.message_id || '',
                is_dir: false,
                // Live event fired, so the path existed at the moment the
                // event arrived. Hydration will later overwrite with the
                // real on-disk state.
                exists_on_disk: true,
              });
            }
          }
          return {
            filesByTask: {
              ...s.filesByTask,
              [taskId]: {
                entries: merged,
                lastMessageId: p.message_id || prev.lastMessageId,
              },
            },
          };
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
