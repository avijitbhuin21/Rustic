import { useRef } from 'react';

/** The spawn_subagent calls of one assistant message as [{ id, agents }], in block order. */
function spawnCalls(m) {
  const out = [];
  if (m.role !== 'assistant') return out;
  for (const b of m.content || []) {
    if (b.type !== 'tool_use' || b.name !== 'spawn_subagent') continue;
    let agents = b.input?.agents;
    if (typeof agents === 'string') {
      try {
        agents = JSON.parse(agents);
      } catch {
        agents = null;
      }
    }
    if (Array.isArray(agents)) out.push({ id: b.id, agents });
  }
  return out;
}

/**
 * Creates a stateful builder for the agentId → name map recovered from
 * spawn_subagent calls (the model's `agents` input carries the names, the tool
 * output carries the spawned ids in matching positions). The store updates
 * messages immutably, so only messages from the first changed one onward are
 * walked again; a streaming flush touches the last message, not the transcript.
 * `parseIds` turns a tool output into the spawned ids.
 */
export function createSubagentNameIndex(parseIds) {
  let prevMessages = [];
  // Flat list of { at: message index, id, agents } in transcript order.
  let prevSpawns = [];
  let prevResults = null;
  let prevMap = {};
  // tool_use id → { output, ids }: an output is parsed once.
  const idCache = new Map();

  /** Returns the spawned ids of one call, parsing its output only when it changed. */
  const idsFor = (toolUseId, output) => {
    const hit = idCache.get(toolUseId);
    if (hit && hit.output === output) return hit.ids;
    const ids = parseIds(output);
    idCache.set(toolUseId, { output, ids });
    return ids;
  };

  return function build(messages, toolResults) {
    const msgs = messages || [];
    let k = 0;
    const common = Math.min(msgs.length, prevMessages.length);
    while (k < common && msgs[k] === prevMessages[k]) k++;
    const sameMessages = k === msgs.length && k === prevMessages.length;
    if (sameMessages && toolResults === prevResults) return prevMap;

    let spawns = prevSpawns;
    if (!sameMessages) {
      let keep = 0;
      while (keep < prevSpawns.length && prevSpawns[keep].at < k) keep++;
      spawns = prevSpawns.slice(0, keep);
      for (let i = k; i < msgs.length; i++) {
        for (const call of spawnCalls(msgs[i])) spawns.push({ at: i, ...call });
      }
    }

    const map = {};
    for (const s of spawns) {
      const ids = idsFor(s.id, toolResults?.[s.id]?.output);
      ids.forEach((id, i) => {
        if (id && s.agents[i]?.name) map[id] = s.agents[i].name;
      });
    }

    prevMessages = msgs;
    prevSpawns = spawns;
    prevResults = toolResults;
    prevMap = map;
    return map;
  };
}

/** React hook wrapper: one incremental name index per component instance. */
export function useSubagentNames(messages, toolResults, parseIds) {
  const ref = useRef(null);
  if (!ref.current) ref.current = createSubagentNameIndex(parseIds);
  return ref.current(messages, toolResults);
}
