import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

const ICON_HIGHLIGHT = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 11-6 6v3h3l6-6"/><path d="m22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4"/></svg>';
const ICON_TEXTBOX = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 7 4 4 20 4 20 7"/><line x1="9" x2="15" y1="20" y2="20"/><line x1="12" x2="12" y1="4" y2="20"/></svg>';
const ICON_DELETE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-2 14a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>';

export function createPdfPreview() {
  const container = el('div', { class: 'preview-container pdf-preview' });
  const toolbar = el('div', { class: 'preview-toolbar pdf-annotation-toolbar' });
  const scrollWrap = el('div', { class: 'pdf-scroll-wrap' });
  const pagesContainer = el('div', { class: 'pdf-pages-container' });
  const info = el('div', { class: 'preview-info' });

  scrollWrap.appendChild(pagesContainer);
  container.appendChild(toolbar);
  container.appendChild(scrollWrap);
  container.appendChild(info);

  let pdfDoc = null;
  let pdfjsLib = null;
  let currentScale = 1.0;
  let activeTool = null;
  let annotations = [];
  let annotationLayer = null;
  let isDragging = false;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragRect = null;

  const zoomOutBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom Out' }, '−');
  const zoomLabel = el('span', { class: 'preview-toolbar-label' }, '100%');
  const zoomInBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom In' }, '+');
  const spacer = el('div', { class: 'preview-toolbar-spacer' });
  const highlightBtn = el('button', { class: 'preview-toolbar-btn preview-toolbar-icon-btn', title: 'Highlight' });
  const textBoxBtn = el('button', { class: 'preview-toolbar-btn preview-toolbar-icon-btn', title: 'Text Box' });
  const deleteBtn = el('button', { class: 'preview-toolbar-btn preview-toolbar-icon-btn', title: 'Delete Annotation' });
  highlightBtn.innerHTML = ICON_HIGHLIGHT;
  textBoxBtn.innerHTML = ICON_TEXTBOX;
  deleteBtn.innerHTML = ICON_DELETE;

  toolbar.appendChild(zoomOutBtn);
  toolbar.appendChild(zoomLabel);
  toolbar.appendChild(zoomInBtn);
  toolbar.appendChild(spacer);
  toolbar.appendChild(highlightBtn);
  toolbar.appendChild(textBoxBtn);
  toolbar.appendChild(deleteBtn);

  function setActiveTool(tool) {
    activeTool = activeTool === tool ? null : tool;
    highlightBtn.classList.toggle('active', activeTool === 'highlight');
    textBoxBtn.classList.toggle('active', activeTool === 'textbox');
    deleteBtn.classList.toggle('active', activeTool === 'delete');
    pagesContainer.classList.toggle('pdf-tool-active', activeTool !== null);
    if (activeTool === 'highlight') {
      scrollWrap.style.cursor = 'crosshair';
    } else if (activeTool === 'textbox') {
      scrollWrap.style.cursor = 'text';
    } else if (activeTool === 'delete') {
      scrollWrap.style.cursor = 'pointer';
    } else {
      scrollWrap.style.cursor = '';
    }
  }

  highlightBtn.addEventListener('click', () => setActiveTool('highlight'));
  textBoxBtn.addEventListener('click', () => setActiveTool('textbox'));
  deleteBtn.addEventListener('click', () => setActiveTool('delete'));

  zoomInBtn.addEventListener('click', async () => {
    currentScale = Math.min(5, currentScale + 0.25);
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
    if (pdfDoc) await renderAllPages();
  });

  zoomOutBtn.addEventListener('click', async () => {
    currentScale = Math.max(0.25, currentScale - 0.25);
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
    if (pdfDoc) await renderAllPages();
  });

  scrollWrap.addEventListener('mousedown', (e) => {
    if (!activeTool || !annotationLayer) return;

    if (activeTool === 'highlight') {
      e.preventDefault();
      isDragging = true;
      const layerRect = annotationLayer.getBoundingClientRect();
      dragStartX = e.clientX - layerRect.left;
      dragStartY = e.clientY - layerRect.top;

      dragRect = el('div', { class: 'pdf-annotation pdf-annotation-highlight pdf-annotation-drag-preview' });
      dragRect.style.left = dragStartX + 'px';
      dragRect.style.top = dragStartY + 'px';
      dragRect.style.width = '0px';
      dragRect.style.height = '0px';
      annotationLayer.appendChild(dragRect);
    }

    if (activeTool === 'textbox') {
      e.preventDefault();
      const layerRect = annotationLayer.getBoundingClientRect();
      const x = e.clientX - layerRect.left;
      const y = e.clientY - layerRect.top;
      placeTextBox(x, y);
    }
  });

  scrollWrap.addEventListener('mousemove', (e) => {
    if (!isDragging || activeTool !== 'highlight' || !dragRect || !annotationLayer) return;
    const layerRect = annotationLayer.getBoundingClientRect();
    const currentX = e.clientX - layerRect.left;
    const currentY = e.clientY - layerRect.top;
    const left = Math.min(dragStartX, currentX);
    const top = Math.min(dragStartY, currentY);
    const width = Math.abs(currentX - dragStartX);
    const height = Math.abs(currentY - dragStartY);
    dragRect.style.left = left + 'px';
    dragRect.style.top = top + 'px';
    dragRect.style.width = width + 'px';
    dragRect.style.height = height + 'px';
  });

  scrollWrap.addEventListener('mouseup', (e) => {
    if (!isDragging || activeTool !== 'highlight' || !dragRect || !annotationLayer) return;
    isDragging = false;

    const layerRect = annotationLayer.getBoundingClientRect();
    const currentX = e.clientX - layerRect.left;
    const currentY = e.clientY - layerRect.top;
    const width = Math.abs(currentX - dragStartX);
    const height = Math.abs(currentY - dragStartY);

    dragRect.classList.remove('pdf-annotation-drag-preview');

    if (width < 4 || height < 4) {
      dragRect.remove();
      dragRect = null;
      return;
    }

    const annotation = { type: 'highlight', element: dragRect };
    annotations.push(annotation);
    dragRect.addEventListener('click', (ev) => handleAnnotationClick(ev, annotation));
    dragRect = null;
  });

  function placeTextBox(x, y) {
    const box = el('div', { class: 'pdf-annotation pdf-annotation-textbox', contenteditable: 'true' });
    box.style.left = x + 'px';
    box.style.top = y + 'px';
    annotationLayer.appendChild(box);

    const annotation = { type: 'textbox', element: box };
    annotations.push(annotation);
    box.addEventListener('click', (ev) => handleAnnotationClick(ev, annotation));
    setTimeout(() => box.focus(), 0);
  }

  function handleAnnotationClick(e, annotation) {
    if (activeTool !== 'delete') return;
    e.stopPropagation();
    annotation.element.remove();
    const idx = annotations.indexOf(annotation);
    if (idx !== -1) annotations.splice(idx, 1);
  }

  async function loadPdfjs() {
    if (pdfjsLib) return pdfjsLib;
    pdfjsLib = await import('pdfjs-dist');
    pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
      'pdfjs-dist/build/pdf.worker.min.mjs',
      import.meta.url
    ).href;
    return pdfjsLib;
  }

  async function renderAllPages() {
    if (!pdfDoc) return;

    const totalPages = pdfDoc.numPages;
    for (const layer of pagesContainer.querySelectorAll('.pdf-text-layer')) {
      unregisterTextLayer(layer);
    }
    pagesContainer.innerHTML = '';
    annotations = [];
    annotationLayer = null;

    const pageGap = 12;
    const outputScale = window.devicePixelRatio || 1;
    let totalHeight = 0;
    let maxWidth = 0;
    const pageRenderJobs = [];

    for (let pageNum = 1; pageNum <= totalPages; pageNum++) {
      const page = await pdfDoc.getPage(pageNum);
      const viewport = page.getViewport({ scale: currentScale });

      const cssWidth = Math.floor(viewport.width);
      const cssHeight = Math.floor(viewport.height);

      const pageWrap = el('div', { class: 'pdf-page' });
      pageWrap.style.position = 'absolute';
      pageWrap.style.left = '0';
      pageWrap.style.top = totalHeight + 'px';
      pageWrap.style.width = cssWidth + 'px';
      pageWrap.style.height = cssHeight + 'px';
      pageWrap.style.setProperty('--scale-factor', String(currentScale));
      pageWrap.style.setProperty('--total-scale-factor', String(currentScale));
      pageWrap.style.setProperty('--user-unit', '1');
      pageWrap.style.setProperty('--scale-round-x', '1px');
      pageWrap.style.setProperty('--scale-round-y', '1px');

      const canvas = el('canvas', { class: 'pdf-page-canvas' });
      canvas.width = Math.floor(cssWidth * outputScale);
      canvas.height = Math.floor(cssHeight * outputScale);
      canvas.style.width = cssWidth + 'px';
      canvas.style.height = cssHeight + 'px';

      const ctx = canvas.getContext('2d');
      const transform = outputScale !== 1 ? [outputScale, 0, 0, outputScale, 0, 0] : null;

      const textLayerDiv = el('div', { class: 'pdf-text-layer textLayer' });

      pageWrap.appendChild(canvas);
      pageWrap.appendChild(textLayerDiv);

      pageRenderJobs.push({ page, viewport, canvas, ctx, transform, textLayerDiv, pageWrap });

      if (cssWidth > maxWidth) maxWidth = cssWidth;
      totalHeight += cssHeight + pageGap;
    }

    totalHeight = Math.max(0, totalHeight - pageGap);

    const inner = el('div', { class: 'pdf-pages-inner' });
    inner.style.position = 'relative';
    inner.style.width = maxWidth + 'px';
    inner.style.height = totalHeight + 'px';

    for (const job of pageRenderJobs) {
      inner.appendChild(job.pageWrap);
    }

    const layer = el('div', { class: 'pdf-annotation-layer' });
    layer.style.position = 'absolute';
    layer.style.top = '0';
    layer.style.left = '0';
    layer.style.width = maxWidth + 'px';
    layer.style.height = totalHeight + 'px';
    inner.appendChild(layer);

    pagesContainer.appendChild(inner);
    annotationLayer = layer;

    for (const job of pageRenderJobs) {
      try {
        await job.page.render({
          canvasContext: job.ctx,
          viewport: job.viewport,
          transform: job.transform,
        }).promise;

        const textContentSource = job.page.streamTextContent
          ? job.page.streamTextContent({ includeMarkedContent: true, disableNormalization: true })
          : await job.page.getTextContent();
        const textLayer = new pdfjsLib.TextLayer({
          textContentSource,
          container: job.textLayerDiv,
          viewport: job.viewport,
        });
        await textLayer.render();

        const endOfContent = el('div', { class: 'endOfContent' });
        job.textLayerDiv.appendChild(endOfContent);
        registerTextLayer(job.textLayerDiv, endOfContent);
      } catch (err) {
        console.error('PDF page render failed:', err);
      }
    }
  }

  async function load(path) {
    for (const layer of pagesContainer.querySelectorAll('.pdf-text-layer')) {
      unregisterTextLayer(layer);
    }
    pagesContainer.innerHTML = '';
    annotations = [];
    annotationLayer = null;
    info.textContent = 'Loading PDF...';

    if (pdfDoc) {
      pdfDoc.destroy();
      pdfDoc = null;
    }

    try {
      const lib = await loadPdfjs();
      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }

      pdfDoc = await lib.getDocument({ data: bytes }).promise;
      const totalPages = pdfDoc.numPages;

      info.textContent = `${totalPages} page${totalPages !== 1 ? 's' : ''}  •  ${formatSize(result.size)}`;
      await renderAllPages();
    } catch (e) {
      info.textContent = '';
      pagesContainer.innerHTML = `<div class="preview-error">Could not render PDF: ${e}</div>`;
    }
  }

  function destroy() {
    if (pdfDoc) {
      pdfDoc.destroy();
      pdfDoc = null;
    }
    for (const layer of pagesContainer.querySelectorAll('.pdf-text-layer')) {
      unregisterTextLayer(layer);
    }
    annotations = [];
    annotationLayer = null;
    pagesContainer.innerHTML = '';
    activeTool = null;
    highlightBtn.classList.remove('active');
    textBoxBtn.classList.remove('active');
    deleteBtn.classList.remove('active');
    pagesContainer.classList.remove('pdf-tool-active');
    scrollWrap.style.cursor = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

const textLayerRegistry = new Map();
let globalSelectionListenerInstalled = false;
let isPointerDownGlobal = false;
let prevSelectionRange = null;

function resetTextLayer(layerDiv, endDiv) {
  layerDiv.appendChild(endDiv);
  endDiv.style.width = '';
  endDiv.style.height = '';
  endDiv.style.userSelect = '';
  layerDiv.classList.remove('selecting');
}

function resetAllTextLayers() {
  for (const [layerDiv, endDiv] of textLayerRegistry) {
    resetTextLayer(layerDiv, endDiv);
  }
  prevSelectionRange = null;
}

function ensureGlobalSelectionListener() {
  if (globalSelectionListenerInstalled) return;
  globalSelectionListenerInstalled = true;

  document.addEventListener('pointerdown', () => {
    isPointerDownGlobal = true;
  });
  document.addEventListener('pointerup', () => {
    isPointerDownGlobal = false;
    resetAllTextLayers();
  });
  window.addEventListener('blur', () => {
    isPointerDownGlobal = false;
    resetAllTextLayers();
  });
  document.addEventListener('keyup', () => {
    if (!isPointerDownGlobal) resetAllTextLayers();
  });

  document.addEventListener('selectionchange', () => {
    const selection = document.getSelection();
    if (!selection || selection.rangeCount === 0) {
      resetAllTextLayers();
      return;
    }

    const activeLayers = new Set();
    for (let i = 0; i < selection.rangeCount; i++) {
      const range = selection.getRangeAt(i);
      for (const layerDiv of textLayerRegistry.keys()) {
        if (!activeLayers.has(layerDiv) && range.intersectsNode(layerDiv)) {
          activeLayers.add(layerDiv);
        }
      }
    }

    for (const [layerDiv, endDiv] of textLayerRegistry) {
      if (activeLayers.has(layerDiv)) {
        layerDiv.classList.add('selecting');
      } else {
        resetTextLayer(layerDiv, endDiv);
      }
    }

    if (activeLayers.size === 0) return;

    const range = selection.getRangeAt(0);
    const modifyStart = prevSelectionRange && (
      range.compareBoundaryPoints(Range.END_TO_END, prevSelectionRange) === 0 ||
      range.compareBoundaryPoints(Range.START_TO_END, prevSelectionRange) === 0
    );
    let anchor = modifyStart ? range.startContainer : range.endContainer;
    if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode;
    if (!modifyStart && range.endOffset === 0) {
      try {
        do {
          while (!anchor.previousSibling) anchor = anchor.parentNode;
          anchor = anchor.previousSibling;
        } while (!anchor.childNodes.length);
      } catch {
        prevSelectionRange = range.cloneRange();
        return;
      }
    }

    const parentTextLayer = anchor.parentElement?.closest('.pdf-text-layer');
    const endDiv = parentTextLayer ? textLayerRegistry.get(parentTextLayer) : null;
    if (endDiv && anchor.parentElement) {
      endDiv.style.width = parentTextLayer.style.width || '100%';
      endDiv.style.height = parentTextLayer.style.height || '100%';
      endDiv.style.userSelect = 'text';
      anchor.parentElement.insertBefore(endDiv, modifyStart ? anchor : anchor.nextSibling);
    }

    prevSelectionRange = range.cloneRange();
  });
}

function registerTextLayer(layerDiv, endDiv) {
  textLayerRegistry.set(layerDiv, endDiv);
  ensureGlobalSelectionListener();
}

function unregisterTextLayer(layerDiv) {
  textLayerRegistry.delete(layerDiv);
}
