import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';
import { basename } from '@/state/editor';
import {
  excelJsBufferToUniver,
  univerSnapshotToExcelJs,
} from './univer-adapter';
import './xlsx-preview.css';

// We import the Univer presets dynamically inside the effect so the
// big chunk (~1.5 MB) only loads when a spreadsheet is actually opened.
// Doing it at module scope would pull Univer into any page that imports
// EditorPane via the lazy-loaded `xlsx` chunk regardless of whether a
// user ever opens an .xlsx file.

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
function bytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

function extOf(path) {
  const i = path.lastIndexOf('.');
  return i < 0 ? '' : path.slice(i + 1).toLowerCase();
}

// CSV is special-cased because (a) it's plain text — no ExcelJS round-
// trip needed — and (b) Univer wants a workbook snapshot. We synthesise
// a single-sheet workbook in the Univer shape from the parsed rows.
function csvTextToUniverSnapshot(text, sheetName = 'Sheet1') {
  const rows = [];
  let row = [];
  let field = '';
  let inQuotes = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; }
        else inQuotes = false;
      } else field += ch;
    } else {
      if (ch === '"') inQuotes = true;
      else if (ch === ',') { row.push(field); field = ''; }
      else if (ch === '\r') { /* skip */ }
      else if (ch === '\n') { row.push(field); rows.push(row); row = []; field = ''; }
      else field += ch;
    }
  }
  if (field.length > 0 || row.length > 0) { row.push(field); rows.push(row); }

  const cellData = {};
  let maxCol = 0;
  for (let r = 0; r < rows.length; r++) {
    const cols = rows[r];
    for (let c = 0; c < cols.length; c++) {
      const raw = cols[c];
      if (raw === '') continue;
      const trimmed = raw.trim();
      const isNum = trimmed !== '' && Number.isFinite(Number(trimmed)) && /^-?\d/.test(trimmed);
      if (!cellData[r]) cellData[r] = {};
      cellData[r][c] = isNum
        ? { v: Number(trimmed), t: 2 }
        : { v: raw, t: 1 };
      if (c > maxCol) maxCol = c;
    }
  }

  const sheetId = 'sheet-csv-0';
  return {
    id: `workbook-csv-${Date.now()}`,
    name: sheetName,
    appVersion: '0.24.0',
    locale: 'enUS',
    styles: {},
    sheetOrder: [sheetId],
    sheets: {
      [sheetId]: {
        id: sheetId,
        name: sheetName,
        tabColor: '',
        hidden: 0,
        rowCount: Math.max(100, rows.length + 20),
        columnCount: Math.max(26, maxCol + 5),
        defaultColumnWidth: 80,
        defaultRowHeight: 24,
        mergeData: [],
        cellData,
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
      },
    },
  };
}

function univerSnapshotToCsvText(snapshot) {
  const firstId = snapshot.sheetOrder?.[0];
  const sheet = firstId ? snapshot.sheets?.[firstId] : null;
  if (!sheet || !sheet.cellData) return '';
  const rows = sheet.cellData;
  const indices = Object.keys(rows).map(Number).filter(Number.isFinite).sort((a, b) => a - b);
  if (indices.length === 0) return '';
  const maxRow = indices[indices.length - 1];
  const lines = [];
  for (let r = 0; r <= maxRow; r++) {
    const cols = rows[r] || {};
    const colIdxs = Object.keys(cols).map(Number).filter(Number.isFinite);
    const maxCol = colIdxs.length ? Math.max(...colIdxs) : -1;
    const line = [];
    for (let c = 0; c <= maxCol; c++) {
      const cell = cols[c];
      if (!cell || cell.v == null) { line.push(''); continue; }
      const text = String(cell.v);
      if (/[",\r\n]/.test(text)) line.push(`"${text.replace(/"/g, '""')}"`);
      else line.push(text);
    }
    lines.push(line.join(','));
  }
  return lines.join('\n');
}

export default function XlsxPreview({ tab }) {
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(true);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const containerRef = useRef(null);
  // Univer state. `univer` is the framework instance (for dispose), and
  // `univerAPI` is the facade we call to read / write the active workbook.
  const univerRef = useRef(null);
  const univerAPIRef = useRef(null);
  // Used to skip the initial onValueChange burst that Univer fires when a
  // workbook first mounts (Univer treats `createWorkbook` as a set of
  // edits internally), so opening a file doesn't immediately mark it
  // dirty.
  const readyForDirtyRef = useRef(false);
  const tabSetDirty = useEditor((s) => s.setDirty);

  const ext = extOf(tab.path);
  const isCsv = ext === 'csv';

  const reloadVersion = useFileReloadVersion(tab.path, { enabled: !dirty });

  // ── Mount Univer + load the file. We tear down on path change. ──
  useEffect(() => {
    let cancelled = false;
    setError(null);
    setLoading(true);
    setDirty(false);
    readyForDirtyRef.current = false;

    (async () => {
      try {
        // Read file bytes first; pull Univer in parallel so the heavy
        // dynamic import overlaps with the IPC read. The CSS import is
        // critical — without it Univer's toolbar renders as a vertical
        // stack of unstyled labels and the canvas grid doesn't appear
        // (we hit this on first integration). The bundle ships its full
        // theme in a single 79 KB stylesheet via the preset entry.
        const [{ createUniver }, { UniverSheetsCorePreset }, enUS, _css, res] = await Promise.all([
          import('@univerjs/presets'),
          import('@univerjs/preset-sheets-core'),
          import('@univerjs/preset-sheets-core/locales/en-US'),
          import('@univerjs/preset-sheets-core/lib/index.css'),
          invoke('read_file_base64', { path: tab.path }),
        ]);
        if (cancelled) return;

        // Build the workbook snapshot. CSV bypasses ExcelJS, XLSX goes
        // through it for full style preservation.
        let snapshot;
        if (isCsv) {
          const decoded = new TextDecoder('utf-8').decode(base64ToBytes(res.data));
          snapshot = csvTextToUniverSnapshot(decoded, basename(tab.path));
        } else {
          const bytes = base64ToBytes(res.data);
          snapshot = await excelJsBufferToUniver(bytes.buffer, basename(tab.path));
        }
        if (cancelled) return;

        // The container must exist before createUniver wires up DOM.
        const container = containerRef.current;
        if (!container) return;
        // Strip any leftover DOM from a prior mount (HMR or fast tab
        // re-open) so Univer doesn't render into a stale node.
        container.innerHTML = '';

        const { univer, univerAPI } = createUniver({
          presets: [
            UniverSheetsCorePreset({
              container,
              // Hide the formula bar by default — the user explicitly
              // asked us to drop it in a prior turn. Toolbar, sheet
              // tabs, and the bottom status row stay visible.
              formulaBar: false,
            }),
          ],
          // Univer ships dark mode natively; we don't have to do any
          // CSS filter-inversion hacks here.
          darkMode: true,
          locale: 'enUS',
          locales: {
            enUS: enUS.default || enUS,
          },
        });
        if (cancelled) {
          univer.dispose();
          return;
        }
        univerRef.current = univer;
        univerAPIRef.current = univerAPI;

        univerAPI.createWorkbook(snapshot);

        // Wire change tracking. SheetValueChanged fires on every cell
        // edit, paste, fill, sort — anything that touches values. We
        // skip the bursts that happen during initial workbook creation
        // by gating on `readyForDirtyRef`.
        const dispose = univerAPI.addEvent(
          univerAPI.Event.SheetValueChanged,
          () => {
            if (!readyForDirtyRef.current) return;
            setDirty(true);
          },
        );
        // Univer's initial event burst is synchronous-ish; wait one
        // task tick before opening the dirty gate.
        setTimeout(() => { readyForDirtyRef.current = true; }, 0);

        setLoading(false);

        // Store the dispose on the univer instance so the outer cleanup
        // takes it down alongside the framework.
        univer.__rusticEventDispose = dispose;
      } catch (e) {
        if (!cancelled) {
          // Univer's load errors are usually a nested chain — strip to
          // the leaf message so toasts stay readable.
          const msg = e?.message || String(e);
          setError(`Couldn't open spreadsheet: ${msg}`);
          setLoading(false);
        }
      }
    })();

    return () => {
      cancelled = true;
      const univer = univerRef.current;
      univerRef.current = null;
      univerAPIRef.current = null;
      // Univer's `dispose()` synchronously calls `ReactDOMRoot.unmount()`
      // on its internal React root. In React 19, unmounting one root
      // from inside another root's commit/cleanup phase trips
      // "Attempted to synchronously unmount a root while React was
      // already rendering." Push the teardown into a microtask so it
      // runs after the outer commit finishes — the outer React tree
      // doesn't observe Univer's DOM after that anyway.
      if (univer) {
        Promise.resolve().then(() => {
          try { univer.__rusticEventDispose?.dispose?.(); } catch {}
          try { univer.dispose(); } catch {}
        });
      }
    };
    // We deliberately don't depend on `isCsv` — that's derived from
    // tab.path so a path change covers it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.path, reloadVersion]);

  // Mirror local `dirty` into the editor store so the tab gets a yellow
  // dot. Clear on unmount so a closed tab doesn't leave a phantom dot.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);

  // ── Save ─────────────────────────────────────────────────────────
  const onSave = async () => {
    const api = univerAPIRef.current;
    if (!api || saving) return;
    setSaving(true);
    try {
      const wb = api.getActiveWorkbook?.();
      if (!wb) throw new Error('No active workbook');
      const snapshot = wb.save();

      if (isCsv) {
        const csv = univerSnapshotToCsvText(snapshot);
        const b64 = bytesToBase64(new TextEncoder().encode(csv));
        await invoke('write_file_base64', { path: tab.path, data: b64 });
      } else {
        const xwb = await univerSnapshotToExcelJs(snapshot);
        const arrayBuffer = await xwb.xlsx.writeBuffer();
        const b64 = bytesToBase64(new Uint8Array(arrayBuffer));
        await invoke('write_file_base64', { path: tab.path, data: b64 });
      }
      setDirty(false);
      toast.success('Saved');
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Save failed: ${msg}`);
    } finally {
      setSaving(false);
    }
  };

  // Register Ctrl+S handler while we're the active editor.
  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.path, dirty]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  // The container needs to exist before Univer mounts, so we always
  // render it. The Skeleton overlay covers it while the (relatively
  // heavy) Univer bundle is still loading on first open.
  return (
    <div className="flex h-full w-full flex-col">
      <div className="rustic-univer-host relative min-h-0 flex-1">
        <div ref={containerRef} className="absolute inset-0" />
        {loading && (
          <div className="absolute inset-0 flex items-center justify-center bg-background/80">
            <div className="flex flex-col gap-2 p-4">
              <Skeleton className="h-4 w-48" />
              <Skeleton className="h-64 w-96" />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
