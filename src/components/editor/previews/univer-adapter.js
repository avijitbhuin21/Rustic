// Convert between an ExcelJS workbook and Univer's IWorkbookData shape.
//
// Univer's open-source build doesn't ship XLSX import/export — that's
// gated behind their paid `@univerjs-pro/exchange-client` package. So we
// parse the file ourselves via ExcelJS (already in deps), convert to the
// snapshot shape Univer wants, hand it to `univerAPI.createWorkbook`,
// and on save reverse the process to write a new XLSX through ExcelJS.
//
// Univer's wire vocab (see @univerjs/core/lib/types/sheets/typedef and
// types/interfaces/i-style-data):
//   ICellData: { v, t, f, s } — value, type (1=string, 2=number, 3=bool),
//                                formula, style id OR inline IStyleData
//   IStyleData: { ff, fs, bl, it, ul, st, bg, bd, cl, ht, vt, tb, n }
//   HorizontalAlign: 0=unspecified, 1=left, 2=center, 3=right
//   VerticalAlign:   0=unspecified, 1=top,  2=middle, 3=bottom
//   WrapStrategy:    0=unspecified, 1=overflow, 2=clip, 3=wrap
//   BooleanNumber:   0=false, 1=true

import ExcelJS from 'exceljs';

// Numeric mirrors of the Univer enums so we don't have to import the
// enum objects from @univerjs/core at this layer (keeps the adapter
// stand-alone and tree-shakeable).
const H_ALIGN = { left: 1, center: 2, centerContinuous: 2, right: 3, justify: 4, distributed: 6 };
const V_ALIGN = { top: 1, middle: 2, center: 2, bottom: 3 };
const WRAP_STRATEGY = { overflow: 1, clip: 2, wrap: 3 };
const CELL_TYPE = { STRING: 1, NUMBER: 2, BOOLEAN: 3, FORCE_STRING: 4 };

// Excel column widths are measured in "characters of the default font at
// 100%" — about 7 px of glyph + 5 px of padding. Univer wants pixels.
const colWidthCharsToPx = (chars) => Math.round(chars * 7 + 5);
const colWidthPxToChars = (px) => Math.max(1, (px - 5) / 7);
// Excel row heights are in points; 1 pt = 96/72 px.
const rowHeightPtToPx = (pt) => Math.round((pt * 96) / 72);
const rowHeightPxToPt = (px) => (px * 72) / 96;

function argbToHexColor(argb) {
  if (!argb || typeof argb !== 'string') return null;
  const hex = argb.length === 8 ? argb.slice(2) : argb;
  if (!/^[0-9A-Fa-f]{6}$/.test(hex)) return null;
  return `#${hex.toUpperCase()}`;
}
function hexToArgbColor(hex) {
  if (!hex || typeof hex !== 'string') return null;
  const m = hex.replace('#', '');
  if (!/^[0-9A-Fa-f]{6}$/.test(m)) return null;
  return `FF${m.toUpperCase()}`;
}

// Convert an exceljs cell.font / .fill / .alignment / .numFmt block into
// Univer's IStyleData shape. Returns null when the cell has no style
// information worth carrying (so the caller can leave the cell's `s` slot
// empty).
function styleFromExcelCell(cell) {
  const style = {};
  let any = false;

  const font = cell.font;
  if (font) {
    if (font.name) { style.ff = font.name; any = true; }
    if (typeof font.size === 'number') { style.fs = font.size; any = true; }
    if (font.bold) { style.bl = 1; any = true; }
    if (font.italic) { style.it = 1; any = true; }
    if (font.underline) {
      // Univer wants an ITextDecoration object, not a bool.
      style.ul = { s: 1 };
      any = true;
    }
    if (font.strike) { style.st = { s: 1 }; any = true; }
    const fc = argbToHexColor(font.color?.argb);
    if (fc) { style.cl = { rgb: fc }; any = true; }
  }

  const fill = cell.fill;
  if (fill?.type === 'pattern' && fill.pattern === 'solid') {
    const bg =
      argbToHexColor(fill.fgColor?.argb) || argbToHexColor(fill.bgColor?.argb);
    if (bg) { style.bg = { rgb: bg }; any = true; }
  }

  const align = cell.alignment;
  if (align) {
    const ht = H_ALIGN[align.horizontal];
    const vt = V_ALIGN[align.vertical];
    if (ht) { style.ht = ht; any = true; }
    if (vt) { style.vt = vt; any = true; }
    if (align.wrapText) { style.tb = WRAP_STRATEGY.wrap; any = true; }
  }

  if (cell.numFmt && cell.numFmt !== 'General') {
    style.n = { pattern: String(cell.numFmt) };
    any = true;
  }

  return any ? style : null;
}

// Reverse: a Univer IStyleData → fields to assign onto an ExcelJS cell.
function applyStyleToExcelCell(style, target) {
  if (!style) return;

  // Font / colour
  const font = {};
  if (style.ff) font.name = style.ff;
  if (typeof style.fs === 'number') font.size = style.fs;
  if (style.bl === 1) font.bold = true;
  if (style.it === 1) font.italic = true;
  if (style.ul?.s === 1) font.underline = true;
  if (style.st?.s === 1) font.strike = true;
  if (style.cl?.rgb) {
    const argb = hexToArgbColor(style.cl.rgb);
    if (argb) font.color = { argb };
  }
  if (Object.keys(font).length) target.font = font;

  // Fill
  if (style.bg?.rgb) {
    const argb = hexToArgbColor(style.bg.rgb);
    if (argb) {
      target.fill = { type: 'pattern', pattern: 'solid', fgColor: { argb } };
    }
  }

  // Alignment
  const align = {};
  if (style.ht === H_ALIGN.left) align.horizontal = 'left';
  else if (style.ht === H_ALIGN.center) align.horizontal = 'center';
  else if (style.ht === H_ALIGN.right) align.horizontal = 'right';
  if (style.vt === V_ALIGN.top) align.vertical = 'top';
  else if (style.vt === V_ALIGN.middle) align.vertical = 'middle';
  else if (style.vt === V_ALIGN.bottom) align.vertical = 'bottom';
  if (style.tb === WRAP_STRATEGY.wrap) align.wrapText = true;
  if (Object.keys(align).length) target.alignment = align;

  // Number format
  if (style.n?.pattern) target.numFmt = style.n.pattern;
}

/**
 * Parse an .xlsx file buffer with ExcelJS and produce an IWorkbookData
 * snapshot ready for `univerAPI.createWorkbook`.
 *
 * @param {ArrayBuffer} buffer
 * @param {string} workbookName - displayed in Univer's tab header
 * @returns {Promise<IWorkbookData>}
 */
export async function excelJsBufferToUniver(buffer, workbookName = 'Workbook') {
  const wb = new ExcelJS.Workbook();
  await wb.xlsx.load(buffer);

  // Hoist every unique style into the workbook-level `styles` map and
  // reference them from cells by id. This is how Univer recommends
  // representing repeated styles — cuts memory for sheets where the
  // same formatting repeats down a column.
  const stylesById = {};
  const styleKeyToId = new Map();
  let nextStyleId = 1;
  function internStyle(style) {
    if (!style) return null;
    // Cheap stable key — fields are small primitives + nested objects
    // of small primitives, so JSON.stringify with a sorted key list is
    // good enough. Order doesn't change across calls in our generator.
    const key = JSON.stringify(style, Object.keys(style).sort());
    let id = styleKeyToId.get(key);
    if (!id) {
      id = `s${nextStyleId++}`;
      stylesById[id] = style;
      styleKeyToId.set(key, id);
    }
    return id;
  }

  const sheets = {};
  const sheetOrder = [];

  wb.eachSheet((ws, sheetId) => {
    const id = `sheet-${sheetId}-${ws.name}`;
    sheetOrder.push(id);

    let maxRow = 0;
    let maxCol = 0;
    const cellData = {}; // { [row]: { [col]: ICellData } }

    ws.eachRow({ includeEmpty: false }, (row, rowNumber) => {
      row.eachCell({ includeEmpty: false }, (cell, colNumber) => {
        const r = rowNumber - 1;
        const c = colNumber - 1;
        if (r > maxRow) maxRow = r;
        if (c > maxCol) maxCol = c;

        let rawValue = cell.value;
        let formula;
        if (rawValue && typeof rawValue === 'object') {
          if ('formula' in rawValue) {
            formula = rawValue.formula || rawValue.sharedFormula;
            rawValue = rawValue.result ?? '';
          } else if ('richText' in rawValue && Array.isArray(rawValue.richText)) {
            // Flatten rich text — Univer can render styled runs via its
            // `p` (paragraph) field but that requires modeling the runs
            // as a document tree, which is heavy. Cell-level style is
            // good enough for the common case.
            rawValue = rawValue.richText.map((r) => r.text).join('');
          } else if ('hyperlink' in rawValue) {
            rawValue = rawValue.text ?? rawValue.hyperlink;
          } else if ('error' in rawValue) {
            rawValue = rawValue.error;
          } else if (rawValue instanceof Date) {
            // exceljs flags Date values via the wrapper object; keep
            // the Date intact for the type-mapping branch below.
          }
        }

        const out = {};
        if (formula) out.f = formula.startsWith('=') ? formula : `=${formula}`;

        if (typeof rawValue === 'number') {
          out.v = rawValue;
          out.t = CELL_TYPE.NUMBER;
        } else if (typeof rawValue === 'boolean') {
          out.v = rawValue ? 1 : 0;
          out.t = CELL_TYPE.BOOLEAN;
        } else if (rawValue instanceof Date) {
          // Univer stores dates as their Excel serial number with a
          // number type + numFmt — that's what Excel does too. 25569
          // is the offset between Excel epoch (1899-12-30) and Unix.
          out.v = 25569 + rawValue.getTime() / 86400000;
          out.t = CELL_TYPE.NUMBER;
        } else if (rawValue != null && rawValue !== '') {
          out.v = String(rawValue);
          out.t = CELL_TYPE.STRING;
        }

        const styleId = internStyle(styleFromExcelCell(cell));
        if (styleId) out.s = styleId;

        // Skip pure-empty cells unless they carry a style — Univer's
        // matrix is sparse, so blanks-without-style stay implicit.
        if (out.v == null && !out.f && !out.s) return;

        if (!cellData[r]) cellData[r] = {};
        cellData[r][c] = out;
      });
    });

    // Column widths — exceljs reports `.width` (in Excel chars) only
    // for columns that have an explicit override.
    const columnData = {};
    if (Array.isArray(ws.columns)) {
      ws.columns.forEach((col, idx) => {
        if (col && typeof col.width === 'number') {
          columnData[idx] = { w: colWidthCharsToPx(col.width) };
        }
      });
    }

    // Row heights — `.height` in points, populated only on explicit
    // overrides. Track them so heading-rich sheets don't collapse.
    const rowData = {};
    const rowCount = ws.rowCount || 0;
    for (let r = 1; r <= rowCount; r++) {
      const row = ws.getRow(r);
      if (typeof row.height === 'number') {
        rowData[r - 1] = { h: rowHeightPtToPx(row.height) };
      }
    }

    // Merged ranges — convert exceljs's 'B2:C3' strings to Univer's
    // IRange shape ({ startRow, startColumn, endRow, endColumn }).
    const mergeData = [];
    const merges = ws.model?.merges || [];
    for (const m of merges) {
      const dash = m.indexOf(':');
      if (dash < 0) continue;
      const startCell = ws.getCell(m.slice(0, dash));
      const endCell = ws.getCell(m.slice(dash + 1));
      mergeData.push({
        startRow: startCell.row - 1,
        startColumn: startCell.col - 1,
        endRow: endCell.row - 1,
        endColumn: endCell.col - 1,
      });
    }

    sheets[id] = {
      id,
      name: ws.name,
      tabColor: '',
      hidden: 0,
      rowCount: Math.max(100, maxRow + 20),
      columnCount: Math.max(26, maxCol + 5),
      defaultColumnWidth: 80,
      defaultRowHeight: 24,
      mergeData,
      cellData,
      rowData,
      columnData,
      zoomRatio: 1,
      scrollTop: 0,
      scrollLeft: 0,
      freeze: { startRow: -1, startColumn: -1, ySplit: 0, xSplit: 0 },
      rowHeader: { width: 46, hidden: 0 },
      columnHeader: { height: 20, hidden: 0 },
      showGridlines: 1,
      rightToLeft: 0,
    };
  });

  if (sheetOrder.length === 0) {
    const id = 'sheet-empty';
    sheets[id] = {
      id,
      name: 'Sheet1',
      tabColor: '',
      hidden: 0,
      rowCount: 100,
      columnCount: 26,
      defaultColumnWidth: 80,
      defaultRowHeight: 24,
      mergeData: [],
      cellData: {},
      rowData: {},
      columnData: {},
      zoomRatio: 1,
      scrollTop: 0,
      scrollLeft: 0,
      freeze: { startRow: -1, startColumn: -1, ySplit: 0, xSplit: 0 },
      rowHeader: { width: 46, hidden: 0 },
      columnHeader: { height: 20, hidden: 0 },
      showGridlines: 1,
      rightToLeft: 0,
    };
    sheetOrder.push(id);
  }

  return {
    id: `workbook-${Date.now()}`,
    name: workbookName,
    appVersion: '0.24.0',
    locale: 'enUS',
    styles: stylesById,
    sheetOrder,
    sheets,
  };
}

/**
 * Inverse — take a Univer IWorkbookData snapshot (from `fWorkbook.save()`)
 * and produce an ExcelJS Workbook ready for `.xlsx.writeBuffer()`.
 *
 * @param {IWorkbookData} snapshot
 * @returns {Promise<ExcelJS.Workbook>}
 */
export async function univerSnapshotToExcelJs(snapshot) {
  const wb = new ExcelJS.Workbook();
  const styleMap = snapshot.styles || {};

  // Iterate sheets in the order the user sees them, not Object.keys order.
  const order = Array.isArray(snapshot.sheetOrder) && snapshot.sheetOrder.length
    ? snapshot.sheetOrder
    : Object.keys(snapshot.sheets || {});

  for (const sheetId of order) {
    const sheet = snapshot.sheets?.[sheetId];
    if (!sheet) continue;
    const ws = wb.addWorksheet((sheet.name || 'Sheet').slice(0, 31));

    // Column widths first — exceljs will copy these into newly-created
    // columns on .getCell() so cell-level styling later doesn't reset
    // them.
    if (sheet.columnData) {
      for (const [k, col] of Object.entries(sheet.columnData)) {
        const idx = Number(k);
        if (!Number.isFinite(idx) || !col || typeof col.w !== 'number') continue;
        ws.getColumn(idx + 1).width = colWidthPxToChars(col.w);
      }
    }

    // Walk the sparse cellData matrix.
    const cellData = sheet.cellData || {};
    for (const [rKey, row] of Object.entries(cellData)) {
      const r = Number(rKey);
      if (!Number.isFinite(r) || !row) continue;
      for (const [cKey, cell] of Object.entries(row)) {
        const c = Number(cKey);
        if (!Number.isFinite(c) || !cell) continue;
        const target = ws.getCell(r + 1, c + 1);

        // Value + formula.
        if (cell.f && typeof cell.f === 'string') {
          const expr = cell.f.startsWith('=') ? cell.f.slice(1) : cell.f;
          target.value = { formula: expr, result: cell.v ?? null };
        } else if (cell.v != null) {
          // Decode the Univer cell type back to JS native.
          if (cell.t === CELL_TYPE.NUMBER) target.value = Number(cell.v);
          else if (cell.t === CELL_TYPE.BOOLEAN) target.value = Boolean(cell.v);
          else target.value = String(cell.v);
        }

        // Style lookup: `s` can be either a string (id into the styles
        // map) or an inline IStyleData object.
        let style = null;
        if (typeof cell.s === 'string') style = styleMap[cell.s];
        else if (cell.s && typeof cell.s === 'object') style = cell.s;
        applyStyleToExcelCell(style, target);
      }
    }

    // Row heights — applied after cells so each row exists.
    if (sheet.rowData) {
      for (const [k, row] of Object.entries(sheet.rowData)) {
        const idx = Number(k);
        if (!Number.isFinite(idx) || !row || typeof row.h !== 'number') continue;
        ws.getRow(idx + 1).height = rowHeightPxToPt(row.h);
      }
    }

    // Merged ranges last — exceljs throws if the range overlaps an
    // existing merge, which we silently swallow rather than letting
    // a single duplicate kill the save.
    if (Array.isArray(sheet.mergeData)) {
      for (const m of sheet.mergeData) {
        try {
          ws.mergeCells(m.startRow + 1, m.startColumn + 1, m.endRow + 1, m.endColumn + 1);
        } catch {
          // overlap — skip
        }
      }
    }
  }

  return wb;
}
