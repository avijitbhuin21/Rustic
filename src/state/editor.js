import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { getFileType, isPreviewType, isDualMode, getDefaultViewMode } from '../utils/file-types.js';

export const SETTINGS_BUFFER_ID = -9999;

let groupIdCounter = 1;

export const editorStore = createStore({
  openBuffers: {},      // bufferId -> { id, filePath, fileName, projectName, lineCount, language, isModified, fileType, isPreview, isDualMode, viewMode }
  activeBufferId: null,
  cursorLine: 0,
  cursorCol: 0,
  scrollTop: 0,
  pendingGoto: null,    // { line, col } — set after opening file from search, consumed by editor pane
  // Split view groups
  groups: [{ id: 1, bufferIds: [], activeBufferId: null }],
  activeGroupId: 1,
});

// Per-buffer state (cursor pos, scroll pos) saved when switching tabs
// Keys are `${groupId}:${bufferId}` for split-aware state, or just bufferId for legacy
const bufferViewState = new Map();

// Counter for preview-only files (no backend buffer). Use negative IDs to avoid collision.
let previewIdCounter = -1;

export async function openFile(filePath, projectName, targetGroupId) {
  // Check if already open in any group
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    if (buf.filePath === filePath) {
      setActiveBuffer(buf.id, targetGroupId);
      return buf;
    }
  }

  const fileType = getFileType(filePath);
  const dualMode = isDualMode(fileType);

  if (dualMode) {
    // Dual-mode files (markdown, html, svg) — open as backend buffer but support preview toggle
    return openDualModeFile(filePath, projectName, fileType);
  }

  if (isPreviewType(fileType)) {
    return openPreviewFile(filePath, projectName, fileType);
  }

  // Text/code file — use existing backend buffer flow
  try {
    const info = await api.openFile(filePath);
    if (!info) return null;

    const buffer = {
      id: info.id,
      filePath: info.file_path,
      fileName: info.file_name,
      projectName: projectName || '',
      lineCount: info.line_count,
      language: info.language,
      isModified: info.is_modified,
      fileType: 'code',
      isPreview: false,
      isDualMode: false,
      viewMode: 'edit',
    };

    const newBuffers = { ...editorStore.getState('openBuffers'), [info.id]: buffer };
    editorStore.setState({ openBuffers: newBuffers });
    setActiveBuffer(info.id);
    return buffer;
  } catch (e) {
    console.error('Failed to open file:', e);
    return null;
  }
}

async function openDualModeFile(filePath, projectName, fileType) {
  try {
    const info = await api.openFile(filePath);
    if (!info) return null;

    const defaultView = getDefaultViewMode(fileType);

    const buffer = {
      id: info.id,
      filePath: info.file_path,
      fileName: info.file_name,
      projectName: projectName || '',
      lineCount: info.line_count,
      language: info.language,
      isModified: info.is_modified,
      fileType,
      isPreview: false,
      isDualMode: true,
      viewMode: defaultView,
    };

    const newBuffers = { ...editorStore.getState('openBuffers'), [info.id]: buffer };
    editorStore.setState({ openBuffers: newBuffers });
    setActiveBuffer(info.id);
    return buffer;
  } catch (e) {
    console.error('Failed to open dual-mode file:', e);
    return null;
  }
}

function openPreviewFile(filePath, projectName, fileType) {
  const id = previewIdCounter--;
  const parts = filePath.split(/[/\\]/);
  const fileName = parts[parts.length - 1];

  const buffer = {
    id,
    filePath,
    fileName,
    projectName: projectName || '',
    lineCount: 0,
    language: null,
    isModified: false,
    fileType,
    isPreview: true,
    isDualMode: false,
    viewMode: 'preview',
  };

  const newBuffers = { ...editorStore.getState('openBuffers'), [id]: buffer };
  editorStore.setState({ openBuffers: newBuffers });
  setActiveBuffer(id);
  return buffer;
}

/**
 * Open a diff view as a virtual preview buffer.
 * diffData: { projectId, filePath, oid? (for commit diffs), isStaged? (for staged diffs) }
 */
export function openDiffView(diffData) {
  const { projectId, filePath, oid, isStaged } = diffData;

  // Create a unique key for this diff
  const diffKey = oid ? `diff:${oid}:${filePath}` : `diff:${isStaged ? 'staged' : 'working'}:${filePath}`;

  // Check if already open
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    if (buf.diffKey === diffKey) {
      setActiveBuffer(buf.id);
      return buf;
    }
  }

  const id = previewIdCounter--;
  const parts = filePath.split(/[/\\]/);
  const fileName = parts[parts.length - 1];
  const label = oid ? `${fileName} (${oid.substring(0, 7)})` : fileName;

  const buffer = {
    id,
    filePath,
    fileName: label,
    projectName: '',
    lineCount: 0,
    language: null,
    isModified: false,
    fileType: 'diff',
    isPreview: true,
    isDualMode: false,
    viewMode: 'preview',
    diffKey,
    diffData: { projectId, filePath, oid, isStaged },
  };

  const newBuffers = { ...editorStore.getState('openBuffers'), [id]: buffer };
  editorStore.setState({ openBuffers: newBuffers });
  setActiveBuffer(id);
  return buffer;
}

/**
 * Toggle view mode for a dual-mode buffer between 'edit' and 'preview'.
 */
export function toggleViewMode(bufferId) {
  const buffers = { ...editorStore.getState('openBuffers') };
  const buf = buffers[bufferId];
  if (!buf || !buf.isDualMode) return;

  const newMode = buf.viewMode === 'edit' ? 'preview' : 'edit';
  buffers[bufferId] = { ...buf, viewMode: newMode };
  editorStore.setState({ openBuffers: buffers });
}

/**
 * Set view mode for a buffer explicitly.
 */
export function setViewMode(bufferId, mode) {
  const buffers = { ...editorStore.getState('openBuffers') };
  const buf = buffers[bufferId];
  if (!buf || !buf.isDualMode) return;

  buffers[bufferId] = { ...buf, viewMode: mode };
  editorStore.setState({ openBuffers: buffers });
}

export async function openFileAtLine(filePath, projectName, line, col = 0) {
  const buf = await openFile(filePath, projectName);
  if (buf) {
    // line from search results is 1-based; editor uses 0-based
    editorStore.setState({ pendingGoto: { line: line - 1, col } });
  }
  return buf;
}

export async function closeBuffer(bufferId, { force = false } = {}) {
  // Settings buffer — delegate to closeSettings
  if (bufferId === SETTINGS_BUFFER_ID) {
    const { closeSettings } = await import('./settings.js');
    closeSettings();
    return;
  }

  const buffers = { ...editorStore.getState('openBuffers') };
  const buffer = buffers[bufferId];

  // Prompt for unsaved changes unless forced
  if (!force && buffer && buffer.isModified) {
    const { showUnsavedDialog } = await import('../components/confirm-dialog.js');
    const result = await showUnsavedDialog(buffer.fileName);
    if (result === 'cancel') return;
    if (result === 'save') {
      await saveBuffer(bufferId);
    }
    // 'discard' falls through to close without saving
  }

  // Only call backend close for non-preview buffers
  if (buffer && !buffer.isPreview) {
    try {
      await api.closeBuffer(bufferId);
    } catch (e) {
      console.error('Failed to close buffer:', e);
    }
  }

  delete buffers[bufferId];

  // Remove from all groups and clean up view state
  const groups = editorStore.getState('groups').map(g => {
    if (!g.bufferIds.includes(bufferId)) return g;
    const newBufferIds = g.bufferIds.filter(id => id !== bufferId);
    const newActiveId = g.activeBufferId === bufferId
      ? (newBufferIds.length > 0 ? newBufferIds[newBufferIds.length - 1] : null)
      : g.activeBufferId;
    bufferViewState.delete(`${g.id}:${bufferId}`);
    return { ...g, bufferIds: newBufferIds, activeBufferId: newActiveId };
  });

  // Remove empty groups (except the last one)
  const nonEmptyGroups = groups.filter(g => g.bufferIds.length > 0);
  const finalGroups = nonEmptyGroups.length > 0 ? nonEmptyGroups : [groups[0] || { id: 1, bufferIds: [], activeBufferId: null }];

  const activeGroupId = editorStore.getState('activeGroupId');
  const activeGroup = finalGroups.find(g => g.id === activeGroupId) || finalGroups[0];
  const newActiveBufferId = activeGroup.activeBufferId;

  const saved = newActiveBufferId ? (bufferViewState.get(`${activeGroup.id}:${newActiveBufferId}`) || {}) : {};
  editorStore.setState({
    openBuffers: buffers,
    groups: finalGroups,
    activeGroupId: activeGroup.id,
    activeBufferId: newActiveBufferId,
    cursorLine: saved.cursorLine ?? 0,
    cursorCol: saved.cursorCol ?? 0,
    scrollTop: saved.scrollTop ?? 0,
  });
}

export function setActiveBuffer(bufferId, groupId) {
  const groups = editorStore.getState('groups');
  const targetGroupId = groupId || editorStore.getState('activeGroupId');

  // Save current view state for the current group's active buffer
  const currentId = editorStore.getState('activeBufferId');
  const currentGroupId = editorStore.getState('activeGroupId');
  if (currentId !== null) {
    bufferViewState.set(`${currentGroupId}:${currentId}`, {
      cursorLine: editorStore.getState('cursorLine'),
      cursorCol: editorStore.getState('cursorCol'),
      scrollTop: editorStore.getState('scrollTop'),
    });
  }

  // Update the target group's activeBufferId and add buffer if not already in group
  const newGroups = groups.map(g => {
    if (g.id === targetGroupId) {
      const bufferIds = g.bufferIds.includes(bufferId)
        ? g.bufferIds
        : [...g.bufferIds, bufferId];
      return { ...g, bufferIds, activeBufferId: bufferId };
    }
    return g;
  });

  // Restore view state for new buffer in target group
  const saved = bufferViewState.get(`${targetGroupId}:${bufferId}`) || bufferViewState.get(bufferId);
  editorStore.setState({
    groups: newGroups,
    activeGroupId: targetGroupId,
    activeBufferId: bufferId,
    cursorLine: saved?.cursorLine ?? 0,
    cursorCol: saved?.cursorCol ?? 0,
    scrollTop: saved?.scrollTop ?? 0,
  });
}

export function setActiveGroup(groupId) {
  const groups = editorStore.getState('groups');
  const group = groups.find(g => g.id === groupId);
  if (!group) return;

  // Save current group's view state
  const currentId = editorStore.getState('activeBufferId');
  const currentGroupId = editorStore.getState('activeGroupId');
  if (currentId !== null) {
    bufferViewState.set(`${currentGroupId}:${currentId}`, {
      cursorLine: editorStore.getState('cursorLine'),
      cursorCol: editorStore.getState('cursorCol'),
      scrollTop: editorStore.getState('scrollTop'),
    });
  }

  // Restore target group's active buffer state
  const targetBufferId = group.activeBufferId;
  const saved = targetBufferId ? (bufferViewState.get(`${groupId}:${targetBufferId}`) || bufferViewState.get(targetBufferId)) : null;
  editorStore.setState({
    activeGroupId: groupId,
    activeBufferId: targetBufferId,
    cursorLine: saved?.cursorLine ?? 0,
    cursorCol: saved?.cursorCol ?? 0,
    scrollTop: saved?.scrollTop ?? 0,
  });
}

/**
 * Split the editor: create a new group to the right.
 * If bufferId is provided, opens that buffer in the new group.
 * Otherwise duplicates the current active buffer.
 */
export function splitRight(bufferId) {
  const groups = editorStore.getState('groups');
  const activeGroupId = editorStore.getState('activeGroupId');
  const targetBufferId = bufferId || editorStore.getState('activeBufferId');
  if (!targetBufferId) return;

  const newGroupId = ++groupIdCounter;
  const newGroup = { id: newGroupId, bufferIds: [targetBufferId], activeBufferId: targetBufferId };

  // Insert new group after the active group
  const idx = groups.findIndex(g => g.id === activeGroupId);
  const newGroups = [...groups];
  newGroups.splice(idx + 1, 0, newGroup);

  editorStore.setState({ groups: newGroups, activeGroupId: newGroupId, activeBufferId: targetBufferId });
}

/**
 * Close an editor group. Redistributes its buffers to the previous group.
 */
export function closeGroup(groupId) {
  const groups = editorStore.getState('groups');
  if (groups.length <= 1) return; // Can't close the last group

  const idx = groups.findIndex(g => g.id === groupId);
  const closingGroup = groups[idx];
  // Merge buffers into the nearest remaining group
  const targetIdx = idx > 0 ? idx - 1 : 1;
  const targetGroup = groups[targetIdx];

  const mergedBufferIds = [...new Set([...targetGroup.bufferIds, ...closingGroup.bufferIds])];
  const newGroups = groups
    .filter(g => g.id !== groupId)
    .map(g => g.id === targetGroup.id
      ? { ...g, bufferIds: mergedBufferIds }
      : g);

  const activeGroupId = editorStore.getState('activeGroupId');
  const newActiveGroupId = activeGroupId === groupId ? targetGroup.id : activeGroupId;

  editorStore.setState({ groups: newGroups, activeGroupId: newActiveGroupId });
  // Focus the target group
  setActiveGroup(newActiveGroupId);
}

async function saveBuffer(bufferId) {
  const buffers = editorStore.getState('openBuffers');
  const buffer = buffers[bufferId];
  if (!buffer || buffer.isPreview) return;

  try {
    await api.saveFile(bufferId);
    const updatedBuffers = { ...editorStore.getState('openBuffers') };
    if (updatedBuffers[bufferId]) {
      updatedBuffers[bufferId] = { ...updatedBuffers[bufferId], isModified: false };
      editorStore.setState({ openBuffers: updatedBuffers });
    }
  } catch (e) {
    console.error('Failed to save:', e);
  }
}

export async function saveActiveBuffer() {
  const bufferId = editorStore.getState('activeBufferId');
  if (bufferId === null) return;
  await saveBuffer(bufferId);
}

export async function saveAllBuffers() {
  const buffers = editorStore.getState('openBuffers');
  const promises = [];
  for (const buf of Object.values(buffers)) {
    if (buf.isModified && !buf.isPreview) {
      promises.push(saveBuffer(buf.id));
    }
  }
  await Promise.all(promises);
}

export function updateBufferModified(bufferId, isModified, lineCount) {
  const buffers = { ...editorStore.getState('openBuffers') };
  if (buffers[bufferId]) {
    buffers[bufferId] = { ...buffers[bufferId], isModified, lineCount };
    editorStore.setState({ openBuffers: buffers });
  }
}
