import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

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
  let currentScale = 1.5;
  let activeTool = null;
  let annotations = [];
  let annotationLayer = null;
  let isDragging = false;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragRect = null;

  const zoomOutBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom Out' }, '\u2212');
  const zoomLabel = el('span', { class: 'preview-toolbar-label' }, '150%');
  const zoomInBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom In' }, '+');
  const sep1 = el('span', { class: 'preview-toolbar-separator' }, '|');
  const highlightBtn = el('button', { class: 'preview-toolbar-btn', title: 'Highlight' }, '\u25ae Highlight');
  const textBoxBtn = el('button', { class: 'preview-toolbar-btn', title: 'Text Box' }, '\u270d Text Box');
  const deleteBtn = el('button', { class: 'preview-toolbar-btn', title: 'Delete Annotation' }, '\u2715 Delete');

  toolbar.appendChild(zoomOutBtn);
  toolbar.appendChild(zoomLabel);
  toolbar.appendChild(zoomInBtn);
  toolbar.appendChild(sep1);
  toolbar.appendChild(highlightBtn);
  toolbar.appendChild(textBoxBtn);
  toolbar.appendChild(deleteBtn);

  function setActiveTool(tool) {
    activeTool = activeTool === tool ? null : tool;
    highlightBtn.classList.toggle('active', activeTool === 'highlight');
    textBoxBtn.classList.toggle('active', activeTool === 'textbox');
    deleteBtn.classList.toggle('active', activeTool === 'delete');
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
      dragStartX = e.clientX - layerRect.left + annotationLayer.scrollLeft;
      dragStartY = e.clientY - layerRect.top + scrollWrap.scrollTop;

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
      const x = e.clientX - layerRect.left + annotationLayer.scrollLeft;
      const y = e.clientY - layerRect.top + scrollWrap.scrollTop;
      placeTextBox(x, y);
    }
  });

  scrollWrap.addEventListener('mousemove', (e) => {
    if (!isDragging || activeTool !== 'highlight' || !dragRect || !annotationLayer) return;
    const layerRect = annotationLayer.getBoundingClientRect();
    const currentX = e.clientX - layerRect.left + annotationLayer.scrollLeft;
    const currentY = e.clientY - layerRect.top + scrollWrap.scrollTop;
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
    const currentX = e.clientX - layerRect.left + annotationLayer.scrollLeft;
    const currentY = e.clientY - layerRect.top + scrollWrap.scrollTop;
    const left = Math.min(dragStartX, currentX);
    const top = Math.min(dragStartY, currentY);
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
    pagesContainer.innerHTML = '';
    annotations = [];
    annotationLayer = null;

    const pageGap = 12;
    let totalHeight = 0;
    let maxWidth = 0;
    const canvases = [];

    for (let pageNum = 1; pageNum <= totalPages; pageNum++) {
      const page = await pdfDoc.getPage(pageNum);
      const viewport = page.getViewport({ scale: currentScale });

      const canvas = el('canvas', { class: 'pdf-page-canvas' });
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      canvas.style.position = 'absolute';
      canvas.style.left = '0';
      canvas.style.top = totalHeight + 'px';

      if (viewport.width > maxWidth) maxWidth = viewport.width;

      const ctx = canvas.getContext('2d');
      await page.render({ canvasContext: ctx, viewport }).promise;

      canvases.push(canvas);
      totalHeight += viewport.height + pageGap;
    }

    totalHeight = Math.max(0, totalHeight - pageGap);

    const inner = el('div', { class: 'pdf-pages-inner' });
    inner.style.position = 'relative';
    inner.style.width = maxWidth + 'px';
    inner.style.height = totalHeight + 'px';

    for (const canvas of canvases) {
      inner.appendChild(canvas);
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
  }

  async function load(path) {
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

      info.textContent = `${totalPages} page${totalPages !== 1 ? 's' : ''}  \u2022  ${formatSize(result.size)}`;
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
    annotations = [];
    annotationLayer = null;
    pagesContainer.innerHTML = '';
    activeTool = null;
    highlightBtn.classList.remove('active');
    textBoxBtn.classList.remove('active');
    deleteBtn.classList.remove('active');
    scrollWrap.style.cursor = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
