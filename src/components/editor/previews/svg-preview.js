import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';
import { sanitizeSvg } from '../../../lib/markdown.js';

/**
 * SVG preview component — renders SVG inline with zoom controls.
 */
export function createSvgPreview() {
  const container = el('div', { class: 'preview-container svg-preview' });
  const toolbar = el('div', { class: 'preview-toolbar' });
  const svgWrap = el('div', { class: 'svg-preview-wrap' });
  const info = el('div', { class: 'preview-info' });

  let currentScale = 1;
  let svgEl = null;

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

  container.appendChild(toolbar);
  container.appendChild(svgWrap);
  container.appendChild(info);

  function setScale(s) {
    currentScale = Math.max(0.1, Math.min(10, s));
    svgWrap.style.transform = `scale(${currentScale})`;
    svgWrap.style.transformOrigin = 'center center';
    zoomLabel.textContent = `${Math.round(currentScale * 100)}%`;
  }

  zoomInBtn.addEventListener('click', () => setScale(currentScale * 1.25));
  zoomOutBtn.addEventListener('click', () => setScale(currentScale / 1.25));
  zoomResetBtn.addEventListener('click', () => setScale(1));

  fitBtn.addEventListener('click', () => {
    if (!svgEl) return;
    const wrapParent = svgWrap.parentElement;
    const parentRect = wrapParent.getBoundingClientRect();
    const svgRect = svgEl.getBoundingClientRect();
    const natW = svgRect.width / currentScale;
    const natH = svgRect.height / currentScale;
    if (!natW || !natH) return;
    const scaleX = (parentRect.width - 40) / natW;
    const scaleY = (parentRect.height - 120) / natH;
    setScale(Math.min(scaleX, scaleY, 1));
  });

  // Mouse wheel zoom
  svgWrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.1 : 0.9;
    setScale(currentScale * factor);
  }, { passive: false });

  async function load(path) {
    svgWrap.innerHTML = '<div class="preview-loading">Loading...</div>';
    info.textContent = '';

    try {
      const text = await api.readFileContent(path);
      svgWrap.innerHTML = '';

      // Sanitise via DOMPurify SVG profile before inserting into the host DOM.
      // Inline <svg> in the page executes scripts at the host origin \u2014 i.e.
      // with __TAURI_IPC__ access \u2014 so an attacker-controlled file would be RCE
      // without this step. See lib/markdown.js sanitizeSvg.
      const parsedSvg = sanitizeSvg(text);

      if (parsedSvg) {
        svgEl = parsedSvg;
        svgWrap.appendChild(svgEl);

        const w = svgEl.getAttribute('width') || svgEl.viewBox?.baseVal?.width || '?';
        const h = svgEl.getAttribute('height') || svgEl.viewBox?.baseVal?.height || '?';
        const sizeBytes = new Blob([text]).size;
        info.textContent = `${w} \u00d7 ${h}  \u2022  ${formatSize(sizeBytes)}`;
      } else {
        svgWrap.replaceChildren(el('div', { class: 'preview-error' }, 'Invalid SVG file'));
      }

      setScale(1);
    } catch (e) {
      svgWrap.replaceChildren(el('div', { class: 'preview-error' }, `Failed to load SVG: ${e}`));
    }
  }

  function destroy() {
    svgWrap.innerHTML = '';
    svgEl = null;
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
