/**
 * Shared drag state for in-app drag and drop.
 *
 * WebView2 (Tauri on Windows) can fail to expose custom MIME types
 * in dataTransfer.types during dragover events. This module provides
 * a reliable alternative by tracking the active drag type in memory.
 */

let activeDragType = null; // 'tab' | 'file' | 'external' | null

export function setDragType(type) {
  if (activeDragType === type) return;
  activeDragType = type;
}

export function clearDragType() {
  if (activeDragType === null) return;
  activeDragType = null;
}

export function getDragType() {
  return activeDragType;
}
