import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * Office document preview (DOCX and XLSX).
 */
export function createDocxPreview() {
  const container = el('div', { class: 'preview-container office-preview docx-preview' });
  const content = el('div', { class: 'office-preview-content' });
  const info = el('div', { class: 'preview-info' });

  container.appendChild(content);
  container.appendChild(info);

  async function load(path) {
    content.innerHTML = '<div class="preview-loading">Loading document...</div>';

    try {
      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }

      const docxPreview = await import('docx-preview');
      content.innerHTML = '';
      await docxPreview.renderAsync(bytes.buffer, content, null, {
        className: 'docx-rendered',
        inWrapper: true,
        ignoreWidth: false,
        ignoreHeight: false,
        ignoreFonts: false,
        breakPages: true,
        ignoreLastRenderedPageBreak: true,
        experimental: false,
      });

      info.textContent = formatSize(result.size);
    } catch (e) {
      content.innerHTML = `<div class="preview-error">Failed to render document: ${e}</div>`;
    }
  }

  function destroy() {
    content.innerHTML = '';
  }

  return { element: container, load, destroy };
}

export function createXlsxPreview() {
  const container = el('div', { class: 'preview-container office-preview xlsx-preview' });
  const toolbar = el('div', { class: 'preview-toolbar' });
  const content = el('div', { class: 'office-preview-content xlsx-content' });
  const info = el('div', { class: 'preview-info' });

  container.appendChild(toolbar);
  container.appendChild(content);
  container.appendChild(info);

  let workbook = null;
  let XLSX = null;

  async function renderSheet(sheetName) {
    if (!workbook || !XLSX) return;

    const sheet = workbook.Sheets[sheetName];
    const html = XLSX.utils.sheet_to_html(sheet, { editable: false });
    content.innerHTML = html;

    // Style the generated table
    const table = content.querySelector('table');
    if (table) {
      table.classList.add('xlsx-table');
    }
  }

  async function load(path) {
    content.innerHTML = '<div class="preview-loading">Loading spreadsheet...</div>';
    toolbar.innerHTML = '';

    try {
      XLSX = await import('xlsx');
      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);

      workbook = XLSX.read(binaryStr, { type: 'binary' });

      // Create sheet tabs
      workbook.SheetNames.forEach((name, i) => {
        const tab = el('button', {
          class: `preview-toolbar-btn xlsx-sheet-tab${i === 0 ? ' active' : ''}`,
        }, name);
        tab.addEventListener('click', () => {
          toolbar.querySelectorAll('.xlsx-sheet-tab').forEach(t => t.classList.remove('active'));
          tab.classList.add('active');
          renderSheet(name);
        });
        toolbar.appendChild(tab);
      });

      // Render first sheet
      if (workbook.SheetNames.length > 0) {
        await renderSheet(workbook.SheetNames[0]);
      }

      info.textContent = `${workbook.SheetNames.length} sheet${workbook.SheetNames.length !== 1 ? 's' : ''}  \u2022  ${formatSize(result.size)}`;
    } catch (e) {
      content.innerHTML = `<div class="preview-error">Failed to render spreadsheet: ${e}</div>`;
    }
  }

  function destroy() {
    content.innerHTML = '';
    toolbar.innerHTML = '';
    workbook = null;
  }

  return { element: container, load, destroy };
}

export function createUnsupportedPreview() {
  const container = el('div', { class: 'preview-container unsupported-preview' });

  function load(path, fileType) {
    container.innerHTML = '';
    const msg = el('div', { class: 'unsupported-message' });
    msg.appendChild(el('div', { class: 'unsupported-icon' }, '\u26a0'));
    msg.appendChild(el('div', { class: 'unsupported-text' }, `Preview not available for ${fileType.toUpperCase()} files`));
    msg.appendChild(el('div', { class: 'unsupported-hint' }, 'This file can be opened in an external application.'));
    container.appendChild(msg);
  }

  function destroy() {
    container.innerHTML = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
