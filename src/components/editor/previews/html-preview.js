import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * HTML preview component — renders HTML in a sandboxed iframe.
 */
export function createHtmlPreview() {
  const container = el('div', { class: 'preview-container html-preview' });
  const iframe = el('iframe', {
    class: 'html-preview-iframe',
    sandbox: 'allow-scripts',
  });

  container.appendChild(iframe);

  async function load(path) {
    container.innerHTML = '';
    const loading = el('div', { class: 'preview-loading' }, 'Loading...');
    container.appendChild(loading);

    try {
      const text = await api.readFileContent(path);
      container.innerHTML = '';
      container.appendChild(iframe);

      // Use srcdoc to set content without cross-origin document access
      iframe.srcdoc = text;
    } catch (e) {
      container.innerHTML = '';
      container.appendChild(
        el('div', { class: 'preview-error' }, `Failed to render HTML: ${e}`)
      );
    }
  }

  function destroy() {
    iframe.srcdoc = '';
    container.innerHTML = '';
  }

  return { element: container, load, destroy };
}
