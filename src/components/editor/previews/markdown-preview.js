import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * Markdown preview component using the sanitized markdown helper.
 */
export function createMarkdownPreview() {
  const container = el('div', { class: 'preview-container markdown-preview' });
  const content = el('div', { class: 'markdown-preview-content' });

  container.appendChild(content);

  let renderer = null;
  async function loadRenderer() {
    if (renderer) return renderer;
    const mod = await import('../../../lib/markdown.js');
    mod.marked.setOptions({ breaks: true, gfm: true });
    renderer = mod.renderMarkdown;
    return renderer;
  }

  async function load(path) {
    content.innerHTML = '<div class="preview-loading">Loading...</div>';

    try {
      const render = await loadRenderer();
      const text = await api.readFileContent(path);
      content.innerHTML = render(text);

      content.querySelectorAll('a').forEach(a => {
        a.setAttribute('target', '_blank');
        a.setAttribute('rel', 'noopener noreferrer');
      });

      content.querySelectorAll('pre code').forEach(block => {
        block.classList.add('markdown-code-block');
      });
    } catch (e) {
      content.textContent = `Failed to render markdown: ${e}`;
    }
  }

  function destroy() {
    content.innerHTML = '';
  }

  return { element: container, load, destroy };
}
