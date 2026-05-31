/**
 * Shared flag that file-node.jsx sets when a context menu action fires,
 * so file-tree.jsx's handleActivate can ignore the tree-library-driven
 * activation that comes in on the same tick.
 */
export const contextMenuState = { active: false };
