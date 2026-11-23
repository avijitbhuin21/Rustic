/**
 * Closes every live indicator in a transcript: streaming messages stop and open
 * thinking blocks are marked done. With `stopped`, the run was cut short, so
 * every tool_use still waiting on a result is also flagged `interrupted`, as is
 * a thinking block the run ended on (one followed by other output had finished).
 * Returns the same array when nothing changed.
 */
export function settleRunMessages(messages, { stopped = false } = {}) {
  const list = messages || [];
  const lastIdx = list.length - 1;
  let resolved = null;
  if (stopped) {
    resolved = new Set();
    for (const m of list) {
      for (const b of m.content || []) {
        if (b && b.type === 'tool_result') resolved.add(b.tool_use_id);
      }
    }
  }
  let touched = false;
  const next = list.map((m, mIdx) => {
    const content = m.content || [];
    let blockTouched = false;
    const nextContent = content.map((b, bIdx) => {
      if (!b || typeof b !== 'object') return b;
      if (b.type === 'thinking' && !b.done) {
        blockTouched = true;
        const cutShort = stopped && mIdx === lastIdx && bIdx === content.length - 1;
        return cutShort ? { ...b, done: true, interrupted: true } : { ...b, done: true };
      }
      if (resolved && b.type === 'tool_use' && !b.interrupted && !resolved.has(b.id)) {
        blockTouched = true;
        return { ...b, interrupted: true };
      }
      return b;
    });
    if (!m.streaming && !blockTouched) return m;
    touched = true;
    return { ...m, streaming: false, content: blockTouched ? nextContent : m.content };
  });
  return touched ? next : list;
}
