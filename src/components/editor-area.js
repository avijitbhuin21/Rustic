import { el, icon, iconMulti } from '../utils/dom.js';
import { editorStore, setViewMode } from '../state/editor.js';
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

    // Left side: breadcrumb path
    const leftSide = el('div', { class: 'breadcrumb-left' });

    // Project name as first segment
    if (projectName) {
      const seg = el('span', { class: 'breadcrumb-segment breadcrumb-segment--root' }, projectName);
      leftSide.appendChild(seg);
      if (segments.length > 0) {
        leftSide.appendChild(createSeparator());
      }
    }

    segments.forEach((name, i) => {
      const isLast = i === segments.length - 1;
      const seg = el('span', {
        class: `breadcrumb-segment${isLast ? ' breadcrumb-segment--active' : ''}`,
      }, name);
      leftSide.appendChild(seg);
      if (!isLast) {
        leftSide.appendChild(createSeparator());
      }
    });

    // Show file type badge for preview-only files
    if (buf.isPreview && buf.fileType) {
      leftSide.appendChild(createSeparator());
      const badge = el('span', { class: 'breadcrumb-badge' }, buf.fileType.toUpperCase());
      leftSide.appendChild(badge);
    }

    bar.appendChild(leftSide);

    // Right side: Edit/Preview toggle for dual-mode files
    if (buf.isDualMode) {
      const toggle = createViewToggle(buf);
      bar.appendChild(toggle);
    }
  }

  function createViewToggle(buf) {
    const toggle = el('div', { class: 'view-mode-toggle' });

    const editBtn = el('button', {
      class: `view-mode-btn${buf.viewMode === 'edit' ? ' view-mode-btn--active' : ''}`,
      title: 'Edit source',
    });
    // Pencil icon
    editBtn.appendChild(icon('M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z', 12));
    editBtn.appendChild(document.createTextNode(' Edit'));

    const previewBtn = el('button', {
      class: `view-mode-btn${buf.viewMode === 'preview' ? ' view-mode-btn--active' : ''}`,
      title: 'Preview rendered',
    });
    // Eye icon (outline + pupil)
    previewBtn.appendChild(iconMulti(['M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z', 'M15 12a3 3 0 1 1-6 0 3 3 0 1 1 6 0z'], 12));
    previewBtn.appendChild(document.createTextNode(' Preview'));

    editBtn.addEventListener('click', () => {
      if (buf.viewMode !== 'edit') {
        setViewMode(buf.id, 'edit');
      }
    });

    previewBtn.addEventListener('click', () => {
      if (buf.viewMode !== 'preview') {
        setViewMode(buf.id, 'preview');
      }
    });

    toggle.appendChild(editBtn);
    toggle.appendChild(previewBtn);
    return toggle;
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

      const buffers = editorStore.getState('openBuffers');
      const buf = buffers[bufferId];

      if (buf && buf.isDualMode) {
        // Dual-mode file — switch based on viewMode
        if (buf.viewMode === 'preview') {
          editorPane.style.display = 'none';
          filePreview.show(buf);
        } else {
          editorPane.style.display = 'flex';
          filePreview.hide();
        }
      } else if (buf && buf.isPreview) {
        // Preview-only file
        editorPane.style.display = 'none';
        filePreview.show(buf);
      } else {
        // Code file — edit only
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
