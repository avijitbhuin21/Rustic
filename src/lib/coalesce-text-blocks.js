// Assistant replies can arrive as many adjacent text blocks split mid-sentence
// (Claude cuts text at every citation boundary). Rendering each fragment as its
// own markdown document breaks lists, bold, tables and links that straddle a
// cut — the "replies occasionally lose their formatting" bug. Merge them back.

/**
 * Merges consecutive `{ block: { type: 'text' } }` entries that belong to the
 * same message into one text block. Truncated blocks (lazy-loaded full text)
 * are left alone so their fetch-on-demand indices stay valid.
 */
export function coalesceTextBlocks(entries) {
  const out = [];
  for (const entry of entries || []) {
    const prev = out[out.length - 1];
    const b = entry?.block;
    if (
      prev &&
      b?.type === 'text' &&
      prev.block?.type === 'text' &&
      prev.messageId === entry.messageId &&
      !b.truncated &&
      !prev.block.truncated
    ) {
      out[out.length - 1] = {
        ...prev,
        streaming: entry.streaming,
        block: { ...prev.block, text: (prev.block.text || '') + (b.text || '') },
      };
      continue;
    }
    out.push(entry);
  }
  return out;
}
