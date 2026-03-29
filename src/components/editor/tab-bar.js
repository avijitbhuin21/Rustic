import { el, icon } from '../../utils/dom.js';
import { editorStore, splitRight } from '../../state/editor.js';
import { createTab } from './tab.js';

/** Smooth momentum scroller. Returns a function to push delta px onto the element. */
function makeSmoothScroller(el) {
  let target = 0;
  let rafId = null;

  function animate() {
    const dist = target - el.scrollLeft;
    if (Math.abs(dist) < 0.5) {
      el.scrollLeft = target;
      rafId = null;
      return;
    }
    el.scrollLeft += dist * 0.14;
    rafId = requestAnimationFrame(animate);
  }

  return function push(delta) {
    target = Math.max(0, Math.min(el.scrollWidth - el.clientWidth, target + delta));
    if (!rafId) rafId = requestAnimationFrame(animate);
  };
}

export function createTabBar(groupId) {
  const bar = el('div', { class: 'tab-bar' });

  // Scrollable tabs area — created once so the wheel listener persists
  const tabsArea = el('div', { class: 'tab-bar__tabs' });
  const scrollTabs = makeSmoothScroller(tabsArea);
  tabsArea.addEventListener('wheel', (e) => {
    if (e.deltaY !== 0) {
      e.preventDefault();
      scrollTabs(e.deltaY * 0.5);
    }
  }, { passive: false });
  bar.appendChild(tabsArea);

  // Actions area (split button) — stays fixed on the right
  const actions = el('div', { class: 'tab-bar__actions' });
  const splitBtn = el('button', { class: 'tab-bar__action', title: 'Split Editor Right' });
  splitBtn.appendChild(icon('M9 3H5a2 2 0 00-2 2v14a2 2 0 002 2h4M15 3h4a2 2 0 012 2v14a2 2 0 01-2 2h-4M12 3v18', 14));
  splitBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    splitRight();
  });
  actions.appendChild(splitBtn);
  bar.appendChild(actions);

  function render() {
    tabsArea.innerHTML = '';
    const buffers = editorStore.getState('openBuffers');
    const groups = editorStore.getState('groups');
    const group = groups.find(g => g.id === groupId);

    if (!group) return;

    for (const bufId of group.bufferIds) {
      const buf = buffers[bufId];
      if (!buf) continue;
      tabsArea.appendChild(createTab({
        id: buf.id,
        fileName: buf.fileName,
        projectName: buf.projectName,
        isModified: buf.isModified,
        isActive: buf.id === group.activeBufferId,
        groupId,
      }));
    }

    // Scroll the active tab into view
    const activeTab = tabsArea.querySelector('.tab--active');
    if (activeTab) {
      requestAnimationFrame(() => activeTab.scrollIntoView({ block: 'nearest', inline: 'nearest' }));
    }
  }

  editorStore.subscribe('openBuffers', render);
  editorStore.subscribe('groups', render);

  render();
  return bar;
}
