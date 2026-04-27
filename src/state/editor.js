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

// Track file paths currently being opened to prevent duplicate async opens.
// When a file open is in-flight, subsequent clicks for the same path are ignored
// until the first one completes and adds the buffer to openBuffers.
const openingInFlight = new Set();

export async function openFile(filePath, projectName, targetGroupId) {
  // Check if already open in any group — switch to existing tab
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    if (buf.filePath === filePath) {
      setActiveBuffer(buf.id, targetGroupId);
      return buf;
    }
  }

  // Prevent duplicate async opens — if this file is already being loaded, bail out
  if (openingInFlight.has(filePath)) return null;
  openingInFlight.add(filePath);

  try {
    return await _openFileInner(filePath, projectName, targetGroupId);
  } finally {
    openingInFlight.delete(filePath);
  }
}

async function _openFileInner(filePath, projectName, targetGroupId) {
  const fileType = getFileType(filePath);
  const dualMode = isDualMode(fileType);

  if (dualMode) {
    // Dual-mode files (markdown, html, svg) — open as backend buffer but support preview toggle
    return openDualModeFile(filePath, projectName, fileType, targetGroupId);
  }

  if (isPreviewType(fileType)) {
    return openPreviewFile(filePath, projectName, fileType, targetGroupId);
  }

  // Text/code file — use existing backend buffer flow
  try {
    const info = await api.openFile(filePath);
    if (!info) return null;

    // Re-check after await — another call may have added it while we were loading
    const currentBuffers = editorStore.getState('openBuffers');
    if (currentBuffers[info.id]) {
      setActiveBuffer(info.id, targetGroupId);
      return currentBuffers[info.id];
    }

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
    setActiveBuffer(info.id, targetGroupId);
    return buffer;
  } catch (e) {
    console.error('Failed to open file:', e);
    return null;
  }
}

async function openDualModeFile(filePath, projectName, fileType, targetGroupId) {
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
    setActiveBuffer(info.id, targetGroupId);
    return buffer;
  } catch (e) {
    console.error('Failed to open dual-mode file:', e);
    return null;
  }
}

function openPreviewFile(filePath, projectName, fileType, targetGroupId) {
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
  setActiveBuffer(id, targetGroupId);
  return buffer;
}

/**
 * Open a diff view as a virtual preview buffer.
 * diffData: { projectId, filePath, oid? (for commit diffs), isStaged? (for staged diffs), unifiedDiff? (pre-computed) }
 */
export function openDiffView(diffData) {
  const { projectId, filePath, oid, isStaged, unifiedDiff } = diffData;

  // Create a unique key for this diff
  const diffKey = unifiedDiff
    ? `diff:agent:${filePath}`
    : oid ? `diff:${oid}:${filePath}` : `diff:${isStaged ? 'staged' : 'working'}:${filePath}`;

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
    diffData: { projectId, filePath, oid, isStaged, unifiedDiff },
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

export async function closeBuffer(bufferId, { force = false, groupId } = {}) {
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
  }

  // Remove from only the target group (or the active group if not specified)
  const targetGroupId = groupId || editorStore.getState('activeGroupId');
  const groups = editorStore.getState('groups').map(g => {
    if (g.id !== targetGroupId) return g;
    const newBufferIds = g.bufferIds.filter(id => id !== bufferId);
    const newActiveId = g.activeBufferId === bufferId
      ? (newBufferIds.length > 0 ? newBufferIds[newBufferIds.length - 1] : null)
      : g.activeBufferId;
    bufferViewState.delete(`${g.id}:${bufferId}`);
    return { ...g, bufferIds: newBufferIds, activeBufferId: newActiveId };
  });

  // Check if any other group still references this buffer
  const stillReferenced = groups.some(g => g.bufferIds.includes(bufferId));

  // Only truly close the buffer if no group references it anymore
  if (!stillReferenced) {
    if (buffer && !buffer.isPreview) {
      try {
        await api.closeBuffer(bufferId);
      } catch (e) {
        console.error('Failed to close buffer:', e);
      }
    }
    delete buffers[bufferId];
  }

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

/**
 * Move a buffer from one group to another.
 * If the buffer already exists in the target group, just activate it there.
 * If the source group becomes empty, close it.
 */
export function moveBufferToGroup(bufferId, fromGroupId, toGroupId) {
  if (fromGroupId === toGroupId) return;

  let groups = editorStore.getState('groups').map(g => {
    if (g.id === fromGroupId) {
      // Remove from source group
      const newBufferIds = g.bufferIds.filter(id => id !== bufferId);
      const newActiveId = g.activeBufferId === bufferId
        ? (newBufferIds.length > 0 ? newBufferIds[newBufferIds.length - 1] : null)
        : g.activeBufferId;
      return { ...g, bufferIds: newBufferIds, activeBufferId: newActiveId };
    }
    if (g.id === toGroupId) {
      // Add to target group (if not already there)
      const bufferIds = g.bufferIds.includes(bufferId) ? g.bufferIds : [...g.bufferIds, bufferId];
      return { ...g, bufferIds, activeBufferId: bufferId };
    }
    return g;
  });

  // Remove empty groups (except the last one)
  const nonEmpty = groups.filter(g => g.bufferIds.length > 0);
  groups = nonEmpty.length > 0 ? nonEmpty : [groups[0]];

  const activeGroup = groups.find(g => g.id === toGroupId) || groups[0];
  editorStore.setState({
    groups,
    activeGroupId: activeGroup.id,
    activeBufferId: bufferId,
  });
}

async function saveBuffer(bufferId) {
  const buffers = editorStore.getState('openBuffers');
  const buffer = buffers[bufferId];
  if (!buffer || buffer.isPreview) return;

  try {
    // Format on save if enabled
    const { settingsStore } = await import('./settings.js');
    const settings = settingsStore.getState('settings');
    const formatOnSave = settings?.editor?.format_on_save ?? true;
    console.log(`[Formatter] format_on_save=${formatOnSave}, bufferId=${bufferId}`);
    if (formatOnSave) {
      const indentSize = settings?.editor?.tab_size || 4;
      try {
        const newLineCount = await api.formatBuffer(bufferId, indentSize);
        console.log(`[Formatter] formatBuffer returned: ${JSON.stringify(newLineCount)}`);
        if (newLineCount !== null && newLineCount !== undefined) {
          console.log(`[Formatter] formatted buffer ${bufferId}, new line count: ${newLineCount}`);
          // Notify the editor pane to reload content
          editorStore.setState({ _formatEvent: { bufferId, lineCount: newLineCount, ts: Date.now() } });
        } else {
          console.log(`[Formatter] no changes needed for buffer ${bufferId}`);
        }
      } catch (e) {
        console.warn('[Formatter] format failed, saving without formatting:', e);
      }
    }

    try {
      await api.saveFile(bufferId);
    } catch (e) {
      // Backend signals an external on-disk change with this sentinel string.
      // Prompt the user to overwrite, reload-from-disk, or cancel.
      if (typeof e === 'string' && e.includes('EXTERNAL_CHANGE_DETECTED')) {
        const { showConfirmDialog } = await import('../components/confirm-dialog.js');
        const overwrite = await showConfirmDialog(
          'File changed on disk',
          `${buffer.fileName} was modified on disk since you opened it. Overwrite the on-disk version with your changes?`,
          { confirmLabel: 'Overwrite', cancelLabel: 'Cancel', danger: true }
        );
        if (!overwrite) {
          const { showToast } = await import('../components/toast.js');
          showToast(`Save cancelled — ${buffer.fileName} unchanged on disk`, { kind: 'info' });
          return;
        }
        await api.saveFile(bufferId, true);
      } else {
        throw e;
      }
    }
    const updatedBuffers = { ...editorStore.getState('openBuffers') };
    if (updatedBuffers[bufferId]) {
      updatedBuffers[bufferId] = { ...updatedBuffers[bufferId], isModified: false };
      editorStore.setState({ openBuffers: updatedBuffers });
    }
  } catch (e) {
    console.error('Failed to save:', e);
    const { showErrorToast } = await import('../components/toast.js');
    showErrorToast(`Save failed (${buffers[bufferId]?.fileName || 'file'})`, e);
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

/**
 * Detect on-disk changes for any open buffer and prompt the user to reload.
 * Called from the fs-change watcher and on window focus. Cheap: for each
 * open file-backed buffer, calls `buffer_external_change` (single stat) and
 * surfaces a toast with a Reload action when the mtime changed.
 *
 * Optionally pass `withinDirs` to limit the check to buffers whose path is
 * under one of the given directories (the watcher payload provides them).
 */
const externalChangePromptedFor = new Set();
export async function checkOpenBuffersForExternalChanges(withinDirs = null) {
  const buffers = editorStore.getState('openBuffers') || {};
  const norm = (p) => (p || '').replace(/\\/g, '/');

  let candidates = Object.values(buffers).filter((b) => b && b.filePath && !b.isPreview);
  if (withinDirs && withinDirs.length > 0) {
    const normDirs = withinDirs.map(norm);
    candidates = candidates.filter((b) => {
      const np = norm(b.filePath);
      return normDirs.some((d) => np === d || np.startsWith(d + '/'));
    });
  }

  for (const buf of candidates) {
    let changed = false;
    try {
      changed = await api.bufferExternalChange(buf.id);
    } catch {
      continue;
    }
    if (!changed) continue;

    // Don't re-prompt for the same buffer in a tight burst; cleared on focus.
    if (externalChangePromptedFor.has(buf.id)) continue;
    externalChangePromptedFor.add(buf.id);

    const { showToast } = await import('../components/toast.js');
    const isDirty = !!buf.isModified;
    showToast(
      isDirty
        ? `${buf.fileName} changed on disk (you have unsaved edits)`
        : `${buf.fileName} changed on disk`,
      {
        kind: 'warning',
        duration: 0,
        action: 'Reload',
        onAction: async () => {
          try {
            const info = await api.reloadBuffer(buf.id);
            const updatedBuffers = { ...editorStore.getState('openBuffers') };
            if (updatedBuffers[buf.id]) {
              updatedBuffers[buf.id] = {
                ...updatedBuffers[buf.id],
                lineCount: info.line_count,
                isModified: info.is_modified,
              };
              editorStore.setState({ openBuffers: updatedBuffers });
            }
            externalChangePromptedFor.delete(buf.id);
          } catch (e) {
            const { showErrorToast } = await import('../components/toast.js');
            showErrorToast(`Reload failed (${buf.fileName})`, e);
          }
        },
      },
    );
  }
}

/** Allow the next fs-change tick to re-prompt for previously-flagged buffers. */
export function clearExternalChangePromptedSet() {
  externalChangePromptedFor.clear();
}

/**
 * Close all open buffers whose filePath matches or is under the given path.
 * Used when a file or directory is deleted so stale tabs are removed.
 */
export async function closeBuffersForPath(deletedPath) {
  const normalize = (p) => p.replace(/\\/g, '/');
  const norm = normalize(deletedPath);
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    const bufPath = normalize(buf.filePath || '');
    if (bufPath === norm || bufPath.startsWith(norm + '/')) {
      await closeBuffer(buf.id, { force: true });
    }
  }
}

export function updateBufferModified(bufferId, isModified, lineCount) {
  const buffers = { ...editorStore.getState('openBuffers') };
  if (buffers[bufferId]) {
    buffers[bufferId] = { ...buffers[bufferId], isModified, lineCount };
    editorStore.setState({ openBuffers: buffers });
  }
}
