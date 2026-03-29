/**
 * Shared drag state for in-app drag and drop.
 *
 * WebView2 (Tauri on Windows) can fail to expose custom MIME types
 * in dataTransfer.types during dragover events. This module provides
 * a reliable alternative by tracking the active drag type in memory.
 */

const PREFIX = '[DnD]';

let activeDragType = null; // 'tab' | 'file' | 'external' | null

export function setDragType(type) {
  if (activeDragType === type) return; // avoid log spam from repeated calls
  const prev = activeDragType;
  activeDragType = type;
  console.log(`${PREFIX} setDragType: "${prev}" → "${type}"`);
}

export function clearDragType() {
  if (activeDragType === null) return; // already clear
  console.log(`${PREFIX} clearDragType (was "${activeDragType}")`);
  activeDragType = null;
}

export function getDragType() {
  return activeDragType;
}
