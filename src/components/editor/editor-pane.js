import { el } from '../../utils/dom.js';
import { editorStore, updateBufferModified, saveActiveBuffer, closeBuffer, setActiveBuffer, openFile } from '../../state/editor.js';
import * as api from '../../lib/tauri-api.js';
import { renderLine } from './line-renderer.js';
import { renderGutter } from './gutter-renderer.js';
import { createAutocomplete } from './autocomplete.js';
import { createHoverTooltip } from './hover-tooltip.js';
import { createFindReplace } from './find-replace.js';

const LINE_HEIGHT = 20;
const OVERSCAN = 30;
const LINES_PADDING_LEFT = 4;
const TAB_SIZE = 4;

export function createEditorPane() {
  const container = el('div', { class: 'editor-pane' });

  // Gutter
  const gutterEl = el('div', { class: 'editor-gutter-container' });
  const gutterSpacer = el('div', { class: 'editor-gutter-spacer' });
  const gutterContent = el('div', { class: 'editor-gutter-content' });
  gutterEl.appendChild(gutterSpacer);
  gutterEl.appendChild(gutterContent);

  // Code area
  const codeWrapper = el('div', { class: 'editor-code-wrapper' });
  const scrollContainer = el('div', { class: 'editor-scroll-container' });
  const spacer = el('div', { class: 'editor-spacer' });
  const selectionLayer = el('div', { class: 'editor-selection-layer' });
  const linesContainer = el('div', { class: 'editor-lines-container' });

  const matchHighlightLayer = el('div', { class: 'editor-match-highlight-layer' });
  scrollContainer.appendChild(spacer);
  scrollContainer.appendChild(selectionLayer);
  scrollContainer.appendChild(matchHighlightLayer);
  scrollContainer.appendChild(linesContainer);
  codeWrapper.appendChild(scrollContainer);

  // Find/Replace widget
  const findReplace = createFindReplace();
  codeWrapper.appendChild(findReplace.element);

  // Hidden textarea for input
  const textarea = el('textarea', {
    class: 'editor-hidden-input',
    autocomplete: 'off',
    autocorrect: 'off',
    autocapitalize: 'off',
    spellcheck: 'false',
    tabindex: '0',
  });

  // Cursor element
  const cursor = el('div', { class: 'editor-cursor' });

  // Autocomplete popup
  const autocomplete = createAutocomplete((text) => {
    if (currentBufferId && text) {
      editAtCursor(text).then(() => {
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      });
    }
  });

  // Hover tooltip
  const hoverTooltip = createHoverTooltip();

  container.appendChild(gutterEl);
  container.appendChild(codeWrapper);
  container.appendChild(textarea);
  container.appendChild(cursor);
  container.appendChild(autocomplete.element);
  container.appendChild(hoverTooltip.element);

  // ===================== STATE =====================
  let currentBufferId = null;
  let docVersion = 1;
  let lineCount = 0;
  let visibleStart = 0;
  let visibleEnd = 0;
  let renderedLines = [];
  let cursorLine = 0;
  let cursorCol = 0;
  let isComposing = false;

  // Selection state (-1 = no selection)
  let selAnchorLine = -1, selAnchorCol = -1;
  let selHeadLine = -1, selHeadCol = -1;
  let isDragging = false;

  // ===================== SELECTION HELPERS =====================
  function hasSelection() {
    return selAnchorLine >= 0 &&
      (selAnchorLine !== selHeadLine || selAnchorCol !== selHeadCol);
  }

  function getSelectionRange() {
    if (selAnchorLine < selHeadLine ||
        (selAnchorLine === selHeadLine && selAnchorCol <= selHeadCol)) {
      return { startLine: selAnchorLine, startCol: selAnchorCol, endLine: selHeadLine, endCol: selHeadCol };
    }
    return { startLine: selHeadLine, startCol: selHeadCol, endLine: selAnchorLine, endCol: selAnchorCol };
  }

  function clearSelection() {
    selAnchorLine = selAnchorCol = selHeadLine = selHeadCol = -1;
  }

  function selectAll() {
    if (lineCount === 0) return;
    selAnchorLine = 0;
    selAnchorCol = 0;
    const lastLine = lineCache.get(lineCount);
    selHeadLine = lineCount - 1;
    selHeadCol = lastLine ? lastLine.text.length : 0;
    cursorLine = selHeadLine;
    cursorCol = selHeadCol;
    editorStore.setState({ cursorLine, cursorCol });
    updateCursorPosition();
    renderSelection();
  }

  function getSelectedText() {
    if (!hasSelection()) return '';
    const { startLine, startCol, endLine, endCol } = getSelectionRange();
    const parts = [];
    for (let i = startLine; i <= endLine; i++) {
      const cached = lineCache.get(i + 1);
      if (!cached) continue;
      const text = cached.text;
      if (i === startLine && i === endLine) {
        parts.push(text.substring(startCol, endCol));
      } else if (i === startLine) {
        parts.push(text.substring(startCol));
      } else if (i === endLine) {
        parts.push(text.substring(0, endCol));
      } else {
        parts.push(text);
      }
    }
    return parts.join('\n');
  }

  function startSelectionAt(line, col) {
    selAnchorLine = line;
    selAnchorCol = col;
    selHeadLine = line;
    selHeadCol = col;
  }

  function extendSelectionTo(line, col) {
    if (selAnchorLine < 0) {
      selAnchorLine = cursorLine;
      selAnchorCol = cursorCol;
    }
    selHeadLine = line;
    selHeadCol = col;
  }

  function isWordChar(ch) {
    return /[\w$]/.test(ch);
  }

  function selectWordAt(line, col) {
    const cached = lineCache.get(line + 1);
    if (!cached) return;
    const text = cached.text;
    let start = col, end = col;
    while (start > 0 && isWordChar(text[start - 1])) start--;
    while (end < text.length && isWordChar(text[end])) end++;
    selAnchorLine = line; selAnchorCol = start;
    selHeadLine = line; selHeadCol = end;
    cursorLine = line; cursorCol = end;
  }

  // ===================== LINE CACHE =====================
  const lineCache = new Map();
  let fetchGeneration = 0;

  async function loadAllLines(bufferId) {
    if (!bufferId || lineCount === 0) return;
    const gen = ++fetchGeneration;
    try {
      const lines = await api.getVisibleLines(bufferId, 0, lineCount);
      if (gen !== fetchGeneration) return;
      if (!lines || editorStore.getState('activeBufferId') !== bufferId) return;
      lineCache.clear();
      for (const line of lines) lineCache.set(line.line_number, line);
      renderFromCache();
      if (findReplace.isVisible()) doFindSearch();
    } catch (e) {
      console.error('Failed to load lines:', e);
    }
  }

  function reloadAllLines() {
    lineCache.clear();
    loadAllLines(currentBufferId);
  }

  function renderFromCache() {
    const lines = [];
    for (let i = visibleStart; i < visibleEnd; i++) {
      const cached = lineCache.get(i + 1);
      if (cached) lines.push(cached);
    }
    if (lines.length > 0) {
      renderedLines = lines;
      renderVisibleLines();
    }
  }

  // ===================== CHAR WIDTH =====================
  let _charWidth = 0;
  function getCharWidth() {
    if (_charWidth > 0) return _charWidth;
    const span = document.createElement('span');
    span.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;' +
      'font-family:var(--font-family-mono);font-size:var(--font-size-editor);line-height:20px;';
    span.textContent = 'X'.repeat(100);
    // Measure inside the editor container so the span inherits the exact
    // same rendering context (font smoothing, hinting, zoom, etc.)
    // as the actual editor text. Falls back to body before mount.
    const parent = container.isConnected ? container : document.body;
    parent.appendChild(span);
    const rawWidth = span.getBoundingClientRect().width / 100;
    parent.removeChild(span);
    // When measured inside a zoomed container, getBoundingClientRect()
    // returns the zoomed size — divide out zoom to get CSS-space width.
    _charWidth = parent === document.body ? rawWidth : rawWidth / getZoom();
    return _charWidth;
  }
  window.addEventListener('resize', () => { _charWidth = 0; });

  // Remeasure after all fonts have loaded — prevents stale cache when a
  // web/system font finishes loading after the first getCharWidth() call.
  document.fonts.ready.then(() => {
    _charWidth = 0;
    if (currentBufferId) {
      updateGutterWidth();
      renderFromCache();
    }
  });

  // ===================== ZOOM HELPER =====================
  function getZoom() {
    return parseFloat(document.getElementById('app')?.style.zoom) || 1;
  }

  // ===================== TAB-AWARE COLUMN HELPERS =====================
  /** Convert a character column (index in text) to the visual column (display position). */
  function charColToVisualCol(text, charCol) {
    let visual = 0;
    const end = Math.min(charCol, text.length);
    for (let i = 0; i < end; i++) {
      if (text[i] === '\t') {
        visual = (Math.floor(visual / TAB_SIZE) + 1) * TAB_SIZE;
      } else {
        visual++;
      }
    }
    return visual;
  }

  /** Convert a fractional visual column (from click position) to a character column. */
  function visualColToCharCol(text, visualX) {
    let visual = 0;
    for (let i = 0; i < text.length; i++) {
      const prev = visual;
      if (text[i] === '\t') {
        visual = (Math.floor(visual / TAB_SIZE) + 1) * TAB_SIZE;
      } else {
        visual++;
      }
      if (visual > visualX) {
        // Click falls within this character's visual span — snap to closer edge
        return (visualX - prev) >= (visual - prev) / 2 ? i + 1 : i;
      }
    }
    return text.length;
  }

  // ===================== GUTTER WIDTH =====================
  function updateGutterWidth() {
    const digits = Math.max(2, String(lineCount).length);
    // 8px left + 12px right padding on gutter lines = 20, plus 4px buffer
    const width = Math.ceil(digits * getCharWidth() + 24);
    gutterEl.style.minWidth = `${Math.max(width, 50)}px`;
  }

  // ===================== RENDERING =====================
  function renderVisibleLines() {
    if (renderedLines.length === 0) return;
    const startLine = renderedLines[0].line_number - 1;

    gutterContent.replaceChildren(renderGutter(renderedLines, cursorLine + 1));
    gutterContent.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    const frag = document.createDocumentFragment();
    for (const line of renderedLines) frag.appendChild(renderLine(line));
    linesContainer.replaceChildren(frag);
    linesContainer.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    updateCursorPosition();
    renderSelection();
    renderMatchHighlights();
  }

  function renderSelection() {
    selectionLayer.replaceChildren();
    if (!hasSelection()) return;

    const { startLine, startCol, endLine, endCol } = getSelectionRange();
    const charWidth = getCharWidth();
    const frag = document.createDocumentFragment();

    const lo = Math.max(startLine, visibleStart);
    const hi = Math.min(endLine, visibleEnd - 1);

    for (let i = lo; i <= hi; i++) {
      const cached = lineCache.get(i + 1);
      if (!cached) continue;
      const lineLen = cached.text.length;

      let sCol = 0, eCol = lineLen + 1; // +1 to show newline selection
      if (i === startLine) sCol = startCol;
      if (i === endLine) eCol = endCol;
      if (sCol >= eCol && i === endLine) continue;

      // Convert character columns to visual columns for display
      const visualSCol = charColToVisualCol(cached.text, Math.min(sCol, lineLen));
      const visualECol = eCol > lineLen
        ? charColToVisualCol(cached.text, lineLen) + 1
        : charColToVisualCol(cached.text, eCol);

      const div = document.createElement('div');
      div.className = 'editor-selection';
      div.style.cssText =
        `top:${i * LINE_HEIGHT}px;left:${visualSCol * charWidth + LINES_PADDING_LEFT}px;` +
        `width:${Math.max(charWidth * 0.5, (visualECol - visualSCol) * charWidth)}px;height:${LINE_HEIGHT}px;`;
      frag.appendChild(div);
    }
    selectionLayer.replaceChildren(frag);
  }

  function updateCursorPosition() {
    const charWidth = getCharWidth();
    const zoom = getZoom();
    // Measure actual offset between container and scrollContainer.
    // getBoundingClientRect() returns viewport-space (zoomed) values,
    // so divide by zoom to convert back to CSS-space for positioning.
    const containerRect = container.getBoundingClientRect();
    const scrollRect = scrollContainer.getBoundingClientRect();
    const offsetY = (scrollRect.top - containerRect.top) / zoom;
    const offsetX = (scrollRect.left - containerRect.left) / zoom;

    // Convert character column to visual column for display positioning
    const cached = lineCache.get(cursorLine + 1);
    const visualCol = cached ? charColToVisualCol(cached.text, cursorCol) : cursorCol;

    cursor.style.top = `${offsetY + cursorLine * LINE_HEIGHT - scrollContainer.scrollTop}px`;
    cursor.style.left = `${offsetX + visualCol * charWidth + LINES_PADDING_LEFT}px`;
    cursor.style.height = `${LINE_HEIGHT}px`;
    cursor.style.display = currentBufferId ? 'block' : 'none';
  }

  // ===================== CLICK POSITION HELPER =====================
  function getClickPos(e) {
    const zoom = getZoom();
    const rect = scrollContainer.getBoundingClientRect();
    // clientX/clientY and getBoundingClientRect() are in viewport-space (zoomed),
    // but scrollTop/scrollLeft and LINE_HEIGHT/charWidth are in CSS-space (unzoomed).
    // Divide the viewport-space offset by zoom to convert to CSS-space.
    const relY = (e.clientY - rect.top) / zoom + scrollContainer.scrollTop;
    const relX = (e.clientX - rect.left) / zoom - LINES_PADDING_LEFT + scrollContainer.scrollLeft;
    const charWidth = getCharWidth();
    const line = Math.max(0, Math.min(lineCount - 1, Math.floor(relY / LINE_HEIGHT)));
    // Convert pixel offset to a fractional visual column, then map to
    // the actual character column — accounts for tab characters.
    const visualX = Math.max(0, relX / charWidth);
    const cached = lineCache.get(line + 1);
    let col;
    if (cached) {
      col = visualColToCharCol(cached.text, visualX);
      col = Math.min(col, cached.text.length);
    } else {
      col = Math.max(0, Math.round(visualX));
    }
    return { line, col };
  }

  // ===================== SCROLL =====================
  function computeVisibleRange(scrollTop) {
    const vh = scrollContainer.clientHeight;
    return {
      newStart: Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN),
      newEnd: Math.min(lineCount, Math.ceil((scrollTop + vh) / LINE_HEIGHT) + OVERSCAN),
    };
  }

  scrollContainer.addEventListener('scroll', () => {
    gutterEl.scrollTop = scrollContainer.scrollTop;
    updateCursorPosition();
    if (!currentBufferId) return;
    editorStore.setState({ scrollTop: scrollContainer.scrollTop });
    const { newStart, newEnd } = computeVisibleRange(scrollContainer.scrollTop);
    if (newStart !== visibleStart || newEnd !== visibleEnd) {
      visibleStart = newStart;
      visibleEnd = newEnd;
      renderFromCache();
    }
  });

  // ===================== MOUSE HANDLERS =====================
  // Hover tooltip
  codeWrapper.addEventListener('mousemove', (e) => {
    if (!currentBufferId || isDragging) return;
    const zoom = getZoom();
    const rect = scrollContainer.getBoundingClientRect();
    const relY = (e.clientY - rect.top) / zoom + scrollContainer.scrollTop;
    const relX = (e.clientX - rect.left) / zoom - LINES_PADDING_LEFT;
    if (relX < 0) { hoverTooltip.cancelSchedule(); return; }
    const charWidth = getCharWidth();
    const hoverLine = Math.floor(relY / LINE_HEIGHT);
    const visualX = Math.max(0, relX / charWidth);
    const cached = lineCache.get(hoverLine + 1);
    const hoverCol = cached ? visualColToCharCol(cached.text, visualX) : Math.round(visualX);
    hoverTooltip.scheduleShow(currentBufferId, hoverLine, hoverCol, e.clientX, e.clientY);
  });
  codeWrapper.addEventListener('mouseleave', () => { hoverTooltip.hide(); });

  // Ctrl+Click: go to definition
  container.addEventListener('click', (e) => {
    if (e.ctrlKey && currentBufferId) {
      const { line, col } = getClickPos(e);
      api.gotoDefinition(currentBufferId, line, col).then((locs) => {
        if (locs && locs.length > 0) openFile(locs[0].file_path);
      }).catch(() => {});
    }
  });

  // Always keep textarea focused when clicking anywhere in the editor
  container.addEventListener('mousedown', (e) => {
    if (currentBufferId) {
      e.preventDefault(); // Prevent browser from moving focus to the clicked element
      textarea.focus();
    }
  });

  // Mousedown: cursor placement + selection start + double-click word select
  scrollContainer.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || !currentBufferId) return;
    hoverTooltip.hide();
    autocomplete.hide();

    const { line, col } = getClickPos(e);

    // Double-click: select word
    if (e.detail === 2) {
      selectWordAt(line, col);
      editorStore.setState({ cursorLine, cursorCol });
      updateCursorPosition();
      renderSelection();
      return;
    }

    // Triple-click: select line
    if (e.detail >= 3) {
      const cached = lineCache.get(line + 1);
      selAnchorLine = line; selAnchorCol = 0;
      selHeadLine = line; selHeadCol = cached ? cached.text.length : 0;
      cursorLine = line; cursorCol = selHeadCol;
      editorStore.setState({ cursorLine, cursorCol });
      updateCursorPosition();
      renderSelection();
      return;
    }

    // Shift+click: extend selection
    if (e.shiftKey) {
      extendSelectionTo(line, col);
    } else {
      clearSelection();
      startSelectionAt(line, col);
    }

    cursorLine = line;
    cursorCol = col;
    isDragging = true;
    editorStore.setState({ cursorLine, cursorCol });
    updateCursorPosition();
    renderSelection();
  });

  // Drag selection
  document.addEventListener('mousemove', (e) => {
    if (!isDragging) return;
    const { line, col } = getClickPos(e);
    selHeadLine = line;
    selHeadCol = col;
    cursorLine = line;
    cursorCol = col;
    editorStore.setState({ cursorLine, cursorCol });
    updateCursorPosition();
    renderSelection();
    // Auto-scroll when dragging near edges
    const rect = scrollContainer.getBoundingClientRect();
    if (e.clientY < rect.top + 20) {
      scrollContainer.scrollTop -= LINE_HEIGHT;
    } else if (e.clientY > rect.bottom - 20) {
      scrollContainer.scrollTop += LINE_HEIGHT;
    }
  });

  document.addEventListener('mouseup', () => {
    if (!isDragging) return;
    isDragging = false;
    // If no actual selection range, clear it
    if (selAnchorLine === selHeadLine && selAnchorCol === selHeadCol) {
      clearSelection();
      renderSelection();
    }
  });

  // ===================== CLIPBOARD =====================
  async function copyToClipboard(text) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Fallback
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.cssText = 'position:fixed;left:-9999px';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    }
  }

  async function readFromClipboard() {
    try {
      return await navigator.clipboard.readText();
    } catch {
      return '';
    }
  }

  // ===================== EDIT HELPERS =====================
  /**
   * Edit that respects selection: if selection exists, replace it with newText.
   * Otherwise insert newText at cursor. Returns the edit result.
   */
  async function editAtCursor(newText) {
    if (!currentBufferId) return null;
    let editLine = cursorLine, editCol = cursorCol, deleteCount = 0;

    if (hasSelection()) {
      const { startLine, startCol } = getSelectionRange();
      const selText = getSelectedText();
      deleteCount = new TextEncoder().encode(selText).length;
      editLine = startLine;
      editCol = startCol;
      clearSelection();
    }

    try {
      const result = await api.editBuffer(currentBufferId, editLine, editCol, newText, deleteCount);
      if (result) {
        lineCount = result.line_count;
        if (newText.includes('\n')) {
          const parts = newText.split('\n');
          cursorLine = editLine + parts.length - 1;
          cursorCol = parts[parts.length - 1].length;
        } else {
          cursorLine = editLine;
          cursorCol = editCol + newText.length;
        }
        editorStore.setState({ cursorLine, cursorCol });
        updateBufferModified(currentBufferId, result.is_modified, result.line_count);
        updateSpacerHeights();
        reloadAllLines();
        renderSelection();
        return result;
      }
    } catch (e) {
      console.error('Edit failed:', e);
    }
    return null;
  }

  function updateSpacerHeights() {
    spacer.style.height = `${lineCount * LINE_HEIGHT}px`;
    gutterSpacer.style.height = `${lineCount * LINE_HEIGHT}px`;
    updateGutterWidth();
  }

  // ===================== INPUT HANDLING =====================
  textarea.addEventListener('compositionstart', () => { isComposing = true; });
  textarea.addEventListener('compositionend', () => { isComposing = false; handleInput(); });
  textarea.addEventListener('input', () => { if (!isComposing) handleInput(); });

  async function handleInput() {
    const text = textarea.value;
    if (!text || !currentBufferId) return;
    textarea.value = '';
    await editAtCursor(text);
    docVersion++;
    api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
  }

  // ===================== KEYBOARD SHORTCUTS =====================
  textarea.addEventListener('keydown', async (e) => {
    if (!currentBufferId) return;
    if (autocomplete.handleKey(e)) return;

    const shift = e.shiftKey;
    const ctrl = e.ctrlKey;

    // --- Find / Replace ---
    if (e.key === 'Escape' && findReplace.isVisible()) {
      e.preventDefault();
      closeFindReplace();
      return;
    }
    if (ctrl && e.key === 'f') {
      e.preventDefault();
      const sel = hasSelection() ? getSelectedText() : '';
      findReplace.show(false, sel.includes('\n') ? '' : sel);
      return;
    }
    if (ctrl && e.key === 'h') {
      e.preventDefault();
      const sel = hasSelection() ? getSelectedText() : '';
      findReplace.show(true, sel.includes('\n') ? '' : sel);
      return;
    }
    if (e.key === 'F3' && findReplace.isVisible() && findMatches.length > 0) {
      e.preventDefault();
      navigateMatch(shift ? 'prev' : 'next');
      return;
    }

    // --- Clipboard ---
    if (ctrl && e.key === 'c') {
      e.preventDefault();
      if (hasSelection()) await copyToClipboard(getSelectedText());
      return;
    }
    if (ctrl && e.key === 'x') {
      e.preventDefault();
      if (hasSelection()) {
        await copyToClipboard(getSelectedText());
        await editAtCursor('');
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      }
      return;
    }
    if (ctrl && e.key === 'v') {
      e.preventDefault();
      const text = await readFromClipboard();
      if (text) {
        await editAtCursor(text);
        ensureCursorVisible();
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      }
      return;
    }

    // --- Select all ---
    if (ctrl && e.key === 'a') {
      e.preventDefault();
      selectAll();
      return;
    }

    // --- Ctrl+Space: autocomplete ---
    if (ctrl && e.key === ' ') {
      e.preventDefault();
      const cw = getCharWidth();
      const zm = getZoom();
      const cRect = container.getBoundingClientRect();
      const sRect = scrollContainer.getBoundingClientRect();
      const oX = (sRect.left - cRect.left) / zm;
      const oY = (sRect.top - cRect.top) / zm;
      const acCached = lineCache.get(cursorLine + 1);
      const acVisualCol = acCached ? charColToVisualCol(acCached.text, cursorCol) : cursorCol;
      autocomplete.show(currentBufferId, cursorLine, cursorCol,
        oX + acVisualCol * cw + LINES_PADDING_LEFT,
        oY + cursorLine * LINE_HEIGHT - scrollContainer.scrollTop + LINE_HEIGHT);
      return;
    }

    // --- F12: go to definition ---
    if (e.key === 'F12') {
      e.preventDefault();
      try {
        const locs = await api.gotoDefinition(currentBufferId, cursorLine, cursorCol);
        if (locs && locs.length > 0) openFile(locs[0].file_path);
      } catch (err) { console.error('Goto definition failed:', err); }
      return;
    }

    // --- Ctrl+Shift+I: format ---
    if (ctrl && shift && e.key === 'I') {
      e.preventDefault();
      try { await api.formatDocument(currentBufferId); reloadAllLines(); } catch (err) {}
      return;
    }

    // --- Ctrl+S: save ---
    if (ctrl && e.key === 's') {
      e.preventDefault();
      await saveActiveBuffer();
      api.lspNotifySave(currentBufferId).catch(() => {});
      return;
    }

    // --- Ctrl+W: close tab ---
    if (ctrl && e.key === 'w') {
      e.preventDefault();
      if (currentBufferId) closeBuffer(currentBufferId);
      return;
    }

    // --- Ctrl+Tab: cycle tabs ---
    if (ctrl && e.key === 'Tab') {
      e.preventDefault();
      const buffers = editorStore.getState('openBuffers');
      const ids = Object.keys(buffers).map(Number);
      if (ids.length < 2) return;
      const idx = ids.indexOf(currentBufferId);
      const next = shift ? (idx - 1 + ids.length) % ids.length : (idx + 1) % ids.length;
      setActiveBuffer(ids[next]);
      return;
    }

    // --- Ctrl+Z: undo ---
    if (ctrl && e.key === 'z' && !shift) {
      e.preventDefault();
      try {
        const r = await api.undoEdit(currentBufferId);
        if (r) { lineCount = r.line_count; clearSelection(); updateBufferModified(currentBufferId, r.is_modified, r.line_count); updateSpacerHeights(); reloadAllLines(); renderSelection(); }
      } catch (e) {}
      return;
    }

    // --- Ctrl+Y / Ctrl+Shift+Z: redo ---
    if ((ctrl && e.key === 'y') || (ctrl && shift && e.key === 'Z')) {
      e.preventDefault();
      try {
        const r = await api.redoEdit(currentBufferId);
        if (r) { lineCount = r.line_count; clearSelection(); updateBufferModified(currentBufferId, r.is_modified, r.line_count); updateSpacerHeights(); reloadAllLines(); renderSelection(); }
      } catch (e) {}
      return;
    }

    // --- Enter ---
    if (e.key === 'Enter') {
      e.preventDefault();
      textarea.value = '';
      await editAtCursor('\n');
      ensureCursorVisible();
      docVersion++;
      api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      return;
    }

    // --- Backspace ---
    if (e.key === 'Backspace') {
      e.preventDefault();
      textarea.value = '';
      if (hasSelection()) {
        await editAtCursor('');
      } else if (cursorCol > 0) {
        try {
          const r = await api.editBuffer(currentBufferId, cursorLine, cursorCol - 1, '', 1);
          if (r) { cursorCol--; lineCount = r.line_count; editorStore.setState({ cursorLine, cursorCol }); updateBufferModified(currentBufferId, r.is_modified, r.line_count); reloadAllLines(); }
        } catch (err) {}
      } else if (cursorLine > 0) {
        const prev = lineCache.get(cursorLine); // cursorLine is 0-based, this gets the previous line (line_number = cursorLine)
        const prevLen = prev ? prev.text.length : 0;
        try {
          const r = await api.editBuffer(currentBufferId, cursorLine, 0, '', 1);
          if (r) { cursorLine--; cursorCol = prevLen; lineCount = r.line_count; editorStore.setState({ cursorLine, cursorCol }); updateBufferModified(currentBufferId, r.is_modified, r.line_count); updateSpacerHeights(); reloadAllLines(); }
        } catch (err) {}
      }
      docVersion++;
      api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      return;
    }

    // --- Delete ---
    if (e.key === 'Delete') {
      e.preventDefault();
      textarea.value = '';
      if (hasSelection()) {
        await editAtCursor('');
      } else {
        try {
          const r = await api.editBuffer(currentBufferId, cursorLine, cursorCol, '', 1);
          if (r) { lineCount = r.line_count; updateBufferModified(currentBufferId, r.is_modified, r.line_count); updateSpacerHeights(); reloadAllLines(); }
        } catch (err) {}
      }
      docVersion++;
      api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      return;
    }

    // --- Tab ---
    if (e.key === 'Tab') {
      e.preventDefault();
      textarea.value = '';
      await editAtCursor('    ');
      docVersion++;
      api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      return;
    }

    // ===================== NAVIGATION KEYS =====================
    // Helper: move cursor, optionally extending selection
    function moveTo(line, col) {
      cursorLine = line;
      cursorCol = col;
      if (shift) {
        extendSelectionTo(line, col);
      } else if (hasSelection()) {
        clearSelection();
      }
      editorStore.setState({ cursorLine, cursorCol });
      updateCursorPosition();
      renderSelection();
      ensureCursorVisible();
    }

    // --- Ctrl+Home: start of file ---
    if (ctrl && e.key === 'Home') {
      e.preventDefault();
      moveTo(0, 0);
      return;
    }
    // --- Ctrl+End: end of file ---
    if (ctrl && e.key === 'End') {
      e.preventDefault();
      const lastLine = lineCache.get(lineCount);
      moveTo(lineCount - 1, lastLine ? lastLine.text.length : 0);
      return;
    }

    // --- Home ---
    if (e.key === 'Home') {
      e.preventDefault();
      moveTo(cursorLine, 0);
      return;
    }
    // --- End ---
    if (e.key === 'End') {
      e.preventDefault();
      const ln = lineCache.get(cursorLine + 1);
      moveTo(cursorLine, ln ? ln.text.length : 0);
      return;
    }

    // --- Page Up ---
    if (e.key === 'PageUp') {
      e.preventDefault();
      const pageLines = Math.floor(scrollContainer.clientHeight / LINE_HEIGHT);
      const newLine = Math.max(0, cursorLine - pageLines);
      clampCursorColForLine(newLine);
      moveTo(newLine, cursorCol);
      return;
    }
    // --- Page Down ---
    if (e.key === 'PageDown') {
      e.preventDefault();
      const pageLines = Math.floor(scrollContainer.clientHeight / LINE_HEIGHT);
      const newLine = Math.min(lineCount - 1, cursorLine + pageLines);
      clampCursorColForLine(newLine);
      moveTo(newLine, cursorCol);
      return;
    }

    // --- Arrow Up ---
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (!shift && hasSelection()) {
        const { startLine, startCol } = getSelectionRange();
        moveTo(startLine, startCol);
      } else if (cursorLine > 0) {
        const newLine = cursorLine - 1;
        clampCursorColForLine(newLine);
        moveTo(newLine, cursorCol);
      }
      return;
    }
    // --- Arrow Down ---
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!shift && hasSelection()) {
        const { endLine, endCol } = getSelectionRange();
        moveTo(endLine, endCol);
      } else if (cursorLine < lineCount - 1) {
        const newLine = cursorLine + 1;
        clampCursorColForLine(newLine);
        moveTo(newLine, cursorCol);
      }
      return;
    }
    // --- Arrow Left ---
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      if (!shift && hasSelection()) {
        const { startLine, startCol } = getSelectionRange();
        moveTo(startLine, startCol);
      } else if (cursorCol > 0) {
        moveTo(cursorLine, cursorCol - 1);
      } else if (cursorLine > 0) {
        const prevLn = lineCache.get(cursorLine);
        moveTo(cursorLine - 1, prevLn ? prevLn.text.length : 0);
      }
      return;
    }
    // --- Arrow Right ---
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      if (!shift && hasSelection()) {
        const { endLine, endCol } = getSelectionRange();
        moveTo(endLine, endCol);
      } else {
        const ln = lineCache.get(cursorLine + 1);
        const maxCol = ln ? ln.text.length : 0;
        if (cursorCol < maxCol) {
          moveTo(cursorLine, cursorCol + 1);
        } else if (cursorLine < lineCount - 1) {
          moveTo(cursorLine + 1, 0);
        }
      }
      return;
    }
  });

  function clampCursorColForLine(line) {
    const ln = lineCache.get(line + 1);
    if (ln) cursorCol = Math.min(cursorCol, ln.text.length);
  }

  function ensureCursorVisible() {
    const cursorY = cursorLine * LINE_HEIGHT;
    const viewTop = scrollContainer.scrollTop;
    const viewBottom = viewTop + scrollContainer.clientHeight;
    if (cursorY < viewTop) scrollContainer.scrollTop = cursorY;
    else if (cursorY + LINE_HEIGHT > viewBottom) scrollContainer.scrollTop = cursorY - scrollContainer.clientHeight + LINE_HEIGHT;
  }

  // ===================== FIND / REPLACE =====================
  let findMatches = [];
  let currentMatchIdx = -1;
  let lastSearchOpts = {};

  function escapeRegex(str) {
    return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  function searchInBuffer(query, opts) {
    const matches = [];
    if (!query) return matches;
    try {
      let pattern = opts.useRegex ? query : escapeRegex(query);
      if (opts.wholeWord) pattern = `\\b${pattern}\\b`;
      const re = new RegExp(pattern, opts.caseSensitive ? 'g' : 'gi');
      for (const [num, line] of lineCache) {
        re.lastIndex = 0;
        let m;
        while ((m = re.exec(line.text)) !== null) {
          matches.push({ line: num - 1, startCol: m.index, endCol: m.index + m[0].length });
          if (m[0].length === 0) break;
        }
      }
    } catch { /* invalid regex */ }
    matches.sort((a, b) => a.line - b.line || a.startCol - b.startCol);
    return matches;
  }

  function findClosestMatch() {
    for (let i = 0; i < findMatches.length; i++) {
      const m = findMatches[i];
      if (m.line > cursorLine || (m.line === cursorLine && m.startCol >= cursorCol)) return i;
    }
    return 0;
  }

  function doFindSearch() {
    if (!findReplace.isVisible() || !findReplace.getQuery()) {
      findMatches = [];
      currentMatchIdx = -1;
      renderMatchHighlights();
      findReplace.setMatchInfo(0, 0);
      return;
    }
    findMatches = searchInBuffer(findReplace.getQuery(), lastSearchOpts);
    currentMatchIdx = findMatches.length > 0 ? findClosestMatch() : -1;
    findReplace.setMatchInfo(currentMatchIdx >= 0 ? currentMatchIdx + 1 : 0, findMatches.length);
    renderMatchHighlights();
    if (currentMatchIdx >= 0) scrollToMatch(currentMatchIdx);
  }

  function renderMatchHighlights() {
    matchHighlightLayer.replaceChildren();
    if (findMatches.length === 0) return;
    const charWidth = getCharWidth();
    const frag = document.createDocumentFragment();
    for (let i = 0; i < findMatches.length; i++) {
      const m = findMatches[i];
      if (m.line < visibleStart || m.line >= visibleEnd) continue;
      const cached = lineCache.get(m.line + 1);
      if (!cached) continue;
      const vStart = charColToVisualCol(cached.text, m.startCol);
      const vEnd = charColToVisualCol(cached.text, m.endCol);
      const div = document.createElement('div');
      div.className = i === currentMatchIdx ? 'editor-match-highlight--current' : 'editor-match-highlight';
      div.style.cssText = `top:${m.line * LINE_HEIGHT}px;left:${vStart * charWidth + LINES_PADDING_LEFT}px;width:${(vEnd - vStart) * charWidth}px;height:${LINE_HEIGHT}px;`;
      frag.appendChild(div);
    }
    matchHighlightLayer.replaceChildren(frag);
  }

  function scrollToMatch(idx) {
    if (idx < 0 || idx >= findMatches.length) return;
    const m = findMatches[idx];
    cursorLine = m.line;
    cursorCol = m.startCol;
    editorStore.setState({ cursorLine, cursorCol });
    const matchY = m.line * LINE_HEIGHT;
    const viewTop = scrollContainer.scrollTop;
    const viewBottom = viewTop + scrollContainer.clientHeight;
    if (matchY < viewTop || matchY + LINE_HEIGHT > viewBottom) {
      scrollContainer.scrollTop = matchY - scrollContainer.clientHeight / 2;
    }
    updateCursorPosition();
  }

  function navigateMatch(dir) {
    if (findMatches.length === 0) return;
    if (dir === 'next') currentMatchIdx = (currentMatchIdx + 1) % findMatches.length;
    else currentMatchIdx = (currentMatchIdx - 1 + findMatches.length) % findMatches.length;
    findReplace.setMatchInfo(currentMatchIdx + 1, findMatches.length);
    renderMatchHighlights();
    scrollToMatch(currentMatchIdx);
  }

  function closeFindReplace() {
    findReplace.hide();
    findMatches = [];
    currentMatchIdx = -1;
    renderMatchHighlights();
    textarea.focus();
  }

  // Wire up find/replace callbacks
  findReplace.onSearch((query, opts) => {
    lastSearchOpts = opts;
    findMatches = searchInBuffer(query, opts);
    currentMatchIdx = findMatches.length > 0 ? findClosestMatch() : -1;
    findReplace.setMatchInfo(currentMatchIdx >= 0 ? currentMatchIdx + 1 : 0, findMatches.length);
    renderMatchHighlights();
    if (currentMatchIdx >= 0) scrollToMatch(currentMatchIdx);
  });

  findReplace.onNavigate(navigateMatch);

  findReplace.onReplace(async (replaceText) => {
    if (currentMatchIdx < 0 || !currentBufferId) return;
    const m = findMatches[currentMatchIdx];
    const cached = lineCache.get(m.line + 1);
    if (!cached) return;
    const matchText = cached.text.substring(m.startCol, m.endCol);
    const deleteCount = new TextEncoder().encode(matchText).length;
    try {
      const r = await api.editBuffer(currentBufferId, m.line, m.startCol, replaceText, deleteCount);
      if (r) {
        lineCount = r.line_count;
        updateBufferModified(currentBufferId, r.is_modified, r.line_count);
        updateSpacerHeights();
      }
    } catch (e) { console.error('Replace failed:', e); return; }
    docVersion++;
    api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
    reloadAllLines();
  });

  findReplace.onReplaceAll(async (replaceText) => {
    if (findMatches.length === 0 || !currentBufferId) return;
    for (let i = findMatches.length - 1; i >= 0; i--) {
      const m = findMatches[i];
      const cached = lineCache.get(m.line + 1);
      if (!cached) continue;
      const matchText = cached.text.substring(m.startCol, m.endCol);
      const deleteCount = new TextEncoder().encode(matchText).length;
      try {
        const r = await api.editBuffer(currentBufferId, m.line, m.startCol, replaceText, deleteCount);
        if (r) { lineCount = r.line_count; updateBufferModified(currentBufferId, r.is_modified, r.line_count); }
      } catch (e) { console.error('Replace all failed:', e); break; }
    }
    updateSpacerHeights();
    docVersion++;
    api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
    reloadAllLines();
  });

  findReplace.onClose(closeFindReplace);

  // ===================== BUFFER LIFECYCLE =====================
  function onActiveBufferChange(bufferId) {
    currentBufferId = bufferId;
    lineCache.clear();
    clearSelection();
    findMatches = [];
    currentMatchIdx = -1;

    if (!bufferId) {
      linesContainer.replaceChildren();
      gutterContent.replaceChildren();
      selectionLayer.replaceChildren();
      cursor.style.display = 'none';
      return;
    }

    const buffers = editorStore.getState('openBuffers');
    const buffer = buffers[bufferId];
    if (!buffer) return;

    lineCount = buffer.lineCount;
    cursorLine = editorStore.getState('cursorLine');
    cursorCol = editorStore.getState('cursorCol');
    const scrollTop = editorStore.getState('scrollTop');

    updateSpacerHeights();
    scrollContainer.scrollTop = scrollTop;
    gutterEl.scrollTop = scrollTop;

    const vh = scrollContainer.clientHeight || 600;
    visibleStart = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
    visibleEnd = Math.min(lineCount, Math.ceil((scrollTop + vh) / LINE_HEIGHT) + OVERSCAN);

    loadAllLines(bufferId);
    textarea.focus();
    api.lspNotifyOpen(bufferId).catch(() => {});
    docVersion = 1;
  }

  editorStore.subscribe('activeBufferId', onActiveBufferChange);
  return container;
}
