import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * PDF preview component using PDF.js
 */
export function createPdfPreview() {
  const container = el('div', { class: 'preview-container pdf-preview' });
  const toolbar = el('div', { class: 'preview-toolbar' });
  const pagesContainer = el('div', { class: 'pdf-pages-container' });
  const info = el('div', { class: 'preview-info' });

  let pdfDoc = null;
  let currentPage = 1;
  let totalPages = 0;
  let currentScale = 1.5;
  let pdfjsLib = null;

  // Toolbar
  const prevBtn = el('button', { class: 'preview-toolbar-btn', title: 'Previous Page' }, '\u25c0');
  const nextBtn = el('button', { class: 'preview-toolbar-btn', title: 'Next Page' }, '\u25b6');
  const pageLabel = el('span', { class: 'preview-toolbar-label' }, 'Page 0 / 0');
  const zoomOutBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom Out' }, '\u2212');
  const zoomInBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom In' }, '+');
  const zoomLabel = el('span', { class: 'preview-toolbar-label' }, '150%');

  toolbar.appendChild(prevBtn);
  toolbar.appendChild(pageLabel);
  toolbar.appendChild(nextBtn);
  toolbar.appendChild(el('span', { class: 'preview-toolbar-separator' }, '|'));
  toolbar.appendChild(zoomOutBtn);
  toolbar.appendChild(zoomLabel);
  toolbar.appendChild(zoomInBtn);

  container.appendChild(toolbar);
  container.appendChild(pagesContainer);
  container.appendChild(info);

  prevBtn.addEventListener('click', () => {
    if (currentPage > 1) {
      currentPage--;
      renderPage(currentPage);
    }
  });

  nextBtn.addEventListener('click', () => {
    if (currentPage < totalPages) {
      currentPage++;
      renderPage(currentPage);
    }
  });

  zoomInBtn.addEventListener('click', () => {
    currentScale = Math.min(5, currentScale + 0.25);
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
    renderPage(currentPage);
  });

  zoomOutBtn.addEventListener('click', () => {
    currentScale = Math.max(0.25, currentScale - 0.25);
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
    renderPage(currentPage);
  });

  async function loadPdfjs() {
    if (pdfjsLib) return pdfjsLib;
    pdfjsLib = await import('pdfjs-dist');
    // Set worker source
    pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
      'pdfjs-dist/build/pdf.worker.mjs',
      import.meta.url
    ).toString();
    return pdfjsLib;
  }

  async function renderPage(pageNum) {
    if (!pdfDoc) return;

    const page = await pdfDoc.getPage(pageNum);
    const viewport = page.getViewport({ scale: currentScale });

    pagesContainer.innerHTML = '';
    const canvas = el('canvas', { class: 'pdf-page-canvas' });
    canvas.width = viewport.width;
    canvas.height = viewport.height;
    pagesContainer.appendChild(canvas);

    const ctx = canvas.getContext('2d');
    await page.render({ canvasContext: ctx, viewport }).promise;

    pageLabel.textContent = `Page ${pageNum} / ${totalPages}`;
    prevBtn.disabled = pageNum <= 1;
    nextBtn.disabled = pageNum >= totalPages;
  }

  async function load(path) {
    pagesContainer.innerHTML = '';
    info.textContent = 'Loading PDF...';
    pageLabel.textContent = 'Loading...';

    try {
      const lib = await loadPdfjs();
      const result = await api.readFileBase64(path);
      const binaryStr = atob(result.data);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }

      pdfDoc = await lib.getDocument({ data: bytes }).promise;
      totalPages = pdfDoc.numPages;
      currentPage = 1;

      info.textContent = `${totalPages} page${totalPages !== 1 ? 's' : ''}  \u2022  ${formatSize(result.size)}`;
      await renderPage(1);
    } catch (e) {
      info.textContent = `Failed to load PDF: ${e}`;
      pagesContainer.innerHTML = `<div class="preview-error">Could not render PDF</div>`;
    }
  }

  function destroy() {
    if (pdfDoc) {
      pdfDoc.destroy();
      pdfDoc = null;
    }
    pagesContainer.innerHTML = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
