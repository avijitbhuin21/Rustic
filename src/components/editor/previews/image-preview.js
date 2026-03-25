import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';
import { getMimeType } from '../../../utils/file-types.js';

export function createImagePreview() {
  const container = el('div', { class: 'preview-container image-preview' });
  const toolbar = el('div', { class: 'preview-toolbar' });
  const imageWrap = el('div', { class: 'image-preview-wrap' });
  const img = el('img', { class: 'image-preview-img' });
  const info = el('div', { class: 'preview-info' });

  let currentScale = 1;
  let filePath = null;

  // Toolbar buttons
  const zoomInBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom In' }, '+');
  const zoomOutBtn = el('button', { class: 'preview-toolbar-btn', title: 'Zoom Out' }, '\u2212');
  const zoomResetBtn = el('button', { class: 'preview-toolbar-btn', title: 'Reset Zoom' }, '1:1');
  const fitBtn = el('button', { class: 'preview-toolbar-btn', title: 'Fit to Window' }, 'Fit');
  const zoomLabel = el('span', { class: 'preview-toolbar-label' }, '100%');

  toolbar.appendChild(zoomOutBtn);
  toolbar.appendChild(zoomLabel);
  toolbar.appendChild(zoomInBtn);
  toolbar.appendChild(zoomResetBtn);
  toolbar.appendChild(fitBtn);

  imageWrap.appendChild(img);
  container.appendChild(toolbar);
  container.appendChild(imageWrap);
  container.appendChild(info);

  function setScale(s) {
    currentScale = Math.max(0.1, Math.min(10, s));
    img.style.transform = `scale(${currentScale})`;
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
  }

  zoomInBtn.addEventListener('click', () => setScale(currentScale * 1.25));
  zoomOutBtn.addEventListener('click', () => setScale(currentScale / 1.25));
  zoomResetBtn.addEventListener('click', () => setScale(1));

  fitBtn.addEventListener('click', () => {
    const wrapRect = imageWrap.getBoundingClientRect();
    const natW = img.naturalWidth;
    const natH = img.naturalHeight;
    if (!natW || !natH) return;
    const scaleX = (wrapRect.width - 40) / natW;
    const scaleY = (wrapRect.height - 40) / natH;
    setScale(Math.min(scaleX, scaleY, 1));
  });

  // Mouse wheel zoom
  imageWrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.1 : 0.9;
    setScale(currentScale * factor);
  }, { passive: false });

  async function load(path) {
    filePath = path;
    info.textContent = 'Loading...';
    img.src = '';

    try {
      const mime = getMimeType(path);
      const result = await api.readFileBase64(path);
      img.src = `data:${mime};base64,${result.data}`;

      img.onload = () => {
        const sizeInfo = formatSize(result.size);
        info.textContent = `${img.naturalWidth} \u00d7 ${img.naturalHeight}  \u2022  ${sizeInfo}`;
        setScale(1);
        // Auto-fit if image is larger than container
        const wrapRect = imageWrap.getBoundingClientRect();
        if (img.naturalWidth > wrapRect.width - 40 || img.naturalHeight > wrapRect.height - 40) {
          fitBtn.click();
        }
      };
    } catch (e) {
      info.textContent = `Failed to load image: ${e}`;
    }
  }

  function destroy() {
    img.src = '';
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
