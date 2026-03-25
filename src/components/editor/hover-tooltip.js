import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

/**
 * Hover tooltip overlay for the editor.
 * Shows type info / documentation on mouse hover.
 */
export function createHoverTooltip() {
  const tooltip = el('div', { class: 'hover-tooltip' });
  tooltip.style.display = 'none';

  let hoverTimeout = null;
  let visible = false;

  async function show(bufferId, line, col, x, y) {
    try {
      const result = await api.getHover(bufferId, line, col);
      if (!result || !result.contents) {
        hide();
        return;
      }

      tooltip.innerHTML = '';
      const content = el('div', { class: 'hover-tooltip__content' });
      // Simple markdown-ish rendering
      content.innerHTML = formatHoverContent(result.contents);
      tooltip.appendChild(content);

      tooltip.style.left = `${x}px`;
      tooltip.style.top = `${Math.max(0, y - 4)}px`;
      tooltip.style.transform = 'translateY(-100%)';
      tooltip.style.display = 'block';
      visible = true;
    } catch {
      hide();
    }
  }

  function hide() {
    tooltip.style.display = 'none';
    visible = false;
    if (hoverTimeout) {
      clearTimeout(hoverTimeout);
      hoverTimeout = null;
    }
  }

  function scheduleShow(bufferId, line, col, x, y, delay = 500) {
    hide();
    hoverTimeout = setTimeout(() => show(bufferId, line, col, x, y), delay);
  }

  function cancelSchedule() {
    if (hoverTimeout) {
      clearTimeout(hoverTimeout);
      hoverTimeout = null;
    }
  }

  return { element: tooltip, show, hide, scheduleShow, cancelSchedule, isVisible: () => visible };
}

function formatHoverContent(text) {
  let html = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, '<pre class="hover-code"><code>$2</code></pre>');
  html = html.replace(/`([^`]+)`/g, '<code class="hover-inline-code">$1</code>');
  html = html.replace(/\n/g, '<br>');
  return html;
}
