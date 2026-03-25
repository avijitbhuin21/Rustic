import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';

export const editorStore = createStore({
  openBuffers: {},      // bufferId -> { id, filePath, fileName, projectName, lineCount, language, isModified }
  activeBufferId: null,
  cursorLine: 0,
  cursorCol: 0,
  scrollTop: 0,
});

// Per-buffer state (cursor pos, scroll pos) saved when switching tabs
const bufferViewState = new Map();

export async function openFile(filePath, projectName) {
  // Check if already open
  const buffers = editorStore.getState('openBuffers');
  for (const buf of Object.values(buffers)) {
    if (buf.filePath === filePath) {
      setActiveBuffer(buf.id);
      return buf;
    }
  }

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

export async function closeBuffer(bufferId) {
  try {
    await api.closeBuffer(bufferId);
  } catch (e) {
    console.error('Failed to close buffer:', e);
  }

  const buffers = { ...editorStore.getState('openBuffers') };
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

  try {
    await api.saveFile(bufferId);
    const buffers = { ...editorStore.getState('openBuffers') };
    if (buffers[bufferId]) {
      buffers[bufferId] = { ...buffers[bufferId], isModified: false };
      editorStore.setState({ openBuffers: buffers });
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
