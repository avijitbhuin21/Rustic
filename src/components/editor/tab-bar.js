import { el } from '../../utils/dom.js';
import { editorStore } from '../../state/editor.js';
import { createTab } from './tab.js';

export function createTabBar() {
  const bar = el('div', { class: 'tab-bar' });

  function render() {
    bar.innerHTML = '';
    const buffers = editorStore.getState('openBuffers');
    const activeId = editorStore.getState('activeBufferId');

    for (const buf of Object.values(buffers)) {
      bar.appendChild(createTab({
        id: buf.id,
        fileName: buf.fileName,
        projectName: buf.projectName,
        isModified: buf.isModified,
        isActive: buf.id === activeId,
      }));
    }
  }

  // Re-render when buffers or active tab change
  editorStore.subscribe('openBuffers', render);
  editorStore.subscribe('activeBufferId', render);

  // Shift+wheel for horizontal scrolling
  bar.addEventListener('wheel', (e) => {
    if (e.deltaY !== 0) {
      e.preventDefault();
      bar.scrollLeft += e.deltaY;
    }
  }, { passive: false });

  // Initial render
  render();

  return bar;
}
