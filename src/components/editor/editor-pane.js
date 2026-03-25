import { el } from '../../utils/dom.js';
import { editorStore, updateBufferModified, saveActiveBuffer, closeBuffer, setActiveBuffer, openFile } from '../../state/editor.js';
import * as api from '../../lib/tauri-api.js';
import { renderLine } from './line-renderer.js';
import { renderGutter } from './gutter-renderer.js';
import { createAutocomplete } from './autocomplete.js';
import { createHoverTooltip } from './hover-tooltip.js';

const LINE_HEIGHT = 20;
const OVERSCAN = 30;
const LINES_PADDING_LEFT = 4;

export function createEditorPane() {
  const container = el('div', { class: 'editor-pane' });

  // Gutter (with virtual scrolling support)
  const gutterEl = el('div', { class: 'editor-gutter-container' });
  const gutterSpacer = el('div', { class: 'editor-gutter-spacer' });
  const gutterContent = el('div', { class: 'editor-gutter-content' });
  gutterEl.appendChild(gutterSpacer);
  gutterEl.appendChild(gutterContent);

  // Code area (virtual scrolling)
  const codeWrapper = el('div', { class: 'editor-code-wrapper' });
  const scrollContainer = el('div', { class: 'editor-scroll-container' });
  const spacer = el('div', { class: 'editor-spacer' });
  const linesContainer = el('div', { class: 'editor-lines-container' });

  scrollContainer.appendChild(spacer);
  scrollContainer.appendChild(linesContainer);
  codeWrapper.appendChild(scrollContainer);

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
      api.editBuffer(currentBufferId, cursorLine, cursorCol, text, 0).then((result) => {
        if (result) {
          cursorCol += text.length;
          lineCount = result.line_count;
          editorStore.setState({ cursorLine, cursorCol });
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          reloadAllLines();
        }
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

  let currentBufferId = null;
  let docVersion = 1;
  let lineCount = 0;
  let visibleStart = 0;
  let visibleEnd = 0;
  let renderedLines = [];
  let cursorLine = 0;
  let cursorCol = 0;
  let isComposing = false;

  // --- Full-file line cache (all lines loaded on open, re-loaded on edit) ---
  const lineCache = new Map(); // lineNumber (1-based) -> RenderedLine
  let fetchGeneration = 0;

  /** Load ALL lines from the backend into cache. Called on file open and after edits. */
  async function loadAllLines(bufferId) {
    if (!bufferId || lineCount === 0) return;
    const gen = ++fetchGeneration;
    try {
      const lines = await api.getVisibleLines(bufferId, 0, lineCount);
      if (gen !== fetchGeneration) return; // stale
      if (!lines || editorStore.getState('activeBufferId') !== bufferId) return;

      lineCache.clear();
      for (const line of lines) {
        lineCache.set(line.line_number, line);
      }

      // Render whatever is currently visible
      renderFromCache();
    } catch (e) {
      console.error('Failed to load lines:', e);
    }
  }

  /** After an edit: clear cache and re-load all lines. */
  function reloadAllLines() {
    lineCache.clear();
    loadAllLines(currentBufferId);
  }

  /** Render the current visible range from cache (always synchronous if cache is populated). */
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

  // --- Char width measurement ---
  let _charWidth = 0;
  function getCharWidth() {
    if (_charWidth > 0) return _charWidth;
    const span = document.createElement('span');
    span.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;' +
      'font-family:var(--font-family-mono);font-size:var(--font-size-editor);line-height:20px;';
    span.textContent = 'X'.repeat(100);
    document.body.appendChild(span);
    _charWidth = span.getBoundingClientRect().width / 100;
    document.body.removeChild(span);
    return _charWidth;
  }

  window.addEventListener('resize', () => { _charWidth = 0; });

  // --- Gutter width ---
  function updateGutterWidth() {
    const digits = Math.max(2, String(lineCount).length);
    const width = Math.ceil(digits * getCharWidth() + 20 + 1);
    gutterEl.style.minWidth = `${width}px`;
  }

  // --- Rendering ---
  function renderVisibleLines() {
    if (renderedLines.length === 0) return;

    const startLine = renderedLines[0].line_number - 1; // 0-based

    // Gutter
    gutterContent.replaceChildren(renderGutter(renderedLines, cursorLine + 1));
    gutterContent.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    // Lines
    const frag = document.createDocumentFragment();
    for (const line of renderedLines) {
      frag.appendChild(renderLine(line));
    }
    linesContainer.replaceChildren(frag);
    linesContainer.style.transform = `translateY(${startLine * LINE_HEIGHT}px)`;

    updateCursorPosition();
  }

  function updateCursorPosition() {
    const charWidth = getCharWidth();
    const gutterWidth = gutterEl.offsetWidth || 50;

    const cursorY = cursorLine * LINE_HEIGHT - scrollContainer.scrollTop;
    const cursorX = gutterWidth + cursorCol * charWidth + LINES_PADDING_LEFT;

    cursor.style.top = `${cursorY}px`;
    cursor.style.left = `${cursorX}px`;
    cursor.style.height = `${LINE_HEIGHT}px`;
    cursor.style.display = currentBufferId ? 'block' : 'none';
  }

  // --- Scroll handling (purely local, no IPC) ---
  function computeVisibleRange(scrollTop) {
    const viewportHeight = scrollContainer.clientHeight;
    const newStart = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
    const newEnd = Math.min(lineCount, Math.ceil((scrollTop + viewportHeight) / LINE_HEIGHT) + OVERSCAN);
    return { newStart, newEnd };
  }

  let rafId = 0;

  scrollContainer.addEventListener('scroll', () => {
    gutterEl.scrollTop = scrollContainer.scrollTop;
    updateCursorPosition();

    if (!rafId) {
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        if (!currentBufferId) return;

        const scrollTop = scrollContainer.scrollTop;
        editorStore.setState({ scrollTop });

        const { newStart, newEnd } = computeVisibleRange(scrollTop);

        if (newStart !== visibleStart || newEnd !== visibleEnd) {
          visibleStart = newStart;
          visibleEnd = newEnd;
          renderFromCache(); // Always synchronous — no IPC during scroll
        }
      });
    }
  });

  // --- Mouse event handlers ---
  codeWrapper.addEventListener('mousemove', (e) => {
    if (!currentBufferId) return;
    const rect = scrollContainer.getBoundingClientRect();
    const relY = e.clientY - rect.top + scrollContainer.scrollTop;
    const relX = e.clientX - rect.left - LINES_PADDING_LEFT;
    if (relX < 0) { hoverTooltip.cancelSchedule(); return; }

    const hoverLine = Math.floor(relY / LINE_HEIGHT);
    const charWidth = getCharWidth();
    const hoverCol = Math.max(0, Math.round(relX / charWidth));

    hoverTooltip.scheduleShow(currentBufferId, hoverLine, hoverCol, e.clientX, e.clientY);
  });

  codeWrapper.addEventListener('mouseleave', () => {
    hoverTooltip.hide();
  });

  // Ctrl+Click: Go to definition
  container.addEventListener('click', (e) => {
    if (e.ctrlKey && currentBufferId) {
      const rect = scrollContainer.getBoundingClientRect();
      const relY = e.clientY - rect.top + scrollContainer.scrollTop;
      const relX = e.clientX - rect.left - LINES_PADDING_LEFT;
      const clickLine = Math.floor(relY / LINE_HEIGHT);
      const charWidth = getCharWidth();
      const clickCol = Math.max(0, Math.round(relX / charWidth));

      api.gotoDefinition(currentBufferId, clickLine, clickCol).then((locs) => {
        if (locs && locs.length > 0) {
          openFile(locs[0].file_path);
        }
      }).catch(() => {});
    }
  });

  // Focus textarea on click and position cursor
  container.addEventListener('click', (e) => {
    textarea.focus();
    hoverTooltip.hide();
    autocomplete.hide();

    const rect = scrollContainer.getBoundingClientRect();
    const relY = e.clientY - rect.top + scrollContainer.scrollTop;
    const relX = e.clientX - rect.left - LINES_PADDING_LEFT + scrollContainer.scrollLeft;

    const clickedLine = Math.floor(relY / LINE_HEIGHT);
    const charWidth = getCharWidth();
    const clickedCol = Math.max(0, Math.round(relX / charWidth));

    cursorLine = Math.max(0, Math.min(lineCount - 1, clickedLine));
    cursorCol = clickedCol;

    // Clamp col to line length
    const lineText = renderedLines.find(l => l.line_number === cursorLine + 1);
    if (lineText) {
      cursorCol = Math.min(cursorCol, lineText.text.length);
    }

    editorStore.setState({ cursorLine, cursorCol });
    updateCursorPosition();
  });

  // --- Input handling ---
  textarea.addEventListener('compositionstart', () => { isComposing = true; });
  textarea.addEventListener('compositionend', () => {
    isComposing = false;
    handleInput();
  });

  textarea.addEventListener('input', () => {
    if (!isComposing) handleInput();
  });

  async function handleInput() {
    const text = textarea.value;
    if (!text || !currentBufferId) return;
    textarea.value = '';

    try {
      const result = await api.editBuffer(currentBufferId, cursorLine, cursorCol, text, 0);
      if (result) {
        lineCount = result.line_count;
        if (text.includes('\n')) {
          const parts = text.split('\n');
          cursorLine += parts.length - 1;
          cursorCol = parts[parts.length - 1].length;
        } else {
          cursorCol += text.length;
        }
        editorStore.setState({ cursorLine, cursorCol });
        updateBufferModified(currentBufferId, result.is_modified, result.line_count);
        updateSpacerHeights();
        reloadAllLines();
        docVersion++;
        api.lspNotifyChange(currentBufferId, docVersion).catch(() => {});
      }
    } catch (e) {
      console.error('Edit failed:', e);
    }
  }

  function updateSpacerHeights() {
    spacer.style.height = `${lineCount * LINE_HEIGHT}px`;
    gutterSpacer.style.height = `${lineCount * LINE_HEIGHT}px`;
    updateGutterWidth();
  }

  // --- Keyboard shortcuts ---
  textarea.addEventListener('keydown', async (e) => {
    if (!currentBufferId) return;

    if (autocomplete.handleKey(e)) return;

    // Ctrl+Space: trigger autocomplete
    if (e.ctrlKey && e.key === ' ') {
      e.preventDefault();
      const gutterWidth = gutterEl.offsetWidth || 50;
      const charWidth = getCharWidth();
      const x = gutterWidth + cursorCol * charWidth + LINES_PADDING_LEFT;
      const y = cursorLine * LINE_HEIGHT - scrollContainer.scrollTop + LINE_HEIGHT;
      autocomplete.show(currentBufferId, cursorLine, cursorCol, x, y);
      return;
    }

    // F12: Go to definition
    if (e.key === 'F12') {
      e.preventDefault();
      try {
        const locs = await api.gotoDefinition(currentBufferId, cursorLine, cursorCol);
        if (locs && locs.length > 0) {
          openFile(locs[0].file_path);
        }
      } catch (err) { console.error('Goto definition failed:', err); }
      return;
    }

    // Ctrl+Shift+I: Format document
    if (e.ctrlKey && e.shiftKey && e.key === 'I') {
      e.preventDefault();
      try {
        await api.formatDocument(currentBufferId);
        reloadAllLines();
      } catch (err) { console.error('Format failed:', err); }
      return;
    }

    // Ctrl+S: save
    if (e.ctrlKey && e.key === 's') {
      e.preventDefault();
      await saveActiveBuffer();
      api.lspNotifySave(currentBufferId).catch(() => {});
      return;
    }

    // Ctrl+W: close active tab
    if (e.ctrlKey && e.key === 'w') {
      e.preventDefault();
      if (currentBufferId) closeBuffer(currentBufferId);
      return;
    }

    // Ctrl+Tab / Ctrl+Shift+Tab: cycle tabs
    if (e.ctrlKey && e.key === 'Tab') {
      e.preventDefault();
      const buffers = editorStore.getState('openBuffers');
      const ids = Object.keys(buffers).map(Number);
      if (ids.length < 2) return;
      const idx = ids.indexOf(currentBufferId);
      const next = e.shiftKey
        ? (idx - 1 + ids.length) % ids.length
        : (idx + 1) % ids.length;
      setActiveBuffer(ids[next]);
      return;
    }

    // Ctrl+Z: undo
    if (e.ctrlKey && e.key === 'z' && !e.shiftKey) {
      e.preventDefault();
      try {
        const result = await api.undoEdit(currentBufferId);
        if (result) {
          lineCount = result.line_count;
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          updateSpacerHeights();
          reloadAllLines();
        }
      } catch (e) { console.error('Undo failed:', e); }
      return;
    }

    // Ctrl+Y or Ctrl+Shift+Z: redo
    if ((e.ctrlKey && e.key === 'y') || (e.ctrlKey && e.shiftKey && e.key === 'Z')) {
      e.preventDefault();
      try {
        const result = await api.redoEdit(currentBufferId);
        if (result) {
          lineCount = result.line_count;
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          updateSpacerHeights();
          reloadAllLines();
        }
      } catch (e) { console.error('Redo failed:', e); }
      return;
    }

    // Enter
    if (e.key === 'Enter') {
      e.preventDefault();
      textarea.value = '';
      try {
        const result = await api.editBuffer(currentBufferId, cursorLine, cursorCol, '\n', 0);
        if (result) {
          lineCount = result.line_count;
          cursorLine++;
          cursorCol = 0;
          editorStore.setState({ cursorLine, cursorCol });
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          updateSpacerHeights();
          reloadAllLines();
          ensureCursorVisible();
        }
      } catch (err) { console.error('Enter failed:', err); }
      return;
    }

    // Backspace
    if (e.key === 'Backspace') {
      e.preventDefault();
      textarea.value = '';
      if (cursorCol > 0) {
        try {
          const result = await api.editBuffer(currentBufferId, cursorLine, cursorCol - 1, '', 1);
          if (result) {
            cursorCol--;
            lineCount = result.line_count;
            editorStore.setState({ cursorLine, cursorCol });
            updateBufferModified(currentBufferId, result.is_modified, result.line_count);
            reloadAllLines();
          }
        } catch (err) { console.error('Backspace failed:', err); }
      } else if (cursorLine > 0) {
        const prevLine = renderedLines.find(l => l.line_number === cursorLine);
        const prevLineLen = prevLine ? prevLine.text.length : 0;
        try {
          const result = await api.editBuffer(currentBufferId, cursorLine, 0, '', 1);
          if (result) {
            cursorLine--;
            cursorCol = prevLineLen;
            lineCount = result.line_count;
            editorStore.setState({ cursorLine, cursorCol });
            updateBufferModified(currentBufferId, result.is_modified, result.line_count);
            updateSpacerHeights();
            reloadAllLines();
          }
        } catch (err) { console.error('Backspace join failed:', err); }
      }
      return;
    }

    // Delete
    if (e.key === 'Delete') {
      e.preventDefault();
      textarea.value = '';
      try {
        const result = await api.editBuffer(currentBufferId, cursorLine, cursorCol, '', 1);
        if (result) {
          lineCount = result.line_count;
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          updateSpacerHeights();
          reloadAllLines();
        }
      } catch (err) { console.error('Delete failed:', err); }
      return;
    }

    // Tab
    if (e.key === 'Tab') {
      e.preventDefault();
      textarea.value = '';
      const spaces = '    ';
      try {
        const result = await api.editBuffer(currentBufferId, cursorLine, cursorCol, spaces, 0);
        if (result) {
          cursorCol += spaces.length;
          editorStore.setState({ cursorCol });
          updateBufferModified(currentBufferId, result.is_modified, result.line_count);
          reloadAllLines();
        }
      } catch (err) { console.error('Tab failed:', err); }
      return;
    }

    // Arrow keys
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (cursorLine > 0) {
        cursorLine--;
        clampCursorCol();
        editorStore.setState({ cursorLine, cursorCol });
        updateCursorPosition();
        ensureCursorVisible();
      }
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (cursorLine < lineCount - 1) {
        cursorLine++;
        clampCursorCol();
        editorStore.setState({ cursorLine, cursorCol });
        updateCursorPosition();
        ensureCursorVisible();
      }
      return;
    }
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      if (cursorCol > 0) {
        cursorCol--;
      } else if (cursorLine > 0) {
        cursorLine--;
        const line = renderedLines.find(l => l.line_number === cursorLine + 1);
        cursorCol = line ? line.text.length : 0;
      }
      editorStore.setState({ cursorLine, cursorCol });
      updateCursorPosition();
      return;
    }
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      const currentLine = renderedLines.find(l => l.line_number === cursorLine + 1);
      const maxCol = currentLine ? currentLine.text.length : 0;
      if (cursorCol < maxCol) {
        cursorCol++;
      } else if (cursorLine < lineCount - 1) {
        cursorLine++;
        cursorCol = 0;
      }
      editorStore.setState({ cursorLine, cursorCol });
      updateCursorPosition();
      return;
    }

    // Home
    if (e.key === 'Home') {
      e.preventDefault();
      cursorCol = 0;
      editorStore.setState({ cursorCol });
      updateCursorPosition();
      return;
    }
    // End
    if (e.key === 'End') {
      e.preventDefault();
      const line = renderedLines.find(l => l.line_number === cursorLine + 1);
      cursorCol = line ? line.text.length : 0;
      editorStore.setState({ cursorCol });
      updateCursorPosition();
      return;
    }
  });

  function clampCursorCol() {
    const line = renderedLines.find(l => l.line_number === cursorLine + 1);
    if (line) {
      cursorCol = Math.min(cursorCol, line.text.length);
    }
  }

  function ensureCursorVisible() {
    const cursorY = cursorLine * LINE_HEIGHT;
    const viewTop = scrollContainer.scrollTop;
    const viewBottom = viewTop + scrollContainer.clientHeight;

    if (cursorY < viewTop) {
      scrollContainer.scrollTop = cursorY;
    } else if (cursorY + LINE_HEIGHT > viewBottom) {
      scrollContainer.scrollTop = cursorY - scrollContainer.clientHeight + LINE_HEIGHT;
    }
  }

  // --- Buffer lifecycle ---
  function onActiveBufferChange(bufferId) {
    currentBufferId = bufferId;
    lineCache.clear();

    if (!bufferId) {
      linesContainer.replaceChildren();
      gutterContent.replaceChildren();
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

    const viewportHeight = scrollContainer.clientHeight || 600;
    visibleStart = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
    visibleEnd = Math.min(lineCount, Math.ceil((scrollTop + viewportHeight) / LINE_HEIGHT) + OVERSCAN);

    // Load ALL lines at once — scrolling will be fully synchronous after this
    loadAllLines(bufferId);
    textarea.focus();

    api.lspNotifyOpen(bufferId).catch(() => {});
    docVersion = 1;
  }

  editorStore.subscribe('activeBufferId', onActiveBufferChange);

  return container;
}
