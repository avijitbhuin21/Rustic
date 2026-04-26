/**
 * Message normalization, collapsing, and grouping pipeline.
 *
 * Transforms the flat message array from the agent store into a richer structure
 * that supports:
 * - Collapsed read/search groups (like Claude Code's collapseReadSearchGroups)
 * - Parallel tool call grouping (like Claude Code's applyGrouping)
 *
 * Flow: messages → normalizeMessages() → collapseReadSearchGroups() → groupParallelTools() → render
 */

// Tools considered read-only for collapsing purposes.
const READ_ONLY_TOOLS = new Set([
  'read_file',
  'list_directory',
  'grep_search',
  'read_skill',
]);

/**
 * Normalize a flat message array into a list of renderable nodes.
 * Each node represents one logical UI element (text bubble, tool card, thinking block, etc.)
 *
 * @param {Array} messages - The task's message array
 * @param {Map} resultMap - tool_use_id → tool_result block map
 * @returns {Array} Normalized nodes
 */
// Canned assistant reply paired with the injected [Project Memory] user
// pseudo-message. Skipped from inline rendering so the memory-injection
// round-trip stays invisible to the user.
const MEMORY_INJECT_ACK = "Memory loaded. I'll reference this context as needed.";

export function normalizeMessages(messages, resultMap) {
  const nodes = [];
  let thinkingCounter = 0; // unique counter across all thinking blocks

  // The executor persists the complete_task summary as a regular trailing
  // assistant text message AND emits it on the TaskComplete event (which
  // becomes a `task_complete` role message via appendTaskComplete). We render
  // it as a green markdown card from the latter — so suppress the duplicate
  // assistant text when their content matches.
  const suppressedAssistantIdx = (() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.role !== 'task_complete') continue;
      const summary = m.content?.[0]?.summary;
      if (!summary) return -1;
      // Walk back over any later non-text messages to find the previous assistant text bubble.
      for (let j = i - 1; j >= 0; j--) {
        const prev = messages[j];
        if (prev.role !== 'assistant') continue;
        const onlyText = prev.content?.length === 1 && prev.content[0].type === 'text';
        if (!onlyText) return -1;
        return prev.content[0].text?.trim() === summary.trim() ? j : -1;
      }
      return -1;
    }
    return -1;
  })();

  for (let i = 0; i < messages.length; i++) {
    if (i === suppressedAssistantIdx) continue;
    const msg = messages[i];

    // Tool messages are consumed via resultMap — skip
    if (msg.role === 'tool') continue;

    // Task complete
    if (msg.role === 'task_complete') {
      nodes.push({ type: 'task-complete', content: msg.content[0], msgIdx: i });
      continue;
    }

    // Context condense marker
    if (msg.content?.length === 1 && msg.content[0].type === 'context_condense') {
      nodes.push({ type: 'context-condense', content: msg.content[0], msgIdx: i });
      continue;
    }

    // Model switch
    if (msg.content?.length === 1 && msg.content[0].type === 'model_switch') {
      const nextMsg = messages[i + 1];
      const nextIsSwitch = nextMsg?.content?.length === 1 && nextMsg.content[0].type === 'model_switch';
      if (!nextMsg || nextIsSwitch) continue; // suppress trailing/consecutive
      nodes.push({ type: 'model-switch', content: msg.content[0], msgIdx: i });
      continue;
    }

    // User message — but skip:
    //   1. Messages that only contain tool_result blocks (these are loaded
    //      from history where the API stored tool results under the User role;
    //      they're already consumed via resultMap).
    //   2. The injected `[Project Memory]` pseudo-message used to preload the
    //      model with project memory; it's not a real user prompt.
    if (msg.role === 'user') {
      const hasOnlyToolResults = msg.content?.length > 0 &&
        msg.content.every(b => b.type === 'tool_result');
      if (hasOnlyToolResults) continue;
      const firstBlock = msg.content?.[0];
      if (firstBlock?.type === 'text' && firstBlock.text?.startsWith('[Project Memory]')) continue;
      nodes.push({ type: 'user-message', msg, msgIdx: i });
      continue;
    }

    // Assistant message — split into individual blocks
    if (msg.role === 'assistant') {
      // Skip the canned "Memory loaded" acknowledgement that pairs with the
      // [Project Memory] user pseudo-message. Together with the skip above
      // this keeps the memory-injection invisible in the chat.
      const onlyText = msg.content?.length === 1 && msg.content[0].type === 'text';
      if (onlyText && msg.content[0].text === MEMORY_INJECT_ACK) continue;
      // Track contiguous text blocks to group them
      let textBlocks = [];

      const flushText = () => {
        if (textBlocks.length > 0) {
          nodes.push({
            type: 'assistant-text',
            blocks: [...textBlocks],
            msgIdx: i,
            isLastMsg: i === messages.length - 1,
          });
          textBlocks = [];
        }
      };

      for (let ci = 0; ci < msg.content.length; ci++) {
        const block = msg.content[ci];
        if (block.type === 'thinking') {
          flushText();
          nodes.push({
            type: 'thinking',
            block,
            msgIdx: i,
            blockIdx: thinkingCounter++,
            contentIdx: ci,
            isLastMsg: i === messages.length - 1,
          });
        } else if (block.type === 'text' && block.text) {
          textBlocks.push(block);
        } else if (block.type === 'tool_use') {
          flushText();
          nodes.push({
            type: 'tool-use',
            toolUseId: block.id,
            toolName: block.name,
            toolInput: block.input || {},
            toolResult: resultMap.get(block.id) || null,
            msgIdx: i,
            block,
          });
        }
      }

      // If assistant message is empty and streaming, emit thinking indicator
      const isLastMsg = i === messages.length - 1;
      if (textBlocks.length === 0 && nodes.filter(n => n.msgIdx === i).length === 0 && isLastMsg) {
        nodes.push({ type: 'thinking-indicator', msgIdx: i });
      } else {
        flushText();
      }

      // Checkpoint info (attached to the message)
      nodes.push({ type: 'checkpoint-anchor', msgIdx: i, msg });
    }
  }

  return nodes;
}

/**
 * Collapse consecutive read-only tool-use nodes into a single collapsed group.
 * Sequences of 2+ consecutive read-only tools become a CollapsedGroup.
 *
 * @param {Array} nodes - Normalized nodes from normalizeMessages()
 * @returns {Array} Nodes with collapsed groups
 */
export function collapseReadSearchGroups(nodes) {
  const result = [];
  let readBatch = [];

  const flushBatch = () => {
    if (readBatch.length >= 2) {
      // Build summary
      const counts = {};
      for (const node of readBatch) {
        const name = node.toolName;
        counts[name] = (counts[name] || 0) + 1;
      }
      const parts = [];
      if (counts.read_file) parts.push(`Read ${counts.read_file} file${counts.read_file > 1 ? 's' : ''}`);
      if (counts.list_directory) parts.push(`Listed ${counts.list_directory} director${counts.list_directory > 1 ? 'ies' : 'y'}`);
      if (counts.grep_search) parts.push(`${counts.grep_search} search${counts.grep_search > 1 ? 'es' : ''}`);
      if (counts.read_skill) parts.push(`Read ${counts.read_skill} skill${counts.read_skill > 1 ? 's' : ''}`);
      const summary = parts.join(', ');

      // All completed?
      const allCompleted = readBatch.every(n => n.toolResult != null);
      const anyError = readBatch.some(n => n.toolResult?.is_error);

      result.push({
        type: 'collapsed-group',
        summary,
        children: [...readBatch],
        allCompleted,
        anyError,
        count: readBatch.length,
      });
    } else {
      // Single read-only tool — just push as-is
      result.push(...readBatch);
    }
    readBatch = [];
  };

  for (const node of nodes) {
    if (node.type === 'tool-use' && READ_ONLY_TOOLS.has(node.toolName)) {
      readBatch.push(node);
    } else if (node.type === 'checkpoint-anchor') {
      // Transparent marker — renders to nothing. Historically it sat between
      // sequential tool-use nodes (one per assistant turn), which broke the
      // read-batch and made history show individual cards where live showed
      // a collapsed "Read N files, Listed M directories" group. Pass it
      // through without flushing the batch so both views collapse the same
      // way.
      result.push(node);
    } else {
      flushBatch();
      result.push(node);
    }
  }
  flushBatch();

  return result;
}

/**
 * Group multiple tool-use nodes from the same assistant message into a parallel group.
 * Only groups when 2+ tool-use nodes share the same msgIdx.
 *
 * @param {Array} nodes - Nodes (possibly with collapsed groups)
 * @returns {Array} Nodes with parallel groups
 */
export function groupParallelTools(nodes) {
  const result = [];
  let i = 0;

  while (i < nodes.length) {
    const node = nodes[i];

    // Only attempt grouping for tool-use and collapsed-group nodes
    if (node.type === 'tool-use' || node.type === 'collapsed-group') {
      const msgIdx = node.type === 'collapsed-group' ? node.children[0]?.msgIdx : node.msgIdx;

      // Collect all consecutive tool-related nodes from the same message
      const group = [node];
      let j = i + 1;
      while (j < nodes.length) {
        const next = nodes[j];
        const nextMsgIdx = next.type === 'collapsed-group'
          ? next.children[0]?.msgIdx
          : next.type === 'tool-use' ? next.msgIdx : null;
        if (nextMsgIdx === msgIdx && (next.type === 'tool-use' || next.type === 'collapsed-group')) {
          group.push(next);
          j++;
        } else {
          break;
        }
      }

      if (group.length >= 2) {
        result.push({
          type: 'parallel-group',
          children: group,
          count: group.reduce((sum, g) =>
            sum + (g.type === 'collapsed-group' ? g.count : 1), 0),
          msgIdx,
        });
      } else {
        result.push(node);
      }
      i = j;
    } else {
      result.push(node);
      i++;
    }
  }

  return result;
}

/**
 * Full pipeline: normalize → collapse → group.
 *
 * @param {Array} messages - The task's message array
 * @param {Map} resultMap - tool_use_id → tool_result block map
 * @returns {Array} Processed nodes ready for rendering
 */
export function processMessages(messages, resultMap) {
  const normalized = normalizeMessages(messages, resultMap);
  const collapsed = collapseReadSearchGroups(normalized);
  const grouped = groupParallelTools(collapsed);
  return grouped;
}
