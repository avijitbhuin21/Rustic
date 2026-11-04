import { useRef } from 'react';

/** Collects the tool_result entries of one message into [tool_use_id, entry] pairs. */
function resultEntries(m) {
  const out = [];
  for (const block of m.content || []) {
    if (block.type === 'tool_result') {
      out.push([
        block.tool_use_id,
        {
          output: block.output,
          is_error: block.is_error,
          // Present when the transcript shipped a cut-down output; the card
          // fetches the rest on expand.
          truncated: block.truncated,
          truncatedTaskId: block.truncatedTaskId,
          truncatedSortOrder: block.truncatedSortOrder,
          blockIndex: block.block_index,
        },
      ]);
    }
  }
  return out;
}

/** Appends turns for messages[from..] onto `turns`, recording each turn's starting message index. */
function appendTurns(messages, from, turns, starts) {
  let current = turns.length ? turns[turns.length - 1] : null;
  for (let i = from; i < messages.length; i++) {
    const m = messages[i];
    if (m.role === 'tool') continue;
    if (m.role === 'user') {
      current = { user: m, blocks: [] };
      turns.push(current);
      starts.push(i);
      continue;
    }
    if (m.role === 'assistant') {
      if (!current) {
        // Assistant content with no preceding user message — rare, but render
        // it in its own headerless turn rather than dropping it.
        current = { user: null, blocks: [] };
        turns.push(current);
        starts.push(i);
      }
      for (const block of m.content || []) {
        current.blocks.push({
          block,
          messageId: m.id,
          streaming: !!m.streaming,
          timestamp: m.timestamp || 0,
        });
      }
    }
  }
}

/**
 * Creates a stateful builder that groups a flat message stream into turns
 * (one user message + the assistant blocks after it) and a tool_use_id →
 * result map. The store updates messages immutably, so only the turn that
 * contains the first changed message is rebuilt; earlier turns and unchanged
 * result entries keep their identity, which ChatTurn's memo relies on.
 */
export function createTurnBuilder() {
  let prevMessages = [];
  let prevTurns = [];
  let prevStarts = [];
  let prevResults = {};
  const entryCache = new WeakMap();

  /** Returns the cached result entries for a message, recomputing when its content changed. */
  const entriesFor = (m) => {
    const hit = entryCache.get(m);
    if (hit && hit.content === m.content) return hit.entries;
    const entries = resultEntries(m);
    entryCache.set(m, { content: m.content, entries });
    return entries;
  };

  return function build(messages) {
    const msgs = messages || [];
    let k = 0;
    const common = Math.min(msgs.length, prevMessages.length);
    while (k < common && msgs[k] === prevMessages[k]) k++;
    if (k === msgs.length && k === prevMessages.length) {
      return { turns: prevTurns, toolResults: prevResults };
    }

    // Reuse every turn that starts strictly before the turn holding message k.
    let keep = 0;
    while (keep < prevStarts.length && prevStarts[keep] <= k) keep++;
    keep = Math.max(0, keep - 1);
    // A rebuilt range must open with a user message; otherwise its blocks
    // would be appended onto a reused (memoised) turn, so rebuild everything.
    if (keep > 0 && msgs[prevStarts[keep]]?.role !== 'user') keep = 0;
    const from = keep < prevStarts.length ? prevStarts[keep] : 0;
    const turns = prevTurns.slice(0, keep);
    const starts = prevStarts.slice(0, keep);
    appendTurns(msgs, keep === 0 ? 0 : from, turns, starts);

    const toolResults = {};
    for (const m of msgs) {
      for (const [id, entry] of entriesFor(m)) toolResults[id] = entry;
    }

    prevMessages = msgs;
    prevTurns = turns;
    prevStarts = starts;
    prevResults = toolResults;
    return { turns, toolResults };
  };
}

/** React hook wrapper: one incremental builder per component instance. */
export function useTurns(messages) {
  const ref = useRef(null);
  if (!ref.current) ref.current = createTurnBuilder();
  return ref.current(messages);
}
