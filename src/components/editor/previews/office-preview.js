import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * Office document preview (DOCX and XLSX).
 */
export function createDocxPreview() {
  const container = el('div', { class: 'preview-container office-preview docx-preview' });
  const body = el('div', { class: 'docx-body' });
  const contentWrap = el('div', { class: 'docx-content-wrap' });
  const styleContainer = el('div', { class: 'docx-style-container' });

  body.appendChild(contentWrap);
  container.appendChild(styleContainer);
  container.appendChild(body);

  async function load(path) {
    contentWrap.innerHTML = '<div class="preview-loading">Loading document...</div>';

    try {
      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }

      const docx = await import('docx-preview');
      contentWrap.innerHTML = '';

      await docx.renderAsync(bytes.buffer, contentWrap, styleContainer, {
        className: 'docx-rendered',
        inWrapper: true,
        ignoreWidth: true,
        ignoreHeight: true,
        ignoreFonts: false,
        breakPages: true,
        ignoreLastRenderedPageBreak: true,
        experimental: false,
      });
    } catch (e) {
      contentWrap.innerHTML = `<div class="preview-error">Failed to render document: ${e}</div>`;
    }
  }

  function destroy() {
    contentWrap.innerHTML = '';
    styleContainer.innerHTML = '';
  }

  return { element: container, load, destroy };
}

export function createXlsxPreview() {
  const container = el('div', { class: 'preview-container office-preview xlsx-preview' });
  const sheetWrap = el('div', { class: 'xlsx-fortune-wrap' });
  const info = el('div', { class: 'preview-info' });

  container.appendChild(sheetWrap);
  container.appendChild(info);

  let reactRoot = null;

  function xlsxSheetToFortune(xlsxWorkbook, XLSX) {
    return xlsxWorkbook.SheetNames.map((name, idx) => {
      const ws = xlsxWorkbook.Sheets[name];
      const range = XLSX.utils.decode_range(ws['!ref'] || 'A1:A1');
      const celldata = [];

      for (let r = range.s.r; r <= range.e.r; r++) {
        for (let c = range.s.c; c <= range.e.c; c++) {
          const addr = XLSX.utils.encode_cell({ r, c });
          const cell = ws[addr];
          if (!cell) continue;

          const fortuneCell = { v: cell.v, m: cell.w != null ? String(cell.w) : (cell.v != null ? String(cell.v) : '') };
          if (cell.f) fortuneCell.f = '=' + cell.f;

          celldata.push({ r, c, v: fortuneCell });
        }
      }

      const colWidths = {};
      if (ws['!cols']) {
        ws['!cols'].forEach((col, i) => {
          if (col && col.wch) colWidths[i] = Math.round(col.wch * 7);
        });
      }

      const rowHeights = {};
      if (ws['!rows']) {
        ws['!rows'].forEach((row, i) => {
          if (row && row.hpx) rowHeights[i] = row.hpx;
        });
      }

      return {
        name,
        id: String(idx),
        status: idx === 0 ? 1 : 0,
        order: idx,
        celldata,
        config: {
          columnlen: colWidths,
          rowlen: rowHeights,
        },
        filter_select: null,
        filter: null,
      };
    });
  }

  async function load(path) {
    sheetWrap.innerHTML = '<div class="preview-loading">Loading spreadsheet...</div>';
    info.textContent = '';

    try {
      const [XLSX, ReactModule, ReactDOMModule, FortuneModule] = await Promise.all([
        import('xlsx'),
        import('react'),
        import('react-dom/client'),
        import('@fortune-sheet/react'),
      ]);

      await import('@fortune-sheet/react/dist/index.css');

      const React = ReactModule.default ?? ReactModule;
      const { createRoot } = ReactDOMModule;
      const { Workbook } = FortuneModule;

      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      const xlsxWorkbook = XLSX.read(binaryStr, { type: 'binary', cellFormula: true, cellNF: true, cellStyles: true });

      const sheets = xlsxSheetToFortune(xlsxWorkbook, XLSX);

      sheetWrap.innerHTML = '';

      if (reactRoot) {
        reactRoot.unmount();
        reactRoot = null;
      }

      reactRoot = createRoot(sheetWrap);
      reactRoot.render(
        React.createElement(Workbook, {
          data: sheets,
          allowEdit: true,
          showToolbar: false,
          showFormulaBar: true,
          showSheetTabs: true,
        })
      );

      const sheetCount = xlsxWorkbook.SheetNames.length;
      info.textContent = `${sheetCount} sheet${sheetCount !== 1 ? 's' : ''}  \u2022  ${formatSize(result.size)}`;
    } catch (e) {
      sheetWrap.innerHTML = `<div class="preview-error">Failed to render spreadsheet: ${e}</div>`;
    }
  }

  function destroy() {
    if (reactRoot) {
      reactRoot.unmount();
      reactRoot = null;
    }
    sheetWrap.innerHTML = '';
    info.textContent = '';
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
