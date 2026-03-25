import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { getFileType, isPreviewType } from '../utils/file-types.js';

export const editorStore = createStore({
  openBuffers: {},      // bufferId -> { id, filePath, fileName, projectName, lineCount, language, isModified, fileType, isPreview }
  activeBufferId: null,
  cursorLine: 0,
  cursorCol: 0,
  scrollTop: 0,
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
  };

  const newBuffers = { ...editorStore.getState('openBuffers'), [id]: buffer };
  editorStore.setState({ openBuffers: newBuffers });
  setActiveBuffer(id);
  return buffer;
}

export async function closeBuffer(bufferId) {
  const buffers = { ...editorStore.getState('openBuffers') };
  const buffer = buffers[bufferId];

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

export async function saveActiveBuffer() {
  const bufferId = editorStore.getState('activeBufferId');
  if (bufferId === null) return;

  // Don't save preview files
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

export function updateBufferModified(bufferId, isModified, lineCount) {
  const buffers = { ...editorStore.getState('openBuffers') };
  if (buffers[bufferId]) {
    buffers[bufferId] = { ...buffers[bufferId], isModified, lineCount };
    editorStore.setState({ openBuffers: buffers });
  }
}
