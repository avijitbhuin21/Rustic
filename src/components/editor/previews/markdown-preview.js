import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';

/**
 * Markdown preview component using marked.
 */
export function createMarkdownPreview() {
  const container = el('div', { class: 'preview-container markdown-preview' });
  const content = el('div', { class: 'markdown-preview-content' });

  container.appendChild(content);

  let markedLib = null;

  async function loadMarked() {
    if (markedLib) return markedLib;
    const mod = await import('marked');
    markedLib = mod.marked;
    // Configure marked for safety
    markedLib.setOptions({
      breaks: true,
      gfm: true,
    });
    return markedLib;
  }

  async function load(path) {
    content.innerHTML = '<div class="preview-loading">Loading...</div>';

    try {
      const marked = await loadMarked();
      const text = await api.readFileContent(path);
      content.innerHTML = marked(text);

      // Make links open externally
      content.querySelectorAll('a').forEach(a => {
        a.setAttribute('target', '_blank');
        a.setAttribute('rel', 'noopener noreferrer');
      });

      // Add syntax highlighting classes to code blocks
      content.querySelectorAll('pre code').forEach(block => {
        block.classList.add('markdown-code-block');
      });
    } catch (e) {
      content.innerHTML = `<div class="preview-error">Failed to render markdown: ${e}</div>`;
    }
  }

  function destroy() {
    content.innerHTML = '';
  }

  return { element: container, load, destroy };
}
