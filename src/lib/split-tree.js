// Pure helpers for the terminal split layout — a binary(-ish) tree where leaves
// are terminal sessions and internal nodes are resizable splits. Kept free of
// React/Zustand so the logic is easy to reason about and test in isolation.
//
// Node shapes:
//   leaf:  { type: 'leaf',  id, sessionId }
//   split: { type: 'split', id, direction: 'row' | 'column', children: Node[], sizes: number[] }
//
// `direction: 'row'` lays children out left-to-right (a vertical divider);
// `direction: 'column'` stacks them top-to-bottom (a horizontal divider).
// `sizes` are percentages that sum to ~100, one per child.

let nodeSeq = 1;
function nextId(prefix) {
  // Monotonic counter — stable within a run and good enough for React keys and
  // panel ids. (We deliberately avoid time/random so behavior is deterministic.)
  return `${prefix}-${nodeSeq++}`;
}

export function makeLeaf(sessionId) {
  return { type: 'leaf', id: nextId('leaf'), sessionId };
}

/** Even split sizes for `n` children. */
function evenSizes(n) {
  if (n <= 0) return [];
  const base = Math.floor((100 / n) * 1000) / 1000;
  const sizes = Array(n).fill(base);
  // Push rounding remainder onto the last child so they sum to 100.
  sizes[n - 1] = Math.round((100 - base * (n - 1)) * 1000) / 1000;
  return sizes;
}

/** All session ids referenced by leaves in the tree, in left-to-right order. */
export function collectSessionIds(node, out = []) {
  if (!node) return out;
  if (node.type === 'leaf') {
    out.push(node.sessionId);
  } else {
    for (const c of node.children) collectSessionIds(c, out);
  }
  return out;
}

/**
 * Split the leaf holding `targetSessionId` into a split node containing the
 * original leaf and a new leaf for `newSessionId`. `placement` is one of
 * 'left' | 'right' | 'top' | 'bottom' and determines the split direction and
 * which side the new pane lands on. Returns a NEW tree (immutable). If
 * `newSessionId` already exists in the tree it is removed first so a session
 * never appears twice.
 */
export function splitAt(root, targetSessionId, newSessionId, placement) {
  const direction = placement === 'left' || placement === 'right' ? 'row' : 'column';
  const before = placement === 'left' || placement === 'top';

  // Ensure the incoming session isn't duplicated elsewhere in the tree.
  let working = removeSession(root, newSessionId);
  if (!working) {
    // The tree became empty (target was the only/last pane and equalled the new
    // one) — just start fresh with the new leaf.
    return makeLeaf(newSessionId);
  }

  const replaceLeaf = (node) => {
    if (node.type === 'leaf') {
      if (node.sessionId !== targetSessionId) return node;
      const newLeaf = makeLeaf(newSessionId);
      const children = before ? [newLeaf, node] : [node, newLeaf];
      return {
        type: 'split',
        id: nextId('split'),
        direction,
        children,
        sizes: evenSizes(2),
      };
    }
    return { ...node, children: node.children.map(replaceLeaf) };
  };

  return replaceLeaf(working);
}

/**
 * Remove the leaf for `sessionId`. Collapses any split left with a single child
 * (hoisting that child up), and returns null if the tree becomes empty.
 * Returns a NEW tree (immutable); returns the input unchanged if not found.
 */
export function removeSession(node, sessionId) {
  if (!node) return null;
  if (node.type === 'leaf') {
    return node.sessionId === sessionId ? null : node;
  }
  const kept = [];
  const keptSizes = [];
  node.children.forEach((child, i) => {
    const next = removeSession(child, sessionId);
    if (next) {
      kept.push(next);
      keptSizes.push(node.sizes?.[i] ?? 0);
    }
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0]; // collapse single-child split
  // Re-normalize sizes of the survivors back to 100.
  const total = keptSizes.reduce((a, b) => a + b, 0) || kept.length;
  const sizes = keptSizes.map((s) => Math.round((s / total) * 100 * 1000) / 1000);
  return { ...node, children: kept, sizes };
}

/** Update the `sizes` of the split node with `nodeId`. Returns a NEW tree. */
export function setSizes(node, nodeId, sizes) {
  if (!node || node.type === 'leaf') return node;
  if (node.id === nodeId) return { ...node, sizes };
  return { ...node, children: node.children.map((c) => setSizes(c, nodeId, sizes)) };
}

/**
 * Prune leaves whose session is no longer alive. `liveIds` is a Set of valid
 * session ids. Returns a NEW tree (or null if nothing remains).
 */
export function pruneDeadSessions(node, liveIds) {
  if (!node) return null;
  if (node.type === 'leaf') return liveIds.has(node.sessionId) ? node : null;
  const kept = [];
  const keptSizes = [];
  node.children.forEach((child, i) => {
    const next = pruneDeadSessions(child, liveIds);
    if (next) {
      kept.push(next);
      keptSizes.push(node.sizes?.[i] ?? 0);
    }
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0];
  const total = keptSizes.reduce((a, b) => a + b, 0) || kept.length;
  const sizes = keptSizes.map((s) => Math.round((s / total) * 100 * 1000) / 1000);
  return { ...node, children: kept, sizes };
}
