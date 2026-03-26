import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { getFileType, isPreviewType, isDualMode, getDefaultViewMode } from '../utils/file-types.js';

export const editorStore = createStore({
  openBuffers: {},      // bufferId -> { id, filePath, fileName, projectName, lineCount, language, isModified, fileType, isPreview, isDualMode, viewMode }
  activeBufferId: null,
  cursorLine: 0,
  cursorCol: 0,
  scrollTop: 0,
  pendingGoto: null,    // { line, col } — set after opening file from search, consumed by editor pane
});

// Per-buffer state (cursor pos, scroll pos) saved when switching tabs
const bufferViewState = new Map();

// Counter for preview-only files (no backend buffer). Use negative IDs to avoid collision.
let previewIdCounter = -1;

export async function openFile(filePath, projectName) {
  // Check if already open
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    if (buf.filePath === filePath) {
      setActiveBuffer(buf.id);
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
  bufferViewState.delete(bufferId);

  const activeId = editorStore.getState('activeBufferId');
  let newActiveId = null;

  if (activeId === bufferId) {
    const ids = Object.keys(buffers);
    newActiveId = ids.length > 0 ? Number(ids[ids.length - 1]) : null;
  } else {
    newActiveId = activeId;
  }

  editorStore.setState({
    openBuffers: buffers,
    activeBufferId: newActiveId,
  });
}

export function setActiveBuffer(bufferId) {
  // Save current view state
  const currentId = editorStore.getState('activeBufferId');
  if (currentId !== null) {
    bufferViewState.set(currentId, {
      cursorLine: editorStore.getState('cursorLine'),
      cursorCol: editorStore.getState('cursorCol'),
      scrollTop: editorStore.getState('scrollTop'),
    });
  }

  // Restore view state for new buffer
  const saved = bufferViewState.get(bufferId);
  editorStore.setState({
    activeBufferId: bufferId,
    cursorLine: saved?.cursorLine ?? 0,
    cursorCol: saved?.cursorCol ?? 0,
    scrollTop: saved?.scrollTop ?? 0,
  });
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

export function updateBufferModified(bufferId, isModified, lineCount) {
  const buffers = { ...editorStore.getState('openBuffers') };
  if (buffers[bufferId]) {
    buffers[bufferId] = { ...buffers[bufferId], isModified, lineCount };
    editorStore.setState({ openBuffers: buffers });
  }
}
