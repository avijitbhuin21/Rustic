import { el } from '../../utils/dom.js';
import {
  editorStore,
  registerPreviewSaveHandler,
  unregisterPreviewSaveHandler,
  setBufferModified,
} from '../../state/editor.js';
import { createImagePreview } from './previews/image-preview.js';
import { createMediaPreview } from './previews/media-preview.js';
import { createPdfPreview } from './previews/pdf-preview.js';
import { createMarkdownPreview } from './previews/markdown-preview.js';
import { createHtmlPreview } from './previews/html-preview.js';
import { createSvgPreview } from './previews/svg-preview.js';
import { createHexPreview } from './previews/hex-preview.js';
import { createDocxPreview, createXlsxPreview, createUnsupportedPreview } from './previews/office-preview.js';
import { createDiffPreview } from './previews/diff-preview.js';

/**
 * File preview router. Shows the appropriate preview component based on file type.
 */
export function createFilePreview() {
  const container = el('div', { class: 'file-preview-container' });
  container.style.display = 'none';

  // Cache of preview instances to avoid recreating them on every tab switch
  let activePreview = null;
  let activeBufferId = null;

  function getPreviewForType(fileType, buffer) {
    switch (fileType) {
      case 'image': return createImagePreview();
      case 'svg': return createSvgPreview();
      case 'video':
      case 'audio': return createMediaPreview();
      case 'pdf': return createPdfPreview();
      case 'markdown': return createMarkdownPreview();
      case 'html': return createHtmlPreview();
      case 'binary': return createHexPreview();
      case 'docx': return createDocxPreview();
      case 'xlsx': return createXlsxPreview({
        onDirtyChange: (dirty) => setBufferModified(buffer.id, dirty),
      });
      case 'diff': return createDiffPreview();
      case 'pptx':
      default: return createUnsupportedPreview();
    }
  }

  function show(buffer) {
    // No-op when the same buffer is already showing. Required because
    // editor-group resubscribes to openBuffers and would otherwise destroy
    // and re-create the preview on every dirty-flag flip.
    if (activeBufferId === buffer.id && activePreview) {
      container.style.display = 'flex';
      return;
    }

    // Destroy previous preview UI. We deliberately do NOT unregister its
    // save handler here — the previous buffer may still be open in another
    // tab, and its preview retains in-memory state so the close-with-unsaved
    // dialog can still flush pending edits. The handler is unregistered
    // when the buffer itself is fully closed (see editor.js#closeBuffer).
    if (activePreview) {
      activePreview.destroy();
      container.innerHTML = '';
      activePreview = null;
    }

    activeBufferId = buffer.id;

    const preview = getPreviewForType(buffer.fileType, buffer);
    activePreview = preview;
    container.appendChild(preview.element);
    container.style.display = 'flex';

    // Wire up save-handler for previews that own their write path. This
    // replaces any previously-registered handler for the same buffer.
    if (preview.save) {
      registerPreviewSaveHandler(buffer.id, preview.save);
    }

    // Load the file
    if (buffer.fileType === 'diff') {
      preview.load(buffer.diffData);
    } else if (buffer.fileType === 'video' || buffer.fileType === 'audio') {
      preview.load(buffer.filePath, buffer.fileType);
    } else if (buffer.fileType === 'pptx') {
      preview.load(buffer.filePath, buffer.fileType);
    } else {
      preview.load(buffer.filePath);
    }
  }

  function hide() {
    if (activePreview) {
      activePreview.destroy();
      container.innerHTML = '';
      activePreview = null;
    }
    activeBufferId = null;
    container.style.display = 'none';
  }

  function isActive() {
    return container.style.display !== 'none';
  }

  return { element: container, show, hide, isActive };
}
