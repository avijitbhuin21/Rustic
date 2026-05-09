import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

const ICON_SAVE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><polyline points="17 21 17 13 7 13 7 21"/><polyline points="7 3 7 8 15 8"/></svg>';
const ICON_FILTER = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/></svg>';
const ICON_PLUS = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>';
const ICON_FILTER_HEADER = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/></svg>';
const ICON_TRASH = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-2 14a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>';

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

export function createXlsxPreview({ onDirtyChange } = {}) {
  const container = el('div', { class: 'preview-container office-preview xlsx-preview' });

  const toolbar = el('div', { class: 'preview-toolbar xlsx-toolbar' });
  const saveBtn = el('button', { class: 'preview-toolbar-btn preview-toolbar-icon-btn', title: 'Save (Ctrl+S)' });
  saveBtn.innerHTML = ICON_SAVE;
  const dirtyDot = el('span', { class: 'xlsx-dirty-dot', title: 'Unsaved changes' });
  dirtyDot.style.display = 'none';
  const sep1 = el('span', { class: 'preview-toolbar-separator' }, '|');
  const filterBtn = el('button', { class: 'preview-toolbar-btn preview-toolbar-icon-btn', title: 'Toggle filters' });
  filterBtn.innerHTML = ICON_FILTER;
  const sep2 = el('span', { class: 'preview-toolbar-separator' }, '|');
  const zoomOutBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom Out' }, '−');
  const zoomLabel = el('span', { class: 'preview-toolbar-label' }, '100%');
  const zoomInBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom In' }, '+');
  const spacer = el('div', { class: 'preview-toolbar-spacer' });
  const sheetLabel = el('span', { class: 'preview-toolbar-label xlsx-sheet-name' }, '');

  toolbar.appendChild(saveBtn);
  toolbar.appendChild(dirtyDot);
  toolbar.appendChild(sep1);
  toolbar.appendChild(filterBtn);
  toolbar.appendChild(sep2);
  toolbar.appendChild(zoomOutBtn);
  toolbar.appendChild(zoomLabel);
  toolbar.appendChild(zoomInBtn);
  toolbar.appendChild(spacer);
  toolbar.appendChild(sheetLabel);

  const gridScroll = el('div', { class: 'xlsx-grid-scroll' });
  const gridSizer = el('div', { class: 'xlsx-grid-sizer' });
  gridScroll.appendChild(gridSizer);

  const tabBar = el('div', { class: 'xlsx-tab-bar' });
  const tabAddBtn = el('button', { class: 'xlsx-tab-add', title: 'New sheet' });
  tabAddBtn.innerHTML = ICON_PLUS;

  const info = el('div', { class: 'preview-info' });

  container.appendChild(toolbar);
  container.appendChild(gridScroll);
  container.appendChild(tabBar);
  container.appendChild(info);

  // Floating filter popover lives at the container level so it doesn't get
  // clipped by the scroll wrapper or pushed off-screen.
  const filterPopover = el('div', { class: 'xlsx-filter-popover' });
  filterPopover.style.display = 'none';
  container.appendChild(filterPopover);

  // ----- State -----
  let XLSX = null;
  let filePath = null;
  let workbook = null;
  // sheets[i] = { name, rows: number, cols: number, data: cell[][], colWidths: number[], rowHeights: number[] }
  // cell = { v: any (raw), w: string (display), f: string|undefined, s: object|undefined }
  let sheets = [];
  let activeIdx = 0;
  let zoom = 1.0;
  let filtersOn = false;
  // perSheet[idx] = { hidden: Set<number>, columnFilter: Map<colIdx, Set<value>> }
  let perSheet = [];
  let isDirty = false;
  let activeFilterCol = null;

  const ROW_HEADER_W = 44;
  const DEFAULT_COL_W = 96;
  const DEFAULT_ROW_H = 24;
  const HEADER_H = 28;

  // ----- Helpers -----
  function setDirty(v) {
    if (isDirty === v) return;
    isDirty = v;
    dirtyDot.style.display = v ? 'inline-block' : 'none';
    saveBtn.classList.toggle('xlsx-save-active', v);
    if (onDirtyChange) {
      try { onDirtyChange(v); } catch (e) { console.error(e); }
    }
  }

  function colName(c) {
    let s = '';
    c++;
    while (c > 0) {
      const r = (c - 1) % 26;
      s = String.fromCharCode(65 + r) + s;
      c = Math.floor((c - 1) / 26);
    }
    return s;
  }

  function emptyCell() {
    return { v: null, w: '', f: undefined, s: undefined };
  }

  function ensureSheetSize(sheet, rowsNeeded, colsNeeded) {
    const targetRows = Math.max(sheet.rows, rowsNeeded);
    const targetCols = Math.max(sheet.cols, colsNeeded);
    while (sheet.data.length < targetRows) sheet.data.push([]);
    for (let r = 0; r < targetRows; r++) {
      while (sheet.data[r].length < targetCols) sheet.data[r].push(emptyCell());
    }
    while (sheet.colWidths.length < targetCols) sheet.colWidths.push(DEFAULT_COL_W);
    while (sheet.rowHeights.length < targetRows) sheet.rowHeights.push(DEFAULT_ROW_H);
    sheet.rows = targetRows;
    sheet.cols = targetCols;
  }

  function workbookToSheets(wb) {
    return wb.SheetNames.map((name) => {
      const ws = wb.Sheets[name];
      const ref = ws['!ref'] || 'A1:A1';
      const range = XLSX.utils.decode_range(ref);
      const rows = range.e.r + 1;
      const cols = range.e.c + 1;
      const data = [];
      for (let r = 0; r < rows; r++) {
        const row = [];
        for (let c = 0; c < cols; c++) {
          const addr = XLSX.utils.encode_cell({ r, c });
          const cell = ws[addr];
          if (!cell) {
            row.push(emptyCell());
          } else {
            row.push({
              v: cell.v,
              w: cell.w != null ? String(cell.w) : (cell.v != null ? String(cell.v) : ''),
              f: cell.f,
              s: cell.s,
            });
          }
        }
        data.push(row);
      }
      const colWidths = [];
      const wsCols = ws['!cols'] || [];
      for (let c = 0; c < cols; c++) {
        const meta = wsCols[c];
        if (meta && meta.wch) colWidths.push(Math.max(48, Math.round(meta.wch * 7.5)));
        else if (meta && meta.wpx) colWidths.push(Math.max(48, meta.wpx));
        else colWidths.push(DEFAULT_COL_W);
      }
      // Source files often store row heights sized for *wrapped* text in
      // Excel — values like 200px to fit a multi-line bullet list. Our grid
      // doesn't wrap (single-line cells), so honoring those heights makes
      // rows look absurdly tall. Default everyone to a tight uniform height
      // and let the user drag to resize per row.
      const rowHeights = new Array(rows).fill(DEFAULT_ROW_H);
      return { name, rows, cols, data, colWidths, rowHeights };
    });
  }

  function sheetsToWorkbook() {
    const wb = XLSX.utils.book_new();
    for (const sheet of sheets) {
      const aoa = [];
      for (let r = 0; r < sheet.rows; r++) {
        const row = [];
        for (let c = 0; c < sheet.cols; c++) {
          const cell = sheet.data[r]?.[c];
          row.push(cell && cell.v !== null && cell.v !== undefined ? cell.v : null);
        }
        aoa.push(row);
      }
      const ws = XLSX.utils.aoa_to_sheet(aoa);
      // Re-apply formulas if any
      for (let r = 0; r < sheet.rows; r++) {
        for (let c = 0; c < sheet.cols; c++) {
          const cell = sheet.data[r]?.[c];
          if (cell && cell.f) {
            const addr = XLSX.utils.encode_cell({ r, c });
            const target = ws[addr] || (ws[addr] = { t: 's', v: '' });
            target.f = cell.f.replace(/^=/, '');
          }
        }
      }
      ws['!cols'] = sheet.colWidths.map((w) => ({ wpx: w }));
      ws['!rows'] = sheet.rowHeights.map((h) => ({ hpx: h }));
      XLSX.utils.book_append_sheet(wb, ws, sheet.name);
    }
    return wb;
  }

  function getActiveSheet() {
    return sheets[activeIdx];
  }

  function getActiveState() {
    return perSheet[activeIdx];
  }

  function visibleRowOrder() {
    const sheet = getActiveSheet();
    const state = getActiveState();
    if (!sheet || !state) return [];
    const out = [];
    // Treat row 0 as the header row when filters are on
    const headerRow = filtersOn ? 0 : -1;
    for (let r = 0; r < sheet.rows; r++) {
      if (r === headerRow) continue;
      if (state.hidden.has(r)) continue;
      let pass = true;
      if (filtersOn) {
        for (const [col, allowed] of state.columnFilter) {
          const cell = sheet.data[r]?.[col];
          const val = cell ? (cell.w || (cell.v != null ? String(cell.v) : '')) : '';
          if (!allowed.has(val)) { pass = false; break; }
        }
      }
      if (pass) out.push(r);
    }
    return out;
  }

  // ----- Rendering -----
  function renderTabs() {
    tabBar.innerHTML = '';
    sheets.forEach((sheet, idx) => {
      const tab = el('div', { class: 'xlsx-tab' + (idx === activeIdx ? ' xlsx-tab-active' : '') });
      const nameSpan = el('span', { class: 'xlsx-tab-name' }, sheet.name);
      tab.appendChild(nameSpan);
      tab.addEventListener('click', () => {
        if (activeIdx !== idx) {
          activeIdx = idx;
          renderTabs();
          renderGrid();
        }
      });
      tab.addEventListener('dblclick', () => {
        const next = prompt('Rename sheet', sheet.name);
        if (next && next.trim() && next !== sheet.name) {
          if (sheets.some((s, i) => i !== idx && s.name === next.trim())) {
            alert('Sheet name already exists.');
            return;
          }
          sheet.name = next.trim();
          setDirty(true);
          renderTabs();
          updateSheetLabel();
        }
      });
      // Right-click to delete (only if more than one sheet)
      tab.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        if (sheets.length <= 1) return;
        if (!confirm(`Delete sheet "${sheet.name}"?`)) return;
        sheets.splice(idx, 1);
        perSheet.splice(idx, 1);
        if (activeIdx >= sheets.length) activeIdx = sheets.length - 1;
        setDirty(true);
        renderTabs();
        renderGrid();
        updateSheetLabel();
      });
      tabBar.appendChild(tab);
    });
    tabBar.appendChild(tabAddBtn);
  }

  function updateSheetLabel() {
    const sheet = getActiveSheet();
    if (!sheet) { sheetLabel.textContent = ''; return; }
    sheetLabel.textContent = `${sheet.name}  •  ${sheet.rows} × ${sheet.cols}`;
  }

  function renderGrid() {
    const sheet = getActiveSheet();
    if (!sheet) {
      gridSizer.innerHTML = '';
      return;
    }

    const visibleRows = visibleRowOrder();
    const state = getActiveState();

    const rowHeaderW = Math.round(ROW_HEADER_W * zoom);
    const headerH = Math.round(HEADER_H * zoom);
    const z = zoom;

    // Compute total width / height (scaled)
    let totalWidth = rowHeaderW;
    for (let c = 0; c < sheet.cols; c++) totalWidth += Math.round(sheet.colWidths[c] * z);

    let totalHeight = headerH;
    for (const r of visibleRows) totalHeight += Math.round(sheet.rowHeights[r] * z);

    gridSizer.innerHTML = '';
    gridSizer.style.width = totalWidth + 'px';
    gridSizer.style.height = totalHeight + 'px';
    gridSizer.style.fontSize = Math.max(9, Math.round(12 * z)) + 'px';

    const inner = el('div', { class: 'xlsx-grid-inner' });
    inner.style.width = totalWidth + 'px';
    inner.style.height = totalHeight + 'px';
    gridSizer.appendChild(inner);

    // ----- Header row (sticky) -----
    const headerRow = el('div', { class: 'xlsx-row xlsx-header-row' });
    headerRow.style.height = headerH + 'px';
    headerRow.style.width = totalWidth + 'px';

    // Top-left corner
    const corner = el('div', { class: 'xlsx-corner' });
    corner.style.width = rowHeaderW + 'px';
    corner.style.height = headerH + 'px';
    headerRow.appendChild(corner);

    let xOff = rowHeaderW;
    for (let c = 0; c < sheet.cols; c++) {
      const w = Math.round(sheet.colWidths[c] * z);
      const colHeader = el('div', { class: 'xlsx-col-header' });
      colHeader.style.width = w + 'px';
      colHeader.style.height = headerH + 'px';
      colHeader.style.left = xOff + 'px';

      const labelText = filtersOn && sheet.data[0]?.[c]
        ? (sheet.data[0][c].w || (sheet.data[0][c].v != null ? String(sheet.data[0][c].v) : colName(c)))
        : colName(c);

      const label = el('span', { class: 'xlsx-col-header-label' }, labelText);
      colHeader.appendChild(label);

      if (filtersOn) {
        const fbtn = el('button', { class: 'xlsx-col-filter-btn', title: 'Filter' });
        fbtn.innerHTML = ICON_FILTER_HEADER;
        if (state.columnFilter.has(c)) fbtn.classList.add('xlsx-col-filter-btn-active');
        fbtn.addEventListener('click', (e) => {
          e.stopPropagation();
          openColumnFilter(c, fbtn);
        });
        colHeader.appendChild(fbtn);
      }

      // Column resize handle
      const handle = el('div', { class: 'xlsx-col-resize' });
      colHeader.appendChild(handle);
      attachColResize(handle, c);

      headerRow.appendChild(colHeader);
      xOff += w;
    }

    inner.appendChild(headerRow);

    // ----- Data rows -----
    let yOff = headerH;
    for (const r of visibleRows) {
      const rowH = Math.round(sheet.rowHeights[r] * z);
      const rowEl = el('div', { class: 'xlsx-row' });
      rowEl.style.top = yOff + 'px';
      rowEl.style.height = rowH + 'px';
      rowEl.style.width = totalWidth + 'px';

      // Row header (fixed left)
      const rowHeader = el('div', { class: 'xlsx-row-header' });
      rowHeader.style.width = rowHeaderW + 'px';
      rowHeader.style.height = rowH + 'px';
      rowHeader.textContent = String(r + 1);

      const rowResize = el('div', { class: 'xlsx-row-resize' });
      rowHeader.appendChild(rowResize);
      attachRowResize(rowResize, r);

      rowEl.appendChild(rowHeader);

      let cx = rowHeaderW;
      for (let c = 0; c < sheet.cols; c++) {
        const w = Math.round(sheet.colWidths[c] * z);
        const cell = sheet.data[r]?.[c] || emptyCell();
        const cellEl = el('div', { class: 'xlsx-cell', tabindex: '0' });
        cellEl.style.left = cx + 'px';
        cellEl.style.width = w + 'px';
        cellEl.style.height = rowH + 'px';
        cellEl.dataset.r = String(r);
        cellEl.dataset.c = String(c);
        cellEl.textContent = cell.w || (cell.v != null ? String(cell.v) : '');
        if (typeof cell.v === 'number') cellEl.classList.add('xlsx-cell-num');
        attachCellEdit(cellEl, r, c);
        rowEl.appendChild(cellEl);
        cx += w;
      }

      inner.appendChild(rowEl);
      yOff += rowH;
    }
  }

  // ----- Cell editing -----
  function attachCellEdit(cellEl, r, c) {
    let originalText = '';

    function startEdit() {
      if (cellEl.classList.contains('xlsx-cell-editing')) return;
      // Pin the original size as a minimum so the editor can grow but never
      // shrinks below the cell, then drop the fixed dimensions so the box
      // itself (with its opaque background) expands to cover its content
      // instead of letting overflow paint transparently over neighbors.
      cellEl.style.minWidth = cellEl.style.width;
      cellEl.style.minHeight = cellEl.style.height;
      cellEl.style.width = 'auto';
      cellEl.style.height = 'auto';
      cellEl.classList.add('xlsx-cell-editing');
      cellEl.contentEditable = 'true';
      originalText = cellEl.textContent;
      cellEl.focus();
      // Place caret at end
      const range = document.createRange();
      range.selectNodeContents(cellEl);
      range.collapse(false);
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
    }

    function commitEdit(save) {
      if (!cellEl.classList.contains('xlsx-cell-editing')) return;
      cellEl.classList.remove('xlsx-cell-editing');
      cellEl.contentEditable = 'false';
      // Restore the cell to its grid-allotted width/height.
      cellEl.style.width = cellEl.style.minWidth;
      cellEl.style.height = cellEl.style.minHeight;
      cellEl.style.minWidth = '';
      cellEl.style.minHeight = '';
      const text = cellEl.textContent;
      if (!save) {
        cellEl.textContent = originalText;
        return;
      }
      if (text !== originalText) {
        const sheet = getActiveSheet();
        ensureSheetSize(sheet, r + 1, c + 1);
        const cell = sheet.data[r][c];
        cell.f = undefined;
        if (text === '') {
          cell.v = null;
          cell.w = '';
        } else if (text.startsWith('=')) {
          cell.f = text;
          cell.v = text;
          cell.w = text;
        } else {
          const num = Number(text);
          if (!isNaN(num) && text.trim() !== '' && /^-?\d*\.?\d+(e[-+]?\d+)?$/i.test(text.trim())) {
            cell.v = num;
            cell.w = String(num);
          } else {
            cell.v = text;
            cell.w = text;
          }
        }
        cellEl.classList.toggle('xlsx-cell-num', typeof cell.v === 'number');
        setDirty(true);
      }
    }

    cellEl.addEventListener('dblclick', startEdit);
    cellEl.addEventListener('keydown', (e) => {
      if (cellEl.classList.contains('xlsx-cell-editing')) {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          commitEdit(true);
          cellEl.blur();
        } else if (e.key === 'Escape') {
          e.preventDefault();
          commitEdit(false);
          cellEl.blur();
        }
        return;
      }
      // Not editing
      if (e.key === 'Enter' || e.key === 'F2') {
        e.preventDefault();
        startEdit();
      } else if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        const sheet = getActiveSheet();
        ensureSheetSize(sheet, r + 1, c + 1);
        const cell = sheet.data[r][c];
        cell.v = null;
        cell.w = '';
        cell.f = undefined;
        cellEl.textContent = '';
        cellEl.classList.remove('xlsx-cell-num');
        setDirty(true);
      } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        // Begin typing immediately
        cellEl.textContent = '';
        startEdit();
      }
    });
    cellEl.addEventListener('blur', () => commitEdit(true));
  }

  // ----- Column resize -----
  function attachColResize(handle, colIdx) {
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      const sheet = getActiveSheet();
      const startX = e.clientX;
      const startW = sheet.colWidths[colIdx];
      function move(ev) {
        const dx = (ev.clientX - startX) / zoom;
        sheet.colWidths[colIdx] = Math.max(32, Math.round(startW + dx));
        renderGrid();
      }
      function up() {
        document.removeEventListener('mousemove', move);
        document.removeEventListener('mouseup', up);
        setDirty(true);
      }
      document.addEventListener('mousemove', move);
      document.addEventListener('mouseup', up);
    });
  }

  // ----- Row resize -----
  function attachRowResize(handle, rowIdx) {
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      const sheet = getActiveSheet();
      const startY = e.clientY;
      const startH = sheet.rowHeights[rowIdx];
      function move(ev) {
        const dy = (ev.clientY - startY) / zoom;
        sheet.rowHeights[rowIdx] = Math.max(16, Math.round(startH + dy));
        renderGrid();
      }
      function up() {
        document.removeEventListener('mousemove', move);
        document.removeEventListener('mouseup', up);
        setDirty(true);
      }
      document.addEventListener('mousemove', move);
      document.addEventListener('mouseup', up);
    });
  }

  // ----- Filter popover -----
  function closeFilterPopover() {
    filterPopover.style.display = 'none';
    filterPopover.innerHTML = '';
    activeFilterCol = null;
  }

  function openColumnFilter(colIdx, anchorBtn) {
    if (activeFilterCol === colIdx && filterPopover.style.display !== 'none') {
      closeFilterPopover();
      return;
    }
    activeFilterCol = colIdx;
    const sheet = getActiveSheet();
    const state = getActiveState();

    // Gather unique values from non-header rows
    const values = new Map(); // value -> count
    for (let r = 1; r < sheet.rows; r++) {
      const cell = sheet.data[r]?.[colIdx];
      const val = cell ? (cell.w || (cell.v != null ? String(cell.v) : '')) : '';
      values.set(val, (values.get(val) || 0) + 1);
    }
    const sortedVals = [...values.keys()].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));

    const currentAllowed = state.columnFilter.get(colIdx) || new Set(sortedVals);

    filterPopover.innerHTML = '';
    const search = el('input', { class: 'xlsx-filter-search', placeholder: 'Search...', type: 'text' });
    filterPopover.appendChild(search);

    const actions = el('div', { class: 'xlsx-filter-actions' });
    const selectAll = el('button', { class: 'xlsx-filter-link' }, 'Select all');
    const clearAll = el('button', { class: 'xlsx-filter-link' }, 'Clear');
    actions.appendChild(selectAll);
    actions.appendChild(clearAll);
    filterPopover.appendChild(actions);

    const list = el('div', { class: 'xlsx-filter-list' });
    filterPopover.appendChild(list);

    const checkboxes = [];

    function renderList(filterText) {
      list.innerHTML = '';
      checkboxes.length = 0;
      const term = filterText.trim().toLowerCase();
      for (const val of sortedVals) {
        if (term && !val.toLowerCase().includes(term)) continue;
        const labelEl = el('label', { class: 'xlsx-filter-item' });
        const cb = el('input', { type: 'checkbox' });
        cb.checked = currentAllowed.has(val);
        cb.dataset.val = val;
        labelEl.appendChild(cb);
        labelEl.appendChild(el('span', { class: 'xlsx-filter-item-text' }, val === '' ? '(blanks)' : val));
        labelEl.appendChild(el('span', { class: 'xlsx-filter-item-count' }, String(values.get(val))));
        list.appendChild(labelEl);
        checkboxes.push(cb);
      }
    }
    renderList('');

    search.addEventListener('input', () => renderList(search.value));
    selectAll.addEventListener('click', () => {
      checkboxes.forEach((cb) => { cb.checked = true; });
    });
    clearAll.addEventListener('click', () => {
      checkboxes.forEach((cb) => { cb.checked = false; });
    });

    const footer = el('div', { class: 'xlsx-filter-footer' });
    const applyBtn = el('button', { class: 'xlsx-filter-btn xlsx-filter-btn-primary' }, 'Apply');
    const resetBtn = el('button', { class: 'xlsx-filter-btn' }, 'Reset');
    const cancelBtn = el('button', { class: 'xlsx-filter-btn' }, 'Cancel');
    footer.appendChild(resetBtn);
    footer.appendChild(cancelBtn);
    footer.appendChild(applyBtn);
    filterPopover.appendChild(footer);

    applyBtn.addEventListener('click', () => {
      const allowed = new Set();
      list.querySelectorAll('input[type=checkbox]').forEach((cb) => {
        if (cb.checked) allowed.add(cb.dataset.val);
      });
      if (allowed.size === sortedVals.length) {
        state.columnFilter.delete(colIdx);
      } else {
        state.columnFilter.set(colIdx, allowed);
      }
      closeFilterPopover();
      renderGrid();
    });
    resetBtn.addEventListener('click', () => {
      state.columnFilter.delete(colIdx);
      closeFilterPopover();
      renderGrid();
    });
    cancelBtn.addEventListener('click', closeFilterPopover);

    // Position the popover next to the anchor button, clamped to the viewport.
    filterPopover.style.display = 'flex';
    filterPopover.style.visibility = 'hidden';
    const anchorRect = anchorBtn.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    const popRect = filterPopover.getBoundingClientRect();

    let left = anchorRect.left - containerRect.left;
    let top = anchorRect.bottom - containerRect.top + 4;

    if (left + popRect.width > containerRect.width - 8) {
      left = containerRect.width - popRect.width - 8;
    }
    if (left < 8) left = 8;
    if (top + popRect.height > containerRect.height - 8) {
      top = anchorRect.top - containerRect.top - popRect.height - 4;
      if (top < 8) top = 8;
    }
    filterPopover.style.left = left + 'px';
    filterPopover.style.top = top + 'px';
    filterPopover.style.visibility = 'visible';
    setTimeout(() => search.focus(), 0);
  }

  // ----- Toolbar handlers -----
  saveBtn.addEventListener('click', save);
  filterBtn.addEventListener('click', () => {
    filtersOn = !filtersOn;
    filterBtn.classList.toggle('active', filtersOn);
    closeFilterPopover();
    renderGrid();
  });
  zoomInBtn.addEventListener('click', () => {
    zoom = Math.min(2, +(zoom + 0.1).toFixed(2));
    zoomLabel.textContent = `${Math.round(zoom * 100)}%`;
    renderGrid();
  });
  zoomOutBtn.addEventListener('click', () => {
    zoom = Math.max(0.5, +(zoom - 0.1).toFixed(2));
    zoomLabel.textContent = `${Math.round(zoom * 100)}%`;
    renderGrid();
  });

  tabAddBtn.addEventListener('click', () => {
    let n = sheets.length + 1;
    let name = `Sheet${n}`;
    while (sheets.some((s) => s.name === name)) { n++; name = `Sheet${n}`; }
    const newSheet = { name, rows: 30, cols: 12, data: [], colWidths: [], rowHeights: [] };
    ensureSheetSize(newSheet, 30, 12);
    sheets.push(newSheet);
    perSheet.push({ hidden: new Set(), columnFilter: new Map() });
    activeIdx = sheets.length - 1;
    setDirty(true);
    renderTabs();
    renderGrid();
    updateSheetLabel();
  });

  // Outside click / escape closes filter popover
  document.addEventListener('mousedown', (e) => {
    if (filterPopover.style.display === 'none') return;
    if (filterPopover.contains(e.target)) return;
    if (e.target.closest && e.target.closest('.xlsx-col-filter-btn')) return;
    closeFilterPopover();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeFilterPopover();
  });

  // Ctrl+S (only while the preview is in the DOM and focused area is inside it)
  function onKeyDownGlobal(e) {
    if (!container.isConnected) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      // Only intercept if our preview is the visible one
      if (!container.offsetParent) return;
      e.preventDefault();
      save();
    }
  }
  document.addEventListener('keydown', onKeyDownGlobal);

  async function save() {
    if (!filePath || !workbook) return;
    try {
      saveBtn.classList.add('xlsx-save-saving');
      const wb = sheetsToWorkbook();
      const arr = XLSX.write(wb, { type: 'array', bookType: 'xlsx' });
      const bytes = new Uint8Array(arr);
      // Convert to base64
      let bin = '';
      const chunk = 0x8000;
      for (let i = 0; i < bytes.length; i += chunk) {
        bin += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
      }
      const b64 = btoa(bin);
      await api.writeFileBase64(filePath, b64);
      setDirty(false);
      info.textContent = `Saved • ${formatSize(bytes.length)} • ${sheets.length} sheet${sheets.length !== 1 ? 's' : ''}`;
    } catch (e) {
      console.error('XLSX save failed', e);
      info.textContent = `Save failed: ${e}`;
    } finally {
      saveBtn.classList.remove('xlsx-save-saving');
    }
  }

  async function load(path) {
    filePath = path;
    setDirty(false);
    gridSizer.innerHTML = '<div class="preview-loading">Loading spreadsheet...</div>';
    info.textContent = '';
    sheetLabel.textContent = '';
    tabBar.innerHTML = '';
    closeFilterPopover();

    try {
      const XLSXMod = await import('xlsx');
      XLSX = XLSXMod.default ?? XLSXMod;

      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      workbook = XLSX.read(binaryStr, { type: 'binary', cellFormula: true, cellNF: true, cellStyles: true });
      sheets = workbookToSheets(workbook);
      perSheet = sheets.map(() => ({ hidden: new Set(), columnFilter: new Map() }));
      activeIdx = 0;

      gridSizer.innerHTML = '';
      renderTabs();
      renderGrid();
      updateSheetLabel();

      info.textContent = `${sheets.length} sheet${sheets.length !== 1 ? 's' : ''}  •  ${formatSize(result.size)}`;
    } catch (e) {
      gridSizer.innerHTML = `<div class="preview-error">Failed to render spreadsheet: ${e}</div>`;
    }
  }

  function destroy() {
    document.removeEventListener('keydown', onKeyDownGlobal);
    closeFilterPopover();
    gridSizer.innerHTML = '';
    tabBar.innerHTML = '';
    info.textContent = '';
    // NOTE: keep sheets/workbook/filePath alive so a registered save handler
    // can still flush pending edits when the user closes a non-active tab
    // after switching away. They're freed when the closure itself becomes
    // unreachable (i.e. when the buffer is truly closed).
  }

  return { element: container, load, destroy, save, isDirty: () => isDirty };
}

export function createUnsupportedPreview() {
  const container = el('div', { class: 'preview-container unsupported-preview' });

  function load(path, fileType) {
    container.innerHTML = '';
    const msg = el('div', { class: 'unsupported-message' });
    msg.appendChild(el('div', { class: 'unsupported-icon' }, '⚠'));
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
