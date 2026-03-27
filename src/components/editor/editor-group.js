import { el, icon, iconMulti } from '../../utils/dom.js';
import { editorStore, setViewMode, setActiveGroup, closeGroup, splitRight } from '../../state/editor.js';
import { workspaceStore } from '../../state/workspace.js';
import { createEditorPane } from './editor-pane.js';
import { createTabBar } from './tab-bar.js';
import { createFilePreview } from './file-preview.js';
import { createSettingsPanel } from '../settings/settings-panel.js';

function createBreadcrumb(groupId) {
  const bar = el('div', { class: 'breadcrumb-bar' });

  function render() {
    bar.innerHTML = '';
    const groups = editorStore.getState('groups');
    const group = groups.find(g => g.id === groupId);
    if (!group || !group.activeBufferId) return;

    const buffers = editorStore.getState('openBuffers');
    const buf = buffers[group.activeBufferId];
    if (!buf || buf.fileType === 'settings') return;

    const filePath = buf.filePath;
    const projectName = buf.projectName;

    const projects = workspaceStore.getState('projects');
    const project = projects.find(p => p.name === projectName);
    const rootPath = project ? project.root_path : '';

    let relativePath = filePath;
    if (rootPath && filePath.startsWith(rootPath)) {
      relativePath = filePath.substring(rootPath.length).replace(/^[\\/]/, '');
    }

    const segments = relativePath.split(/[\\/]/).filter(Boolean);
    const leftSide = el('div', { class: 'breadcrumb-left' });

    if (projectName) {
      leftSide.appendChild(el('span', { class: 'breadcrumb-segment breadcrumb-segment--root' }, projectName));
      if (segments.length > 0) {
        const sep = el('span', { class: 'breadcrumb-separator' });
        sep.appendChild(icon('M9 18l6-6-6-6', 10));
        leftSide.appendChild(sep);
      }
    }

    segments.forEach((name, i) => {
      const isLast = i === segments.length - 1;
      leftSide.appendChild(el('span', {
        class: `breadcrumb-segment${isLast ? ' breadcrumb-segment--active' : ''}`,
      }, name));
      if (!isLast) {
        const sep = el('span', { class: 'breadcrumb-separator' });
        sep.appendChild(icon('M9 18l6-6-6-6', 10));
        leftSide.appendChild(sep);
      }
    });

    if (buf.isPreview && buf.fileType) {
      const sep = el('span', { class: 'breadcrumb-separator' });
      sep.appendChild(icon('M9 18l6-6-6-6', 10));
      leftSide.appendChild(sep);
      leftSide.appendChild(el('span', { class: 'breadcrumb-badge' }, buf.fileType.toUpperCase()));
    }

    bar.appendChild(leftSide);

    if (buf.isDualMode) {
      const toggle = el('div', { class: 'view-mode-toggle' });
      const editBtn = el('button', {
        class: `view-mode-btn${buf.viewMode === 'edit' ? ' view-mode-btn--active' : ''}`,
        title: 'Edit source',
      });
      editBtn.appendChild(icon('M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z', 12));
      editBtn.appendChild(document.createTextNode(' Edit'));

      const previewBtn = el('button', {
        class: `view-mode-btn${buf.viewMode === 'preview' ? ' view-mode-btn--active' : ''}`,
        title: 'Preview rendered',
      });
      previewBtn.appendChild(iconMulti(['M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z', 'M15 12a3 3 0 1 1-6 0 3 3 0 1 1 6 0z'], 12));
      previewBtn.appendChild(document.createTextNode(' Preview'));

      editBtn.addEventListener('click', () => { if (buf.viewMode !== 'edit') setViewMode(buf.id, 'edit'); });
      previewBtn.addEventListener('click', () => { if (buf.viewMode !== 'preview') setViewMode(buf.id, 'preview'); });
      toggle.appendChild(editBtn);
      toggle.appendChild(previewBtn);
      bar.appendChild(toggle);
    }
  }

  editorStore.subscribe('groups', render);
  editorStore.subscribe('openBuffers', render);
  render();

  return bar;
}

/**
 * Create a single editor group (one pane in the split layout).
 * Each group has its own tab bar, breadcrumb, editor pane, and preview.
 */
export function createEditorGroup(groupId) {
  const container = el('div', { class: 'editor-group' });

  const tabBar = createTabBar(groupId);
  const breadcrumb = createBreadcrumb(groupId);
  const editorPane = createEditorPane(groupId);
  const filePreview = createFilePreview();

  const settingsPanel = createSettingsPanel();
  settingsPanel.style.display = 'none';

  container.appendChild(tabBar);
  container.appendChild(breadcrumb);
  container.appendChild(editorPane);
  container.appendChild(filePreview.element);
  container.appendChild(settingsPanel);

  // Focus this group when clicked
  container.addEventListener('mousedown', () => {
    if (editorStore.getState('activeGroupId') !== groupId) {
      setActiveGroup(groupId);
    }
  });

  function updateVisibility() {
    const groups = editorStore.getState('groups');
    const group = groups.find(g => g.id === groupId);
    if (!group) return;

    const buffers = editorStore.getState('openBuffers');
    const buf = group.activeBufferId != null ? buffers[group.activeBufferId] : null;

    // Show active group highlight
    const isActive = editorStore.getState('activeGroupId') === groupId;
    container.classList.toggle('editor-group--active', isActive);

    if (buf && buf.fileType === 'settings') {
      editorPane.style.display = 'none';
      settingsPanel.style.display = 'flex';
      filePreview.hide();
    } else if (buf) {
      settingsPanel.style.display = 'none';
      if (buf.isDualMode && buf.viewMode === 'preview') {
        editorPane.style.display = 'none';
        filePreview.show(buf);
      } else if (buf.isPreview) {
        editorPane.style.display = 'none';
        filePreview.show(buf);
      } else {
        editorPane.style.display = 'flex';
        filePreview.hide();
      }
    } else {
      editorPane.style.display = 'none';
      settingsPanel.style.display = 'none';
      filePreview.hide();
    }
  }

  editorStore.subscribe('groups', updateVisibility);
  editorStore.subscribe('openBuffers', updateVisibility);
  editorStore.subscribe('activeGroupId', updateVisibility);
  updateVisibility();

  return { element: container, groupId };
}
