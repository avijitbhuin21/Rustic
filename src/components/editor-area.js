import { el, icon, iconMulti } from '../utils/dom.js';
import { editorStore } from '../state/editor.js';
import { workspaceStore } from '../state/workspace.js';
import { settingsStore } from '../state/settings.js';
import { createEditorPane } from './editor/editor-pane.js';
import { createTabBar } from './editor/tab-bar.js';
import { createSettingsPanel } from './settings/settings-panel.js';
import { createFilePreview } from './editor/file-preview.js';

function createBreadcrumb() {
  const bar = el('div', { class: 'breadcrumb-bar' });

  function render() {
    bar.innerHTML = '';
    const activeId = editorStore.getState('activeBufferId');
    if (!activeId) return;

    const buffers = editorStore.getState('openBuffers');
    const buf = buffers[activeId];
    if (!buf) return;

    const filePath = buf.filePath;
    const projectName = buf.projectName;

    // Find project root
    const projects = workspaceStore.getState('projects');
    const project = projects.find(p => p.name === projectName);
    const rootPath = project ? project.root_path : '';

    let relativePath = filePath;
    if (rootPath && filePath.startsWith(rootPath)) {
      relativePath = filePath.substring(rootPath.length).replace(/^[\\/]/, '');
    }

    const segments = relativePath.split(/[\\/]/).filter(Boolean);

    // Project name as first segment
    if (projectName) {
      const seg = el('span', { class: 'breadcrumb-segment breadcrumb-segment--root' }, projectName);
      bar.appendChild(seg);
      if (segments.length > 0) {
        bar.appendChild(createSeparator());
      }
    }

    segments.forEach((name, i) => {
      const isLast = i === segments.length - 1;
      const seg = el('span', {
        class: `breadcrumb-segment${isLast ? ' breadcrumb-segment--active' : ''}`,
      }, name);
      bar.appendChild(seg);
      if (!isLast) {
        bar.appendChild(createSeparator());
      }
    });

    // Show file type badge for preview files
    if (buf.isPreview && buf.fileType) {
      bar.appendChild(createSeparator());
      const badge = el('span', { class: 'breadcrumb-badge' }, buf.fileType.toUpperCase());
      bar.appendChild(badge);
    }
  }

  function createSeparator() {
    const sep = el('span', { class: 'breadcrumb-separator' });
    sep.appendChild(icon('M9 18l6-6-6-6', 10));
    return sep;
  }

  editorStore.subscribe('activeBufferId', render);
  editorStore.subscribe('openBuffers', render);
  render();

  return bar;
}

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
  const breadcrumb = createBreadcrumb();
  const editorPane = createEditorPane();
  const filePreview = createFilePreview();

  // Container for tab bar + breadcrumb + editor/preview (shown when buffer is active)
  const editorContainer = el('div', { class: 'editor-container' });
  editorContainer.appendChild(tabBar);
  editorContainer.appendChild(breadcrumb);
  editorContainer.appendChild(editorPane);
  editorContainer.appendChild(filePreview.element);
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
      filePreview.hide();
    } else if (bufferId) {
      placeholder.style.display = 'none';
      editorContainer.style.display = 'flex';
      settingsPanel.style.display = 'none';

      // Check if this is a preview file or code file
      const buffers = editorStore.getState('openBuffers');
      const buf = buffers[bufferId];

      if (buf && buf.isPreview) {
        // Show preview, hide editor pane
        editorPane.style.display = 'none';
        filePreview.show(buf);
      } else {
        // Show editor pane, hide preview
        editorPane.style.display = 'flex';
        filePreview.hide();
      }
    } else {
      placeholder.style.display = 'flex';
      editorContainer.style.display = 'none';
      settingsPanel.style.display = 'none';
      filePreview.hide();
    }
  }

  editorStore.subscribe('activeBufferId', updateVisibility);
  editorStore.subscribe('openBuffers', updateVisibility);
  settingsStore.subscribe('isOpen', updateVisibility);
  updateVisibility();

  return area;
}
