import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

const BYTES_PER_ROW = 16;
const ROWS_PER_CHUNK = 64;
const CHUNK_SIZE = BYTES_PER_ROW * ROWS_PER_CHUNK; // 1024 bytes per chunk

/**
 * Hex viewer component for binary files.
 */
export function createHexPreview() {
  const container = el('div', { class: 'preview-container hex-preview' });
  const toolbar = el('div', { class: 'preview-toolbar' });
  const hexContainer = el('div', { class: 'hex-container' });
  const info = el('div', { class: 'preview-info' });

  let filePath = null;
  let totalSize = 0;
  let currentOffset = 0;

  // Toolbar navigation
  const startBtn = el('button', { class: 'preview-toolbar-btn', title: 'Go to Start' }, '\u23ee');
  const prevBtn = el('button', { class: 'preview-toolbar-btn', title: 'Previous Page' }, '\u25c0');
  const offsetLabel = el('span', { class: 'preview-toolbar-label' }, 'Offset: 0x00000000');
  const nextBtn = el('button', { class: 'preview-toolbar-btn', title: 'Next Page' }, '\u25b6');
  const endBtn = el('button', { class: 'preview-toolbar-btn', title: 'Go to End' }, '\u23ed');

  toolbar.appendChild(startBtn);
  toolbar.appendChild(prevBtn);
  toolbar.appendChild(offsetLabel);
  toolbar.appendChild(nextBtn);
  toolbar.appendChild(endBtn);

  container.appendChild(toolbar);
  container.appendChild(hexContainer);
  container.appendChild(info);

  startBtn.addEventListener('click', () => goTo(0));
  prevBtn.addEventListener('click', () => goTo(currentOffset - CHUNK_SIZE));
  nextBtn.addEventListener('click', () => goTo(currentOffset + CHUNK_SIZE));
  endBtn.addEventListener('click', () => {
    const lastPage = Math.max(0, totalSize - CHUNK_SIZE);
    goTo(Math.floor(lastPage / BYTES_PER_ROW) * BYTES_PER_ROW);
  });

  // Keyboard navigation
  container.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' || e.key === 'PageDown') {
      e.preventDefault();
      goTo(currentOffset + CHUNK_SIZE);
    } else if (e.key === 'ArrowUp' || e.key === 'PageUp') {
      e.preventDefault();
      goTo(currentOffset - CHUNK_SIZE);
    } else if (e.key === 'Home') {
      e.preventDefault();
      goTo(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      endBtn.click();
    }
  });

  container.setAttribute('tabindex', '0');

  async function goTo(offset) {
    offset = Math.max(0, Math.min(offset, Math.max(0, totalSize - 1)));
    currentOffset = offset;
    await renderChunk();
  }

  async function renderChunk() {
    try {
      const result = await api.readHexChunk(filePath, currentOffset, CHUNK_SIZE);
      totalSize = Number(result.total_size);
      const bytesRead = result.bytes_read;

      hexContainer.innerHTML = '';

      // Header row
      const header = el('div', { class: 'hex-row hex-header' });
      let headerText = '  Offset  ';
      for (let i = 0; i < BYTES_PER_ROW; i++) {
        headerText += ` ${i.toString(16).toUpperCase().padStart(2, '0')}`;
      }
      headerText += '  ASCII';
      header.textContent = headerText;
      hexContainer.appendChild(header);

      // Data rows
      for (let row = 0; row < Math.ceil(bytesRead / BYTES_PER_ROW); row++) {
        const rowOffset = currentOffset + row * BYTES_PER_ROW;
        const rowStart = row * BYTES_PER_ROW;
        const rowEnd = Math.min(rowStart + BYTES_PER_ROW, bytesRead);

        const rowEl = el('div', { class: 'hex-row' });

        // Offset column
        const offsetStr = rowOffset.toString(16).toUpperCase().padStart(8, '0');

        // Hex columns
        let hexStr = '';
        for (let i = rowStart; i < rowStart + BYTES_PER_ROW; i++) {
          if (i < rowEnd) {
            hexStr += ` ${result.hex[i]}`;
          } else {
            hexStr += '   ';
          }
        }

        // ASCII column
        let asciiStr = '';
        for (let i = rowStart; i < rowEnd; i++) {
          asciiStr += result.ascii[i];
        }

        rowEl.textContent = `${offsetStr} ${hexStr}  ${asciiStr}`;
        hexContainer.appendChild(rowEl);
      }

      // Update toolbar
      offsetLabel.textContent = `Offset: 0x${currentOffset.toString(16).toUpperCase().padStart(8, '0')}`;
      prevBtn.disabled = currentOffset <= 0;
      nextBtn.disabled = currentOffset + CHUNK_SIZE >= totalSize;
      startBtn.disabled = currentOffset <= 0;
      endBtn.disabled = currentOffset + CHUNK_SIZE >= totalSize;

      info.textContent = `${formatSize(totalSize)}  \u2022  Showing bytes ${currentOffset}\u2013${Math.min(currentOffset + bytesRead, totalSize) - 1}`;
    } catch (e) {
      hexContainer.innerHTML = `<div class="preview-error">Failed to read file: ${e}</div>`;
    }
  }

  async function load(path) {
    filePath = path;
    currentOffset = 0;
    hexContainer.innerHTML = '<div class="preview-loading">Loading...</div>';

    try {
      const sizeResult = await api.getFileSize(path);
      totalSize = sizeResult.size;
      info.textContent = formatSize(totalSize);
      await renderChunk();
      container.focus();
    } catch (e) {
      info.textContent = `Failed to load file: ${e}`;
    }
  }

  function destroy() {
    hexContainer.innerHTML = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
