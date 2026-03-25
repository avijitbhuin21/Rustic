import { el, iconMulti } from '../utils/dom.js';
import { editorStore } from '../state/editor.js';
import { settingsStore } from '../state/settings.js';
import { createEditorPane } from './editor/editor-pane.js';
import { createTabBar } from './editor/tab-bar.js';
import { createSettingsPanel } from './settings/settings-panel.js';

export function createEditorArea() {
  const area = el('div', { class: 'editor-area' });

  const placeholder = el('div', { class: 'editor-placeholder' }, [
    iconMulti([
      'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z',
      'M13 2v7h7',
    ], 48),
    el('span', {}, 'Open a file to start editing'),
  ]);

  const tabBar = createTabBar();
  const editorPane = createEditorPane();

  // Container for tab bar + editor (shown when buffer is active)
  const editorContainer = el('div', { class: 'editor-container' });
  editorContainer.appendChild(tabBar);
  editorContainer.appendChild(editorPane);
  editorContainer.style.display = 'none';

  // Settings panel (shown when settings are open)
  const settingsPanel = createSettingsPanel();
  settingsPanel.style.display = 'none';

  area.appendChild(placeholder);
  area.appendChild(editorContainer);
  area.appendChild(settingsPanel);

  function updateVisibility() {
    const isSettingsOpen = settingsStore.getState('isOpen');
    const bufferId = editorStore.getState('activeBufferId');

    if (isSettingsOpen) {
      placeholder.style.display = 'none';
      editorContainer.style.display = 'none';
      settingsPanel.style.display = 'flex';
    } else if (bufferId) {
      placeholder.style.display = 'none';
      editorContainer.style.display = 'flex';
      settingsPanel.style.display = 'none';
    } else {
      placeholder.style.display = 'flex';
      editorContainer.style.display = 'none';
      settingsPanel.style.display = 'none';
    }
  }

  editorStore.subscribe('activeBufferId', updateVisibility);
  settingsStore.subscribe('isOpen', updateVisibility);
  updateVisibility();

  return area;
}
