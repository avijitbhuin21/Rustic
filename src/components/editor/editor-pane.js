import { el } from '../../utils/dom.js';
import { editorStore, updateBufferModified, saveActiveBuffer, closeBuffer, setActiveBuffer, openFile } from '../../state/editor.js';
import { searchStore } from '../../state/search.js';
import * as api from '../../lib/tauri-api.js';
import { renderLine, setRendererConfig } from './line-renderer.js';
import { renderGutter } from './gutter-renderer.js';
import { createAutocomplete } from './autocomplete.js';
import { createHoverTooltip } from './hover-tooltip.js';
import { createFindReplace } from './find-replace.js';
import { settingsStore } from '../../state/settings.js';
import { uiStore } from '../../state/ui.js';

let LINE_HEIGHT = 20;
const OVERSCAN = 30;
const LINES_PADDING_LEFT = 4;
const TAB_SIZE = 4;

function computeLineHeight() {
  const fontSize = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--font-size-editor')) || 14;
  return Math.round(fontSize * 1.43);  // ~20px at 14px font
}

export function createEditorPane(groupId) {
  const paneGroupId = groupId || 1;
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

  // Minimap
  const minimapContainer = el('div', { class: 'editor-minimap' });
  const minimapCanvas = document.createElement('canvas');
  minimapCanvas.className = 'editor-minimap__canvas';
  const minimapViewport = el('div', { class: 'editor-minimap__viewport' });
  minimapContainer.appendChild(minimapCanvas);
  minimapContainer.appendChild(minimapViewport);
  minimapContainer.style.display = 'none';
  codeWrapper.appendChild(minimapContainer);

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
  // Per-buffer caches so switching tabs doesn't discard data
  const bufferCaches = new Map();
  let lineCache = new Map();
  let fetchGeneration = 0;

  /** Fetch the visible range (plain text or highlighted) and render. */
  async function loadVisibleLines(bufferId, vStart, vEnd) {
    if (!bufferId || lineCount === 0) return;
    const gen = ++fetchGeneration;
    try {
      // Returns instantly: highlighted from cache, or plain text if not yet highlighted
      const visibleLines = await api.getVisibleLines(bufferId, vStart, vEnd);
      if (gen !== fetchGeneration) return;
      if (!visibleLines || editorStore.getState('activeBufferId') !== bufferId) return;
      for (const line of visibleLines) lineCache.set(line.line_number, line);
      renderFromCache();

      // If lines came back without spans, trigger background highlighting
      const hasHighlighting = visibleLines.some(l => l.spans && l.spans.length > 0);
      if (!hasHighlighting && visibleLines.length > 0) {
        requestHighlighting(bufferId);
      }

      if (findReplace.isVisible()) doFindSearch();
      else if (searchStore.getState('query')) updateGlobalSearchHighlights();
    } catch (e) {
      console.error('[SyntaxHighlight] loadVisibleLines failed:', e);
    }
  }

  // Track per-buffer highlighting requests to avoid duplicate work
  const highlightingInFlight = new Set();
  // Track whether a full background highlight has been triggered per buffer
  const fullHighlightDone = new Set();

  /**
   * Two-phase highlighting for large files:
   * 1. FAST: highlight_range — parses the file but only returns spans for the
   *    visible viewport (~100 lines). This gives the user instant syntax colors.
   * 2. BACKGROUND: highlight_buffer — full parse that caches all lines.
   *    Once done, minimap and scroll will serve highlighted data from cache.
   */
  async function requestHighlighting(bufferId) {
    if (highlightingInFlight.has(bufferId)) return;
    highlightingInFlight.add(bufferId);
    try {
      // Phase 1: Fast viewport-only highlighting
      const vStart = visibleStart;
      const vEnd = visibleEnd;
      try {
        const rangeLines = await api.highlightRange(bufferId, vStart, vEnd);
        if (rangeLines && editorStore.getState('activeBufferId') === bufferId) {
          for (const line of rangeLines) lineCache.set(line.line_number, line);
          // Invalidate the recycled DOM so renderVisibleLines rebuilds nodes
          // with the freshly-arrived spans instead of reusing unhighlighted ones.
          prevRenderedLineNums = [];
          renderFromCache();
        }
      } catch { /* highlight_range not available — fall through to full parse */ }

      // Phase 2: Full background parse (populates cache for all lines)
      if (editorStore.getState('activeBufferId') !== bufferId) return;
      const highlighted = await api.highlightBuffer(bufferId);
      if (!highlighted) return;
      fullHighlightDone.add(bufferId);

      if (editorStore.getState('activeBufferId') !== bufferId) return;

      // Re-fetch visible lines from the now-complete cache
      const lines = await api.getVisibleLines(bufferId, visibleStart, visibleEnd);
      if (!lines || editorStore.getState('activeBufferId') !== bufferId) return;
      for (const line of lines) lineCache.set(line.line_number, line);
      prevRenderedLineNums = [];
      renderFromCache();

      // Highlighting is now available — reload minimap to pick up colors
      minimapCache.clear();
      const settings = settingsStore.getState('settings');
      if (settings?.editor?.minimap) startMinimapLoad(bufferId);
    } catch (e) {
      console.error(`[SyntaxHighlight] requestHighlighting failed for bufferId=${bufferId}:`, e);
    } finally {
      highlightingInFlight.delete(bufferId);
    }
  }

  /** Fetch lines missing from cache in the given range (used on scroll). */
  async function fetchMissingLines(bufferId, start, end) {
    // Check if any lines in range are missing
    let hasMissing = false;
    for (let i = start; i < end; i++) {
      if (!lineCache.has(i + 1)) { hasMissing = true; break; }
    }
    if (!hasMissing) return;

    try {
      const lines = await api.getVisibleLines(bufferId, start, end);
      if (!lines || editorStore.getState('activeBufferId') !== bufferId) return;
      for (const line of lines) lineCache.set(line.line_number, line);
      renderFromCache();
    } catch (e) {
      console.error('Failed to fetch lines:', e);
    }
  }

  function reloadAllLines() {
    lineCache.clear();
    fullHighlightDone.delete(currentBufferId);
    // After edits, fetch visible range (highlight cache was invalidated, so this
    // returns plain text instantly, then background highlighting kicks in again)
    loadVisibleLines(currentBufferId, visibleStart, visibleEnd);
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
    // NOTE: Minimap canvas is NOT repainted on scroll. Only the viewport
    // indicator moves (via updateMinimapViewport in the scroll handler).
    // Minimap canvas is repainted only when new data arrives (in
    // startMinimapLoad and requestHighlighting).
  }

  // ===================== CHAR WIDTH =====================
  let _charWidth = 0;
  function getCharWidth() {
    if (_charWidth > 0) return _charWidth;
    const span = document.createElement('span');
    span.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;' +
      `font-family:var(--font-family-mono);font-size:var(--font-size-editor);line-height:${LINE_HEIGHT}px;`;
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
  // Track previously rendered line numbers for DOM recycling
  let prevRenderedStart = -1;
  let prevRenderedLineNums = []; // line numbers currently in the DOM

  // Map from 1-indexed logical line number → its rendered .editor-line element.
  // Repopulated after every renderVisibleLines(). Used by lineGeometry / lineAtY
  // so word-wrap layout can be queried without reflow loops.
  const renderedLineEls = new Map();

  function isWordWrap() {
    return container.classList.contains('editor-pane--word-wrap');
  }

  /**
   * Geometry of a logical line in scroll-content coordinates — i.e. the
   * coordinate system where the legacy `line * LINE_HEIGHT` lives. When
   * word-wrap is OFF this is exactly that. When word-wrap is ON and the line
   * is currently rendered, we look up the real DOM element and return its
   * `offsetTop` (relative to linesContainer, plus linesContainer's own offset
   * within the scroll content) and `offsetHeight` (which spans all wrapped
   * visual rows). For non-rendered lines (off-screen) we fall back to
   * LINE_HEIGHT math — same approximation as before, so navigation /
   * scroll-to-line behave like they used to.
   */
  function lineGeometry(line) {
    if (!isWordWrap()) {
      return { top: line * LINE_HEIGHT, height: LINE_HEIGHT };
    }
    const el = renderedLineEls.get(line + 1);
    if (el) {
      return {
        top: prevRenderedStart * LINE_HEIGHT + el.offsetTop,
        height: el.offsetHeight,
      };
    }
    return { top: line * LINE_HEIGHT, height: LINE_HEIGHT };
  }

  /** Inverse of lineGeometry: which logical line contains scroll-content y? */
  function lineAtY(absY) {
    if (!isWordWrap()) {
      return Math.max(0, Math.min(lineCount - 1, Math.floor(absY / LINE_HEIGHT)));
    }
    for (const [lineNum, el] of renderedLineEls) {
      const top = prevRenderedStart * LINE_HEIGHT + el.offsetTop;
      if (absY >= top && absY < top + el.offsetHeight) {
        return lineNum - 1;
      }
    }
    return Math.max(0, Math.min(lineCount - 1, Math.floor(absY / LINE_HEIGHT)));
  }

  function renderVisibleLines() {
    if (renderedLines.length === 0) return;
    const startLine = renderedLines[0].line_number - 1;
    const newLineNums = renderedLines.map(l => l.line_number);

    // --- DOM RECYCLING for editor lines ---
    // Build a Set of line numbers already in the DOM for O(1) lookup.
    // On a typical 1-line scroll, ~98% of lines are reused.
    const prevSet = new Set(prevRenderedLineNums);
    const newSet = new Set(newLineNums);
    const existingChildren = linesContainer.children;

    // Check if we can do an incremental update (ranges overlap)
    const hasOverlap = prevRenderedLineNums.length > 0
      && existingChildren.length === prevRenderedLineNums.length
      && newLineNums.some(n => prevSet.has(n));

    if (hasOverlap) {
      // Build a map from line_number -> existing DOM node
      const nodeMap = new Map();
      for (let i = 0; i < prevRenderedLineNums.length; i++) {
        nodeMap.set(prevRenderedLineNums[i], existingChildren[i]);
      }
      // Build new children array, reusing existing nodes where possible
      const frag = document.createDocumentFragment();
      for (let i = 0; i < renderedLines.length; i++) {
        const ln = newLineNums[i];
        const existing = nodeMap.get(ln);
        if (existing) {
          frag.appendChild(existing); // Move existing node (no re-render)
        } else {
          frag.appendChild(renderLine(renderedLines[i])); // New line
        }
      }
      linesContainer.replaceChildren(frag);
    } else {
      // No overlap (big jump or first render) — full rebuild
      const frag = document.createDocumentFragment();
      for (const line of renderedLines) frag.appendChild(renderLine(line));
      linesContainer.replaceChildren(frag);
    }
    linesContainer.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    // --- GUTTER ---
    gutterContent.replaceChildren(renderGutter(renderedLines, cursorLine + 1));
    gutterContent.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    prevRenderedStart = startLine;
    prevRenderedLineNums = newLineNums;

    // Refresh the line-element index used by lineGeometry / lineAtY.
    // Cheap — just one Map clear + N pointer assignments.
    renderedLineEls.clear();
    for (let i = 0; i < newLineNums.length; i++) {
      renderedLineEls.set(newLineNums[i], linesContainer.children[i]);
    }

    // Word wrap: sync gutter line heights with actual content line heights
    if (container.classList.contains('editor-pane--word-wrap')) {
      syncGutterHeightsWithContent();
    }

    updateCursorPosition();
    renderSelection();
    renderMatchHighlights();
    renderGlobalSearchHighlights();
  }

  /** When word wrap is on, match each gutter line's height to its content line. */
  function syncGutterHeightsWithContent() {
    const editorLines = linesContainer.querySelectorAll('.editor-line');
    const gutterLines = gutterContent.querySelectorAll('.editor-gutter__line');
    for (let i = 0; i < editorLines.length && i < gutterLines.length; i++) {
      const h = editorLines[i].offsetHeight;
      if (h > LINE_HEIGHT) {
        gutterLines[i].style.height = h + 'px';
        gutterLines[i].style.lineHeight = LINE_HEIGHT + 'px'; // keep number at top
      } else {
        gutterLines[i].style.height = '';
        gutterLines[i].style.lineHeight = '';
      }
    }
  }

  function renderSelection() {
    selectionLayer.replaceChildren();
    if (!hasSelection()) return;

    const { startLine, startCol, endLine, endCol } = getSelectionRange();
    const charWidth = getCharWidth();
    const frag = document.createDocumentFragment();

    const lo = Math.max(startLine, visibleStart);
    const hi = Math.min(endLine, visibleEnd - 1);

    // Word-wrap mode: a single logical line can span many visual rows. The
    // legacy single-rectangle path (`top: geom.top; height: geom.height;
    // left: visualSCol*charWidth`) paints one giant rectangle covering every
    // wrapped row of the line, even when the user only selected a slice in
    // one row. The clipboard ends up correct (selection model is char-based)
    // but the highlight visualization is misleading.
    //
    // Fix: build a DOM Range over the rendered line element for the selected
    // char span and ask the browser for one rectangle per visual row via
    // `getClientRects()`. The browser already knows where it wrapped, so this
    // is exact regardless of font fallback, ligatures, or wrap algorithm.
    // Plain text (pasted scratch buffers) has one text node per line so the
    // char→DOM offset mapping is direct; for syntax-highlighted lines we
    // walk text nodes accumulating `nodeValue.length` which still matches
    // source chars unless decoration spans (whitespace markers, zero-width
    // labels) are active. In that rare case the rects may be visually off
    // by a column or two — better than the current full-line miss.
    const wrapMode = isWordWrap();
    if (wrapMode) {
      const scrollRect = scrollContainer.getBoundingClientRect();
      const scrollX = scrollContainer.scrollLeft;
      const scrollY = scrollContainer.scrollTop;

      for (let i = lo; i <= hi; i++) {
        const cached = lineCache.get(i + 1);
        if (!cached) continue;
        const lineEl = renderedLineEls.get(i + 1);
        if (!lineEl) continue; // off-screen — caller already clipped to visible
        const lineLen = cached.text.length;

        let sCol = 0, eCol = lineLen;
        if (i === startLine) sCol = Math.min(startCol, lineLen);
        if (i === endLine) eCol = Math.min(endCol, lineLen);
        // Clamp; an empty selection on the end line means nothing to draw.
        if (sCol > eCol) continue;
        const includesNewline = i < endLine; // selection extends past EOL

        const range = rangeForLineCharSpan(lineEl, sCol, eCol);
        let rects = range ? Array.from(range.getClientRects()) : [];

        if (rects.length === 0) {
          // Empty range (e.g. caret-only or all-empty line): synthesize one
          // zero-width rect at the line's start so the newline-selection
          // hint below still has something to extend.
          const lineRect = lineEl.getBoundingClientRect();
          rects = [{ top: lineRect.top, left: lineRect.left, width: 0, height: lineRect.height || LINE_HEIGHT }];
        }

        for (let r = 0; r < rects.length; r++) {
          const rect = rects[r];
          if (rect.height === 0) continue;
          const isLastRect = r === rects.length - 1;
          // Mirror the legacy `+1` newline hint: if the selection extends to
          // a following line, stretch the LAST visual row's rect a half-char
          // past its end so the user sees the newline included.
          const widthBoost = includesNewline && isLastRect ? charWidth * 0.5 : 0;
          const div = document.createElement('div');
          div.className = 'editor-selection';
          div.style.cssText =
            `top:${rect.top - scrollRect.top + scrollY}px;` +
            `left:${rect.left - scrollRect.left + scrollX}px;` +
            `width:${Math.max(charWidth * 0.5, rect.width + widthBoost)}px;` +
            `height:${rect.height}px;`;
          frag.appendChild(div);
        }
      }
      selectionLayer.replaceChildren(frag);
      return;
    }

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

      const geom = lineGeometry(i);
      const div = document.createElement('div');
      div.className = 'editor-selection';
      div.style.cssText =
        `top:${geom.top}px;left:${visualSCol * charWidth + LINES_PADDING_LEFT}px;` +
        `width:${Math.max(charWidth * 0.5, (visualECol - visualSCol) * charWidth)}px;height:${geom.height}px;`;
      frag.appendChild(div);
    }
    selectionLayer.replaceChildren(frag);
  }

  /**
   * Walk text nodes inside a rendered .editor-line element and produce a
   * DOM Range covering the source-char span [sCol, eCol]. Assumes each text
   * node's `nodeValue.length` equals its source-char count — true for plain
   * text and ordinary syntax-highlight spans, off only for decoration spans
   * (whitespace markers, zero-width-char labels) which pasted-text buffers
   * don't trigger anyway. Returns null when the line element has no text
   * nodes (theoretically shouldn't happen — renderLine inserts a single
   * space placeholder for empty lines).
   */
  function rangeForLineCharSpan(lineEl, sCol, eCol) {
    const walker = document.createTreeWalker(lineEl, NodeFilter.SHOW_TEXT);
    const textNodes = [];
    let n;
    while ((n = walker.nextNode())) textNodes.push(n);
    if (textNodes.length === 0) return null;

    const range = document.createRange();
    let acc = 0;
    let startSet = false, endSet = false;
    for (const node of textNodes) {
      const len = node.nodeValue.length;
      if (!startSet && sCol <= acc + len) {
        range.setStart(node, Math.max(0, Math.min(len, sCol - acc)));
        startSet = true;
      }
      if (!endSet && eCol <= acc + len) {
        range.setEnd(node, Math.max(0, Math.min(len, eCol - acc)));
        endSet = true;
      }
      if (startSet && endSet) break;
      acc += len;
    }
    // Anchor anything past the last text node to its end.
    const last = textNodes[textNodes.length - 1];
    if (!startSet) range.setStart(last, last.nodeValue.length);
    if (!endSet) range.setEnd(last, last.nodeValue.length);
    return range;
  }

  // Cache layout offsets to avoid forced reflows on every scroll/cursor update.
  // Invalidated on resize, buffer switch, and any container layout change.
  let _cachedOffsetX = null;
  let _cachedOffsetY = null;
  function invalidateLayoutCache() { _cachedOffsetX = null; _cachedOffsetY = null; _charWidth = 0; }
  window.addEventListener('resize', invalidateLayoutCache);

  function updateCursorPosition() {
    const charWidth = getCharWidth();
    const zoom = getZoom();
    // Use cached layout offsets; recompute only when invalidated
    if (_cachedOffsetX === null) {
      const containerRect = container.getBoundingClientRect();
      const scrollRect = scrollContainer.getBoundingClientRect();
      _cachedOffsetY = (scrollRect.top - containerRect.top) / zoom;
      _cachedOffsetX = (scrollRect.left - containerRect.left) / zoom;
    }
    const offsetY = _cachedOffsetY;
    const offsetX = _cachedOffsetX;

    // Convert character column to visual column for display positioning
    const cached = lineCache.get(cursorLine + 1);
    const visualCol = cached ? charColToVisualCol(cached.text, cursorCol) : cursorCol;

    const cursorGeom = lineGeometry(cursorLine);
    cursor.style.top = `${offsetY + cursorGeom.top - scrollContainer.scrollTop}px`;
    cursor.style.left = `${offsetX + visualCol * charWidth + LINES_PADDING_LEFT}px`;
    cursor.style.height = `${LINE_HEIGHT}px`;
    cursor.style.display = currentBufferId ? 'block' : 'none';
  }

  // ===================== CLICK POSITION HELPER =====================
  /**
   * Resolve a logical line number for a pointer event by asking the DOM
   * which element is under the pointer. Each `.editor-line` carries a
   * `data-line` attribute, so this bypasses every form of geometry math —
   * works correctly with word-wrap, mid-render races, zoom, scroll deltas,
   * and any future layout change. Returns null when the pointer didn't land
   * on an editor line (e.g. clicked the empty area below the last line),
   * so the caller can fall back to geometry-based lineAtY().
   */
  function lineFromPointer(e) {
    // Walking up from event.target works when overlay layers (selection,
    // highlights) have pointer-events: none. elementFromPoint is the
    // belt-and-suspenders fallback — it ignores pointer-events:none layers
    // by design and returns whatever element actually receives the hit.
    let target = e.target;
    let lineEl = target && target.closest ? target.closest('.editor-line') : null;
    if (!lineEl) {
      const hit = document.elementFromPoint(e.clientX, e.clientY);
      lineEl = hit && hit.closest ? hit.closest('.editor-line') : null;
    }
    if (!lineEl) return null;
    const n = parseInt(lineEl.dataset.line, 10);
    if (Number.isNaN(n) || n < 1) return null;
    return n - 1; // data-line is 1-indexed; everything else is 0-indexed
  }

  function lineElFromPointer(e) {
    let target = e.target;
    let lineEl = target && target.closest ? target.closest('.editor-line') : null;
    if (!lineEl) {
      const hit = document.elementFromPoint(e.clientX, e.clientY);
      lineEl = hit && hit.closest ? hit.closest('.editor-line') : null;
    }
    if (!lineEl) return null;
    const n = parseInt(lineEl.dataset.line, 10);
    if (Number.isNaN(n) || n < 1) return null;
    return lineEl;
  }

  function getLineTextFromEl(lineEl) {
    const raw = lineEl.textContent || '';
    return raw.replace(/\u2192/g, '').replace(/\u00B7/g, ' ');
  }

  function getClickPos(e) {
    const zoom = getZoom();
    const rect = scrollContainer.getBoundingClientRect();
    // clientX/clientY and getBoundingClientRect() are in viewport-space (zoomed),
    // but scrollTop/scrollLeft and LINE_HEIGHT/charWidth are in CSS-space (unzoomed).
    // Divide the viewport-space offset by zoom to convert to CSS-space.
    const relY = (e.clientY - rect.top) / zoom + scrollContainer.scrollTop;
    const relX = (e.clientX - rect.left) / zoom - LINES_PADDING_LEFT + scrollContainer.scrollLeft;
    const charWidth = getCharWidth();
    // DOM-based hit-test first (always correct); geometry as fallback for
    // clicks below the last line.
    const fromDom = lineFromPointer(e);
    const line = fromDom !== null ? fromDom : lineAtY(relY);

    // Word-wrap path: a logical line spans multiple visual rows, so the
    // X-based visualColToCharCol math collapses every row onto row 0 and
    // the user can't drag past the first row. Ask the browser for the
    // exact (textNode, offset) under the pointer instead — its layout
    // engine already knows where the line wrapped — then walk the line's
    // text nodes to convert that to a source char column.
    if (isWordWrap()) {
      const lineEl = lineElFromPointer(e);
      if (lineEl) {
        const col = caretCharColAtPoint(lineEl, e.clientX, e.clientY);
        if (col !== null) {
          const cachedLen = (lineCache.get(line + 1)?.text?.length) ?? col;
          return { line, col: Math.min(col, cachedLen) };
        }
      }
      // Pointer is outside any rendered .editor-line element (most often
      // the empty area below the last line during a drag). The legacy
      // X-based fallback would compute a tiny visualX, map it to a low
      // column, and visually snap the selection head to the TOP of the
      // wrapped block — exactly the bug we're fixing here. Instead, find
      // the nearest line by Y-distance, clamp clientY into that line's
      // vertical span, and re-query caret-from-point. End-of-line for
      // below-the-text drags, start-of-line for above-the-text drags.
      const clamped = clampPointToNearestLine(e.clientX, e.clientY);
      if (clamped) {
        const col = caretCharColAtPoint(clamped.lineEl, clamped.x, clamped.y);
        if (col !== null) {
          const cachedLen = (lineCache.get(clamped.lineNum)?.text?.length) ?? col;
          return { line: clamped.lineNum - 1, col: Math.min(col, cachedLen) };
        }
        // caret-from-point still missed (rare; e.g. line has no text node
        // at the clamped X). Fall back to end-of-line for below clamps and
        // start-of-line otherwise — guaranteed sensible.
        const cached = lineCache.get(clamped.lineNum);
        const lineLen = cached?.text?.length ?? 0;
        return { line: clamped.lineNum - 1, col: clamped.below ? lineLen : 0 };
      }
      // Fallthrough to legacy X-based math below.
    }

    // Convert pixel offset to a fractional visual column, then map to
    // the actual character column — accounts for tab characters.
    const visualX = Math.max(0, relX / charWidth);
    const cached = lineCache.get(line + 1);
    let col;
    if (cached) {
      col = visualColToCharCol(cached.text, visualX);
      col = Math.min(col, cached.text.length);
    } else {
      const domEl = lineElFromPointer(e);
      const lineText = domEl ? getLineTextFromEl(domEl) : '';
      col = visualColToCharCol(lineText, visualX);
      col = Math.min(col, lineText.length);
    }
    return { line, col };
  }

  /**
   * Map a viewport-space point inside a rendered .editor-line element to a
   * source-char column within that logical line. Uses the browser's caret-
   * from-point API (Chromium: caretRangeFromPoint, Firefox: caretPositionFromPoint)
   * to get the exact text node + intra-node offset, then walks all text
   * nodes inside `lineEl` accumulating `nodeValue.length` to convert the
   * (node, offset) pair into a flat character column. Returns null when
   * the API isn't available or the hit landed outside any text node.
   *
   * Same `nodeValue.length === source-char count` assumption as
   * `rangeForLineCharSpan` — exact for plain text and ordinary syntax-
   * highlight spans, off-by-a-few-chars only on lines containing decoration
   * spans (whitespace markers / zero-width labels), which pasted-text
   * buffers don't have.
   */
  function caretCharColAtPoint(lineEl, clientX, clientY) {
    let hitNode = null;
    let hitOffset = 0;
    if (typeof document.caretRangeFromPoint === 'function') {
      const r = document.caretRangeFromPoint(clientX, clientY);
      if (r) { hitNode = r.startContainer; hitOffset = r.startOffset; }
    } else if (typeof document.caretPositionFromPoint === 'function') {
      const p = document.caretPositionFromPoint(clientX, clientY);
      if (p) { hitNode = p.offsetNode; hitOffset = p.offset; }
    }
    if (!hitNode) return null;
    // The hit may be on the line element itself (between text nodes) or on
    // a descendant text node. Bail if it's outside our line.
    if (!lineEl.contains(hitNode) && hitNode !== lineEl) return null;

    const walker = document.createTreeWalker(lineEl, NodeFilter.SHOW_TEXT);
    const textNodes = [];
    let n;
    while ((n = walker.nextNode())) textNodes.push(n);
    if (textNodes.length === 0) return null;

    // Caret landed directly on the line container — interpret as
    // start-or-end-of-line based on which child index hitOffset points to.
    if (hitNode === lineEl) {
      if (hitOffset <= 0) return 0;
      let total = 0;
      for (const t of textNodes) total += t.nodeValue.length;
      return total;
    }

    let acc = 0;
    for (const node of textNodes) {
      if (node === hitNode) return acc + Math.min(hitOffset, node.nodeValue.length);
      acc += node.nodeValue.length;
    }
    // Hit on a non-text descendant (e.g. a decoration span). Walk back up
    // until we find a sibling text node and anchor to its boundary.
    let probe = hitNode;
    while (probe && probe !== lineEl) {
      let prev = probe.previousSibling;
      while (prev) {
        if (prev.nodeType === Node.TEXT_NODE) {
          // Sum lengths up through this text node.
          let sum = 0;
          for (const t of textNodes) {
            sum += t.nodeValue.length;
            if (t === prev) return sum;
          }
        }
        prev = prev.previousSibling;
      }
      probe = probe.parentNode;
    }
    return acc;
  }

  /**
   * Find the rendered .editor-line element nearest to (clientX, clientY) and
   * return a viewport-space point clamped into its bounding rect — plus a
   * `below` flag indicating whether the original point was below the line
   * (used as a hint to anchor end-of-line vs start-of-line when the caret
   * API still can't resolve a position). Used during drag selections when
   * the cursor leaves the line area; without clamping, the caret-from-point
   * fallback to legacy X-based math snaps the selection to the top of the
   * wrapped block.
   */
  function clampPointToNearestLine(clientX, clientY) {
    if (renderedLineEls.size === 0) return null;
    let bestLineNum = null, bestEl = null, bestDist = Infinity, bestRect = null;
    for (const [lineNum, el] of renderedLineEls) {
      const r = el.getBoundingClientRect();
      // Vertical distance to this line's bounding rect (0 if y is inside).
      const dy = clientY < r.top ? r.top - clientY
        : clientY > r.bottom ? clientY - r.bottom
        : 0;
      if (dy < bestDist) {
        bestDist = dy;
        bestLineNum = lineNum;
        bestEl = el;
        bestRect = r;
        if (dy === 0) break; // can't beat zero
      }
    }
    if (!bestEl) return null;
    // Inset the clamp by 1px so the resulting (x, y) is *inside* the rect,
    // which caretRangeFromPoint requires for a hit. A point exactly on the
    // edge sometimes returns null in Chromium.
    const y = Math.max(bestRect.top + 1, Math.min(bestRect.bottom - 1, clientY));
    const x = Math.max(bestRect.left + 1, Math.min(bestRect.right - 1, clientX));
    return {
      lineEl: bestEl,
      lineNum: bestLineNum,
      x, y,
      below: clientY > bestRect.bottom,
    };
  }

  // ===================== SCROLL =====================
  function computeVisibleRange(scrollTop) {
    const vh = scrollContainer.clientHeight;
    return {
      newStart: Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN),
      newEnd: Math.min(lineCount, Math.ceil((scrollTop + vh) / LINE_HEIGHT) + OVERSCAN),
    };
  }

  // RAF-throttled scroll handler — coalesces rapid scroll events into
  // a single render per frame, preventing layout thrashing on large files.
  let scrollRafId = 0;
  scrollContainer.addEventListener('scroll', () => {
    gutterEl.scrollTop = scrollContainer.scrollTop;
    updateCursorPosition();
    if (!currentBufferId) return;
    if (scrollRafId) return; // already scheduled
    scrollRafId = requestAnimationFrame(() => {
      scrollRafId = 0;
      editorStore.setState({ scrollTop: scrollContainer.scrollTop });
      const { newStart, newEnd } = computeVisibleRange(scrollContainer.scrollTop);
      if (newStart !== visibleStart || newEnd !== visibleEnd) {
        visibleStart = newStart;
        visibleEnd = newEnd;
        renderFromCache();
        // Fetch any lines not yet in cache for the new visible range
        fetchMissingLines(currentBufferId, visibleStart, visibleEnd);
      }
      updateMinimapViewport();
    });
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
    const hoverFromDom = lineFromPointer(e);
    const hoverLine = hoverFromDom !== null ? hoverFromDom : lineAtY(relY);
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
    const contentH = lineCount * LINE_HEIGHT;
    spacer.style.height = `${contentH}px`;
    gutterSpacer.style.height = `${contentH}px`;
    updateGutterWidth();
    // Update the bottom scroll padding via CSS custom property.
    // This allows scrolling the last line to the top of the viewport.
    // Reading clientHeight inside rAF ensures layout is current.
    requestAnimationFrame(() => {
      const viewportH = scrollContainer.clientHeight;
      if (viewportH > 0) {
        const pad = viewportH - LINE_HEIGHT;
        // Set on the editor pane container so both gutter and code area inherit it
        container.style.setProperty('--scroll-pad-bottom', Math.max(0, pad) + 'px');
      }
    });
  }

  // ===================== AUTO-CLOSING BRACKETS =====================
  const BRACKET_PAIRS = { '(': ')', '{': '}', '[': ']' };
  const QUOTE_CHARS = ['"', "'", '`'];
  const CLOSE_BRACKETS = new Set([')', '}', ']']);

  /** Get the character at the current cursor position (the char right after the cursor). */
  function charAfterCursor() {
    const cached = lineCache.get(cursorLine + 1); // 1-based
    if (!cached) return '';
    return cached.text[cursorCol] || '';
  }

  /** Get the character right before the current cursor position. */
  function charBeforeCursor() {
    const cached = lineCache.get(cursorLine + 1);
    if (!cached || cursorCol === 0) return '';
    return cached.text[cursorCol - 1] || '';
  }

  /** Check if a character is a word character (letter, digit, underscore). */
  function isWordChar(ch) {
    return /\w/.test(ch);
  }

  // ===================== INPUT HANDLING =====================
  textarea.addEventListener('compositionstart', () => { isComposing = true; });
  textarea.addEventListener('compositionend', () => { isComposing = false; handleInput(); });
  textarea.addEventListener('input', () => { if (!isComposing) handleInput(); });

  async function handleInput() {
    const text = textarea.value;
    if (!text || !currentBufferId) return;
    textarea.value = '';

    // Auto-close brackets and quotes (only for single-char input, not paste)
    if (text.length === 1) {
      const after = charAfterCursor();

      // Wrap selection with bracket/quote pairs
      if (hasSelection() && (BRACKET_PAIRS[text] || QUOTE_CHARS.includes(text))) {
        const sel = getSelectedText();
        const closer = BRACKET_PAIRS[text] || text;
        await editAtCursor(text + sel + closer);
        // Place cursor after the closing char
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
        return;
      }

      // Opening bracket → insert pair, place cursor between them
      if (BRACKET_PAIRS[text]) {
        const closer = BRACKET_PAIRS[text];
        await editAtCursor(text + closer);
        // Move cursor back one (between the pair)
        cursorCol--;
        editorStore.setState({ cursorLine, cursorCol });
        updateCursorPosition();
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
        return;
      }

      // Closing bracket → overtype if next char is the same closer
      if (CLOSE_BRACKETS.has(text) && after === text) {
        // Just move cursor forward, don't insert
        cursorCol++;
        editorStore.setState({ cursorLine, cursorCol });
        updateCursorPosition();
        return;
      }

      // Quote char → auto-close or overtype
      if (QUOTE_CHARS.includes(text)) {
        // If next char is the same quote, overtype it
        if (after === text) {
          cursorCol++;
          editorStore.setState({ cursorLine, cursorCol });
          updateCursorPosition();
          return;
        }
        // If previous char is a word char, don't auto-close (likely an apostrophe or mid-word)
        const before = charBeforeCursor();
        if (!isWordChar(before) && !isWordChar(after)) {
          await editAtCursor(text + text);
          cursorCol--;
          editorStore.setState({ cursorLine, cursorCol });
          updateCursorPosition();
          docVersion++;
          api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
          return;
        }
      }
    }

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
      const acGeom = lineGeometry(cursorLine);
      autocomplete.show(currentBufferId, cursorLine, cursorCol,
        oX + acVisualCol * cw + LINES_PADDING_LEFT,
        oY + acGeom.top + acGeom.height - scrollContainer.scrollTop);
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

    // --- Enter (with smart auto-indent) ---
    if (e.key === 'Enter') {
      e.preventDefault();
      textarea.value = '';

      // Get current line text and compute indentation
      const cached = lineCache.get(cursorLine + 1);
      const lineText = cached ? cached.text : '';
      const textBeforeCursor = lineText.substring(0, cursorCol);
      const textAfterCursor = lineText.substring(cursorCol);

      // Carry forward leading whitespace from current line
      const indentMatch = lineText.match(/^(\s*)/);
      const baseIndent = indentMatch ? indentMatch[1] : '';

      // Check if we should add extra indent (line before cursor ends with opener)
      const trimmedBefore = textBeforeCursor.trimEnd();
      const lastChar = trimmedBefore[trimmedBefore.length - 1];
      const needsExtraIndent = lastChar === '{' || lastChar === '(' || lastChar === '[' || lastChar === ':';

      // Check if cursor is between matching bracket pair like {|}
      const trimmedAfter = textAfterCursor.trimStart();
      const nextChar = trimmedAfter[0];
      const isBetweenPair = needsExtraIndent && (
        (lastChar === '{' && nextChar === '}') ||
        (lastChar === '(' && nextChar === ')') ||
        (lastChar === '[' && nextChar === ']')
      );

      const indent = '    '; // 4-space indent
      let insertText;

      if (isBetweenPair) {
        // Between brackets: add indented line AND closing line
        // {|} → {\n    |\n}
        insertText = '\n' + baseIndent + indent + '\n' + baseIndent;
      } else if (needsExtraIndent) {
        insertText = '\n' + baseIndent + indent;
      } else {
        insertText = '\n' + baseIndent;
      }

      await editAtCursor(insertText);

      // If between pair, cursor should be on the middle line (one line up from where editAtCursor left it)
      if (isBetweenPair) {
        cursorLine--;
        cursorCol = baseIndent.length + indent.length;
        editorStore.setState({ cursorLine, cursorCol });
      }

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
          // Auto-delete matching closer when backspacing an empty pair like (), {}, [], "", '', ``
          const before = charBeforeCursor();
          const after = charAfterCursor();
          const isEmptyPair = (BRACKET_PAIRS[before] === after) ||
            (QUOTE_CHARS.includes(before) && before === after);
          const deleteSize = isEmptyPair ? 2 : 1;
          const r = await api.editBuffer(currentBufferId, cursorLine, cursorCol - 1, '', deleteSize);
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
    const { top: cursorY, height: cursorH } = lineGeometry(cursorLine);
    const viewTop = scrollContainer.scrollTop;
    const viewBottom = viewTop + scrollContainer.clientHeight;
    if (cursorY < viewTop) scrollContainer.scrollTop = cursorY;
    else if (cursorY + cursorH > viewBottom) scrollContainer.scrollTop = cursorY - scrollContainer.clientHeight + cursorH;
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

  async function doFindSearch() {
    if (!findReplace.isVisible() || !findReplace.getQuery()) {
      findMatches = [];
      currentMatchIdx = -1;
      renderMatchHighlights();
      findReplace.setMatchInfo(0, 0);
      return;
    }
    // Ensure all lines are in cache for a complete search
    if (currentBufferId && lineCache.size < lineCount) {
      try {
        const allLines = await api.getVisibleLines(currentBufferId, 0, lineCount);
        if (allLines && editorStore.getState('activeBufferId') === currentBufferId) {
          for (const line of allLines) lineCache.set(line.line_number, line);
        }
      } catch (e) { /* search with whatever we have */ }
    }
    findMatches = searchInBuffer(findReplace.getQuery(), lastSearchOpts);
    currentMatchIdx = findMatches.length > 0 ? findClosestMatch() : -1;
    findReplace.setMatchInfo(currentMatchIdx >= 0 ? currentMatchIdx + 1 : 0, findMatches.length);
    renderMatchHighlights();
    if (currentMatchIdx >= 0) scrollToMatch(currentMatchIdx);
  }

  /** Binary search to find first match index at or after the given line. */
  function findFirstMatchAtLine(matches, line) {
    let lo = 0, hi = matches.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (matches[mid].line < line) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  function renderMatchHighlights() {
    matchHighlightLayer.replaceChildren();
    if (findMatches.length === 0) return;
    const charWidth = getCharWidth();
    const frag = document.createDocumentFragment();
    // Binary search to jump directly to visible matches instead of scanning all
    const startIdx = findFirstMatchAtLine(findMatches, visibleStart);
    for (let i = startIdx; i < findMatches.length; i++) {
      const m = findMatches[i];
      if (m.line >= visibleEnd) break; // past visible range — stop
      const cached = lineCache.get(m.line + 1);
      if (!cached) continue;
      const vStart = charColToVisualCol(cached.text, m.startCol);
      const vEnd = charColToVisualCol(cached.text, m.endCol);
      const mGeom = lineGeometry(m.line);
      const div = document.createElement('div');
      div.className = i === currentMatchIdx ? 'editor-match-highlight--current' : 'editor-match-highlight';
      div.style.cssText = `top:${mGeom.top}px;left:${vStart * charWidth + LINES_PADDING_LEFT}px;width:${(vEnd - vStart) * charWidth}px;height:${mGeom.height}px;`;
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
    const { top: matchY, height: matchH } = lineGeometry(m.line);
    const viewTop = scrollContainer.scrollTop;
    const viewBottom = viewTop + scrollContainer.clientHeight;
    if (matchY < viewTop || matchY + matchH > viewBottom) {
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
    // Restore global search highlights if sidebar search is active
    updateGlobalSearchHighlights();
    textarea.focus();
  }

  // Wire up find/replace callbacks
  findReplace.onSearch(async (query, opts) => {
    lastSearchOpts = opts;
    // Ensure all lines are in cache for a complete search
    if (currentBufferId && lineCache.size < lineCount) {
      try {
        const allLines = await api.getVisibleLines(currentBufferId, 0, lineCount);
        if (allLines && editorStore.getState('activeBufferId') === currentBufferId) {
          for (const line of allLines) lineCache.set(line.line_number, line);
        }
      } catch (e) { /* search with whatever we have */ }
    }
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

  // ===================== GLOBAL SEARCH HIGHLIGHTS =====================
  // Highlights matches from the sidebar search panel (separate from find/replace)
  let globalSearchMatches = [];

  function renderGlobalSearchHighlights() {
    // Don't show global search highlights when find/replace widget is open
    if (findReplace.isVisible() || globalSearchMatches.length === 0) return;
    const charWidth = getCharWidth();
    const frag = document.createDocumentFragment();
    // Binary search to jump directly to visible matches
    const startIdx = findFirstMatchAtLine(globalSearchMatches, visibleStart);
    for (let i = startIdx; i < globalSearchMatches.length; i++) {
      const m = globalSearchMatches[i];
      if (m.line >= visibleEnd) break;
      const cached = lineCache.get(m.line + 1);
      if (!cached) continue;
      const vStart = charColToVisualCol(cached.text, m.startCol);
      const vEnd = charColToVisualCol(cached.text, m.endCol);
      const gGeom = lineGeometry(m.line);
      const div = document.createElement('div');
      div.className = 'editor-search-highlight';
      div.style.cssText = `top:${gGeom.top}px;left:${vStart * charWidth + LINES_PADDING_LEFT}px;width:${(vEnd - vStart) * charWidth}px;height:${gGeom.height}px;`;
      frag.appendChild(div);
    }
    matchHighlightLayer.appendChild(frag);
  }

  async function updateGlobalSearchHighlights() {
    const query = searchStore.getState('query');
    if (!query || !query.trim() || !currentBufferId || findReplace.isVisible()) {
      globalSearchMatches = [];
      // Re-render to clear stale highlights (renderMatchHighlights replaces children)
      if (!findReplace.isVisible()) renderMatchHighlights();
      return;
    }
    // Ensure all lines are cached for a thorough search
    if (lineCache.size < lineCount) {
      try {
        const allLines = await api.getVisibleLines(currentBufferId, 0, lineCount);
        if (allLines && editorStore.getState('activeBufferId') === currentBufferId) {
          for (const line of allLines) lineCache.set(line.line_number, line);
        }
      } catch { /* search with what we have */ }
    }
    const opts = {
      caseSensitive: searchStore.getState('caseSensitive'),
      wholeWord: searchStore.getState('wholeWord'),
      useRegex: searchStore.getState('isRegex'),
    };
    globalSearchMatches = searchInBuffer(query, opts);
    // Only render if find/replace is still closed
    if (!findReplace.isVisible()) {
      renderMatchHighlights();
      renderGlobalSearchHighlights();
    }
  }

  // Subscribe to sidebar search query changes
  searchStore.subscribe('query', () => updateGlobalSearchHighlights());
  searchStore.subscribe('caseSensitive', () => updateGlobalSearchHighlights());
  searchStore.subscribe('wholeWord', () => updateGlobalSearchHighlights());
  searchStore.subscribe('isRegex', () => updateGlobalSearchHighlights());

  // ===================== PENDING GOTO (from search result click) =====================
  editorStore.subscribe('pendingGoto', (goto) => {
    if (!goto || !currentBufferId) return;
    const { line, col } = goto;
    editorStore.setState({ pendingGoto: null });

    // Wait a tick for lines to load after buffer switch
    requestAnimationFrame(async () => {
      // Ensure the target line is cached
      if (!lineCache.has(line + 1)) {
        const rangeStart = Math.max(0, line - OVERSCAN);
        const rangeEnd = Math.min(lineCount, line + OVERSCAN);
        try {
          const lines = await api.getVisibleLines(currentBufferId, rangeStart, rangeEnd);
          if (lines) {
            for (const l of lines) lineCache.set(l.line_number, l);
          }
        } catch { /* best effort */ }
      }

      // Scroll to line (center it)
      cursorLine = Math.min(line, lineCount - 1);
      cursorCol = col;
      editorStore.setState({ cursorLine, cursorCol });
      const { top: targetY } = lineGeometry(cursorLine);
      scrollContainer.scrollTop = Math.max(0, targetY - scrollContainer.clientHeight / 2);

      // Update visible range and re-render
      const { newStart, newEnd } = computeVisibleRange(scrollContainer.scrollTop);
      visibleStart = newStart;
      visibleEnd = newEnd;
      renderFromCache();
      fetchMissingLines(currentBufferId, visibleStart, visibleEnd);
      updateCursorPosition();

      // Refresh global search highlights for the new buffer
      updateGlobalSearchHighlights();
    });
  });

  // ===================== BUFFER LIFECYCLE =====================
  function onActiveBufferChange(bufferId) {
    // Save current buffer's cache before switching
    if (currentBufferId !== null) {
      bufferCaches.set(currentBufferId, lineCache);
    }

    currentBufferId = bufferId;
    invalidateLayoutCache();
    prevRenderedStart = -1; // force full DOM rebuild on buffer switch
    clearSelection();
    findMatches = [];
    currentMatchIdx = -1;

    if (!bufferId) {
      lineCache = new Map();
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

    // Restore cached lines for this buffer (or start fresh)
    lineCache = bufferCaches.get(bufferId) || new Map();
    bufferCaches.set(bufferId, lineCache);

    // Invalidate the DOM-recycling tracker. Without this, switching from
    // d.rs back to c.rs would see overlapping line numbers (1..30 in both
    // files) and reuse d.rs's DOM nodes — leaving the editor stuck on the
    // previous file's text even though lineCache now holds c.rs's data.
    prevRenderedLineNums = [];

    updateSpacerHeights();
    scrollContainer.scrollTop = scrollTop;
    gutterEl.scrollTop = scrollTop;

    const vh = scrollContainer.clientHeight || 600;
    visibleStart = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
    visibleEnd = Math.min(lineCount, Math.ceil((scrollTop + vh) / LINE_HEIGHT) + OVERSCAN);

    // Render immediately from cache if we have data (instant tab switch)
    if (lineCache.size > 0) {
      renderFromCache();
      updateCursorPosition();
    }

    // Fetch only visible lines — rest will be fetched on scroll
    loadVisibleLines(bufferId, visibleStart, visibleEnd);
    textarea.focus();
    api.lspNotifyOpen(bufferId).catch(() => {});
    docVersion = 1;

    // Start minimap background load for the new buffer
    minimapCache.clear();
    const settings = settingsStore.getState('settings');
    // Re-apply settings now that `currentBufferId` changed — the
    // `isPastedTextBuffer` override depends on it, so a switch into / out of
    // a pasted-text buffer must flip word-wrap and minimap accordingly.
    applyEditorSettings(settings);
    if (settings?.editor?.minimap && !isPastedTextBuffer(bufferId)) {
      startMinimapLoad(bufferId);
    }
  }

  // Clean up caches when buffers are closed
  editorStore.subscribe('openBuffers', (buffers) => {
    for (const id of bufferCaches.keys()) {
      if (!buffers[id]) bufferCaches.delete(id);
    }
  });

  // React to format-on-save: reload content + re-highlight
  editorStore.subscribe('_formatEvent', (evt) => {
    if (!evt || evt.bufferId !== currentBufferId) return;
    console.log(`[Formatter] reloading buffer ${evt.bufferId} after format, lineCount: ${evt.lineCount}`);
    lineCount = evt.lineCount;
    // Clear all caches so stale content is gone
    lineCache.clear();
    bufferCaches.delete(evt.bufferId);
    minimapCache.clear();
    highlightingInFlight.delete(evt.bufferId);
    fullHighlightDone.delete(evt.bufferId);
    // Reload visible lines from the newly formatted rope
    loadVisibleLines(evt.bufferId, visibleStart, visibleEnd);
  });

  // Subscribe to group changes — trigger buffer switch when this group's active buffer changes
  let lastGroupActiveBufferId = null;
  function onGroupsChange() {
    const groups = editorStore.getState('groups');
    const group = groups.find(g => g.id === paneGroupId);
    const groupActiveBufferId = group ? group.activeBufferId : null;
    if (groupActiveBufferId !== lastGroupActiveBufferId) {
      lastGroupActiveBufferId = groupActiveBufferId;
      onActiveBufferChange(groupActiveBufferId);
    }
  }
  editorStore.subscribe('groups', onGroupsChange);
  // Also listen to activeBufferId for backward compat (single-group mode)
  editorStore.subscribe('activeBufferId', () => {
    if (editorStore.getState('activeGroupId') === paneGroupId) {
      onGroupsChange();
    }
  });

  // ===================== MINIMAP =====================
  const MINIMAP_LINE_HEIGHT = 4;   // px per line — taller = more readable
  const MINIMAP_CHAR_WIDTH = 1.8;  // px per character
  const MINIMAP_LINE_GAP = 1;      // gap between lines
  const MINIMAP_WIDTH = 86;        // total canvas width

  // Separate cache for minimap data so it doesn't depend on scroll position
  const minimapCache = new Map(); // line_number (1-based) -> line data
  let minimapLoadGeneration = 0;  // cancel stale loads on buffer switch
  let minimapRepaintScheduled = false;

  function scheduleMinimapRepaint() {
    if (minimapRepaintScheduled) return;
    minimapRepaintScheduled = true;
    requestAnimationFrame(() => {
      minimapRepaintScheduled = false;
      paintMinimap();
    });
  }

  /**
   * Progressive background loader: fetches lines for the minimap starting
   * from the VISIBLE AREA first, then expanding outward in both directions.
   * This ensures the area the user is looking at renders first, while the
   * rest fills in progressively. Uses setTimeout(0) between batches so the
   * main thread stays responsive.
   */
  function startMinimapLoad(bufferId) {
    if (!bufferId || lineCount === 0) return;
    const gen = ++minimapLoadGeneration;
    // Adaptive chunk size: larger files use bigger chunks to reduce API calls
    const CHUNK = lineCount > 50000 ? 2000 : lineCount > 10000 ? 1000 : 500;

    // Build load order: visible area first, then expand outward
    function buildLoadOrder() {
      const chunks = [];
      // Start from the current viewport center
      const vpCenter = Math.floor((visibleStart + visibleEnd) / 2);
      const vpChunkStart = Math.max(0, Math.floor(vpCenter / CHUNK) * CHUNK);

      // Add viewport chunk first
      const added = new Set();
      function addChunk(start) {
        if (start < 0 || start >= lineCount) return;
        const key = start;
        if (added.has(key)) return;
        added.add(key);
        chunks.push(start);
      }

      addChunk(vpChunkStart);

      // Expand outward from viewport in both directions
      for (let offset = CHUNK; offset < lineCount; offset += CHUNK) {
        addChunk(vpChunkStart - offset);
        addChunk(vpChunkStart + offset);
      }

      return chunks;
    }

    const loadOrder = buildLoadOrder();
    let chunkIdx = 0;

    async function loadNextChunk() {
      if (gen !== minimapLoadGeneration) return;
      if (chunkIdx >= loadOrder.length) return;
      if (editorStore.getState('activeBufferId') !== bufferId) return;

      const start = loadOrder[chunkIdx++];
      const end = Math.min(start + CHUNK, lineCount);

      // Skip chunks already fully cached
      let allCached = true;
      for (let i = start; i < end; i++) {
        if (!minimapCache.has(i + 1)) { allCached = false; break; }
      }

      if (!allCached) {
        try {
          const lines = await api.getVisibleLines(bufferId, start, end);
          if (gen !== minimapLoadGeneration) return;
          if (lines) {
            for (const line of lines) minimapCache.set(line.line_number, line);
          }
          scheduleMinimapRepaint();
        } catch { /* non-fatal */ }
      }

      // Schedule next chunk — setTimeout(0) yields to the event loop
      setTimeout(loadNextChunk, 0);
    }

    loadNextChunk();
  }

  /** Get token colors from CSS custom properties (cached per repaint). */
  let _tokenColorsCache = null;
  function getTokenColors() {
    if (_tokenColorsCache) return _tokenColorsCache;
    const s = getComputedStyle(document.documentElement);
    _tokenColorsCache = {
      keyword: s.getPropertyValue('--token-keyword').trim() || '#fb4934',
      string: s.getPropertyValue('--token-string').trim() || '#b8bb26',
      comment: s.getPropertyValue('--token-comment').trim() || '#928374',
      function: s.getPropertyValue('--token-function').trim() || '#b8bb26',
      type: s.getPropertyValue('--token-type').trim() || '#fabd2f',
      number: s.getPropertyValue('--token-number').trim() || '#d3869b',
      variable: s.getPropertyValue('--token-variable').trim() || '#83a598',
      operator: s.getPropertyValue('--token-operator').trim() || '#8ec07c',
      punctuation: s.getPropertyValue('--token-punctuation').trim() || '#a89984',
      default: s.getPropertyValue('--fg4').trim() || '#a89984',
    };
    // Invalidate after a short delay so theme changes are picked up
    setTimeout(() => { _tokenColorsCache = null; }, 2000);
    return _tokenColorsCache;
  }

  function paintMinimap() {
    const settings = settingsStore.getState('settings');
    if (!settings?.editor?.minimap || !currentBufferId) return;

    const containerHeight = codeWrapper.clientHeight || 400;
    const rowHeight = MINIMAP_LINE_HEIGHT + MINIMAP_LINE_GAP;
    const canvasHeight = Math.min(lineCount * rowHeight, containerHeight);
    const dpr = window.devicePixelRatio || 1;

    minimapCanvas.width = MINIMAP_WIDTH * dpr;
    minimapCanvas.height = canvasHeight * dpr;
    minimapCanvas.style.width = MINIMAP_WIDTH + 'px';
    minimapCanvas.style.height = canvasHeight + 'px';

    const ctx = minimapCanvas.getContext('2d');
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, MINIMAP_WIDTH, canvasHeight);

    const tokenColors = getTokenColors();

    // Scale factor if file is taller than container
    const scale = lineCount * rowHeight > containerHeight
      ? containerHeight / (lineCount * rowHeight)
      : 1;

    const maxLines = Math.min(lineCount, Math.ceil(canvasHeight / (rowHeight * scale)));
    const lineH = MINIMAP_LINE_HEIGHT * scale;

    // For very large files (lineH < 1.5px), use fast block rendering instead
    // of per-character rendering to keep performance smooth
    const useBlockMode = lineH < 1.5;

    for (let i = 0; i < maxLines; i++) {
      // Use minimap's own cache; fall back to editor line cache
      const cached = minimapCache.get(i + 1) || lineCache.get(i + 1);
      if (!cached) continue;

      const y = i * rowHeight * scale;
      const text = cached.text;

      if (useBlockMode) {
        // Fast block mode: one rect per span, no per-character loop
        paintMinimapLineBlock(ctx, cached, y, lineH, tokenColors);
      } else if (cached.spans && cached.spans.length > 0) {
        paintMinimapLineWithSpans(ctx, cached, y, lineH, tokenColors);
      } else if (text && text.trim().length > 0) {
        paintMinimapLinePlain(ctx, text, y, lineH, tokenColors.default);
      }
    }
    ctx.globalAlpha = 1;

    updateMinimapViewport(scale, containerHeight, rowHeight);
  }

  function paintMinimapLineWithSpans(ctx, cached, y, lineH, tokenColors) {
    const text = cached.text;
    // Render highlighted spans
    for (const span of cached.spans) {
      ctx.fillStyle = tokenColors[span.highlight_class] || tokenColors.default;
      ctx.globalAlpha = 0.9;
      for (let c = span.start_col; c < span.end_col; c++) {
        const ch = text[c];
        if (ch === ' ' || ch === '\t') continue;
        const x = c * MINIMAP_CHAR_WIDTH;
        if (x >= MINIMAP_WIDTH) break;
        ctx.fillRect(x, y, Math.max(1, MINIMAP_CHAR_WIDTH - 0.3), lineH);
      }
    }
    // Render gaps and trailing text
    ctx.fillStyle = tokenColors.default;
    ctx.globalAlpha = 0.5;
    let lastEnd = 0;
    for (const span of cached.spans) {
      for (let c = lastEnd; c < span.start_col; c++) {
        if (text[c] === ' ' || text[c] === '\t') continue;
        const x = c * MINIMAP_CHAR_WIDTH;
        if (x >= MINIMAP_WIDTH) break;
        ctx.fillRect(x, y, Math.max(1, MINIMAP_CHAR_WIDTH - 0.3), lineH);
      }
      lastEnd = Math.max(lastEnd, span.end_col);
    }
    for (let c = lastEnd; c < text.length; c++) {
      if (text[c] === ' ' || text[c] === '\t') continue;
      const x = c * MINIMAP_CHAR_WIDTH;
      if (x >= MINIMAP_WIDTH) break;
      ctx.fillRect(x, y, Math.max(1, MINIMAP_CHAR_WIDTH - 0.3), lineH);
    }
  }

  function paintMinimapLinePlain(ctx, text, y, lineH, color) {
    ctx.fillStyle = color;
    ctx.globalAlpha = 0.5;
    for (let c = 0; c < text.length; c++) {
      if (text[c] === ' ' || text[c] === '\t') continue;
      const x = c * MINIMAP_CHAR_WIDTH;
      if (x >= MINIMAP_WIDTH) break;
      ctx.fillRect(x, y, Math.max(1, MINIMAP_CHAR_WIDTH - 0.3), lineH);
    }
  }

  /** Fast block mode for large files: one rect per span, no per-character loop. */
  function paintMinimapLineBlock(ctx, cached, y, lineH, tokenColors) {
    const text = cached.text;
    if (!text || text.trim().length === 0) return;

    if (cached.spans && cached.spans.length > 0) {
      for (const span of cached.spans) {
        const x = span.start_col * MINIMAP_CHAR_WIDTH;
        const w = Math.max(1, (span.end_col - span.start_col) * MINIMAP_CHAR_WIDTH);
        ctx.fillStyle = tokenColors[span.highlight_class] || tokenColors.default;
        ctx.globalAlpha = 0.8;
        ctx.fillRect(x, y, Math.min(w, MINIMAP_WIDTH - x), lineH);
      }
    } else {
      const indent = text.length - text.trimStart().length;
      const contentLen = text.trim().length;
      const x = indent * MINIMAP_CHAR_WIDTH;
      const w = Math.max(1, contentLen * MINIMAP_CHAR_WIDTH);
      ctx.fillStyle = tokenColors.default;
      ctx.globalAlpha = 0.5;
      ctx.fillRect(x, y, Math.min(w, MINIMAP_WIDTH - x), lineH);
    }
  }

  // Kept for backward compat — called from renderFromCache
  function renderMinimap() { scheduleMinimapRepaint(); }

  function updateMinimapViewport(scaleOverride, containerHeightOverride, rowHeightOverride) {
    if (!minimapContainer || minimapContainer.style.display === 'none') return;

    const containerHeight = containerHeightOverride || codeWrapper.clientHeight || 400;
    const rowH = rowHeightOverride || (MINIMAP_LINE_HEIGHT + MINIMAP_LINE_GAP);
    const totalMinimapHeight = lineCount * rowH;
    const scale = scaleOverride ?? (totalMinimapHeight > containerHeight
      ? containerHeight / totalMinimapHeight
      : 1);

    const canvasH = Math.min(totalMinimapHeight, containerHeight);
    const scrollTop = scrollContainer.scrollTop;
    const viewportHeight = scrollContainer.clientHeight;
    // Use browser-measured scroll height (includes spacer + margin padding)
    const totalScrollH = scrollContainer.scrollHeight || lineCount * LINE_HEIGHT;
    const maxScroll = Math.max(1, totalScrollH - viewportHeight);

    // Viewport indicator height: proportion of visible lines to total content lines
    const contentHeight = lineCount * LINE_HEIGHT;
    const vpHeight = contentHeight > 0
      ? Math.max(8, (viewportHeight / contentHeight) * canvasH)
      : 20;

    // Viewport position: scroll fraction mapped to the available travel range
    // When scrollTop=0 -> vpTop=0; when scrollTop=maxScroll -> vpTop=canvasH-vpHeight
    const scrollFraction = Math.min(1, scrollTop / maxScroll);
    const vpTop = scrollFraction * (canvasH - vpHeight);

    minimapViewport.style.top = Math.max(0, vpTop) + 'px';
    minimapViewport.style.height = vpHeight + 'px';
  }

  // Minimap click/drag to scroll
  let minimapDragging = false;
  function minimapScrollTo(clientY) {
    const rect = minimapCanvas.getBoundingClientRect();
    const zoom = getZoom();
    const relY = (clientY - rect.top) / zoom;
    const canvasH = parseFloat(minimapCanvas.style.height) || 1;
    const viewportHeight = scrollContainer.clientHeight;
    // Use browser-measured scroll height (includes spacer + margin padding)
    const totalScrollH = scrollContainer.scrollHeight || lineCount * LINE_HEIGHT;
    const maxScroll = Math.max(0, totalScrollH - viewportHeight);

    // Compute viewport indicator height (same formula as updateMinimapViewport)
    const contentHeight = lineCount * LINE_HEIGHT;
    const vpHeight = contentHeight > 0
      ? Math.max(8, (viewportHeight / contentHeight) * canvasH)
      : 20;

    // The viewport center should follow the mouse. The center travels from
    // vpHeight/2 (scroll=0) to canvasH-vpHeight/2 (scroll=maxScroll).
    const travelRange = canvasH - vpHeight;
    if (travelRange <= 0) {
      scrollContainer.scrollTop = 0;
      return;
    }

    const fraction = Math.max(0, Math.min(1, (relY - vpHeight / 2) / travelRange));
    scrollContainer.scrollTop = fraction * maxScroll;
  }

  minimapContainer.addEventListener('mousedown', (e) => {
    e.preventDefault();
    minimapDragging = true;
    minimapScrollTo(e.clientY);
  });
  window.addEventListener('mousemove', (e) => {
    if (minimapDragging) minimapScrollTo(e.clientY);
  });
  window.addEventListener('mouseup', () => { minimapDragging = false; });

  // Pasted-text scratch buffers (created from the chat-input paste handler)
  // are pure prose pulled from the user's clipboard, so we force word-wrap on
  // and hide the minimap regardless of the user's editor preferences. The
  // minimap on a wall of unhighlighted text adds noise and the lack of wrap
  // forces horizontal scrolling on long lines. The buffer's `filePath` is the
  // synthetic title we passed to `open_scratch_buffer` (`Pasted text #N`),
  // which is the only signal we have here.
  function isPastedTextBuffer(bufferId) {
    if (!bufferId) return false;
    const buffer = editorStore.getState('openBuffers')[bufferId];
    if (!buffer) return false;
    return typeof buffer.filePath === 'string' && buffer.filePath.startsWith('Pasted text');
  }

  // Apply editor settings (word wrap, line numbers, font size)
  function applyEditorSettings(settings) {
    if (!settings) return;
    const editor = settings.editor || {};
    const pastedOverride = isPastedTextBuffer(currentBufferId);

    // Line numbers: toggle gutter visibility
    gutterEl.style.display = editor.line_numbers === false ? 'none' : '';

    // Word wrap: toggle via CSS class on the editor pane. Pasted-text
    // buffers force-enable wrap regardless of the user's setting.
    const wrapOn = pastedOverride ? true : !!editor.word_wrap;
    container.classList.toggle('editor-pane--word-wrap', wrapOn);

    // Line-renderer config (whitespace, zero-width, bracket colors)
    setRendererConfig({
      render_whitespace: editor.render_whitespace || 'none',
      show_zero_width: !!editor.show_zero_width,
      bracket_pair_colorization: !!editor.bracket_pair_colorization,
    });

    // Minimap: toggle visibility. Pasted-text scratch buffers force the
    // minimap off — see `isPastedTextBuffer` above for the rationale.
    if (minimapContainer) {
      const minimapOn = pastedOverride ? false : !!editor.minimap;
      minimapContainer.style.display = minimapOn ? '' : 'none';
      codeWrapper.classList.toggle('minimap-visible', minimapOn);
      if (minimapOn && currentBufferId) {
        startMinimapLoad(currentBufferId);
      } else {
        minimapLoadGeneration++; // cancel any in-progress load
      }
    }

    const newLineHeight = computeLineHeight();
    if (newLineHeight !== LINE_HEIGHT) {
      LINE_HEIGHT = newLineHeight;
      invalidateLayoutCache();
      container.style.setProperty('--editor-line-height', LINE_HEIGHT + 'px');
    }

    // Re-render visible lines to apply all settings changes
    if (currentBufferId) {
      lineCache.clear();
      updateSpacerHeights();
      const vh = scrollContainer.clientHeight || 600;
      const scrollTop = scrollContainer.scrollTop;
      visibleStart = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
      visibleEnd = Math.min(lineCount, Math.ceil((scrollTop + vh) / LINE_HEIGHT) + OVERSCAN);
      loadVisibleLines(currentBufferId, visibleStart, visibleEnd);
    }
  }

  settingsStore.subscribe('settings', applyEditorSettings);
  // Apply initial settings
  applyEditorSettings(settingsStore.getState('settings'));

  // Recalculate bottom padding when editor resizes (e.g. terminal open/close).
  // Also flush the layout-offset cache so the next click/cursor update
  // re-measures container positions against the post-resize geometry.
  new ResizeObserver(() => {
    invalidateLayoutCache();
    if (!currentBufferId) return;
    requestAnimationFrame(() => {
      updateSpacerHeights();
      updateMinimapViewport();
      scheduleMinimapRepaint();
    });
  }).observe(scrollContainer);

  // Also recalculate when terminal panel toggles or resizes — the editor
  // viewport height changes and the bottom padding needs to match.
  function onPanelLayoutChange() {
    invalidateLayoutCache();
    if (!currentBufferId) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        updateSpacerHeights();
        updateMinimapViewport();
      });
    });
  }
  uiStore.subscribe('bottomPanelVisible', onPanelLayoutChange);
  uiStore.subscribe('panelHeight', onPanelLayoutChange);

  return container;
}
