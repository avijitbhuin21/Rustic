import { el, icon, iconMulti } from '../../utils/dom.js';
import { toggleProject, removeProject, refreshProject, clearChildrenCache, loadChildren } from '../../state/workspace.js';
import { createFileTree } from './file-tree.js';
import { insertInlineInput } from './file-tree-item.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';

export function createProjectSection(project) {
  const section = el('div', { class: 'project-section', dataset: { projectId: String(project.id) } });

  // Header
  const header = el('div', { class: 'project-section__header' });

  const caretIcon = icon(
    project.isExpanded ? 'M6 9l6 6 6-6' : 'M9 18l6-6-6-6',
    12,
  );
  const caret = el('span', { class: 'project-section__caret' }, caretIcon);

  const nameEl = el('span', { class: 'project-section__name' }, project.name);

  const headerLeft = el('div', {
    class: 'project-section__header-left',
    onClick: () => toggleProject(project.id),
  }, [caret, nameEl]);

  // Action buttons (visible on hover)
  const actions = el('div', { class: 'project-section__actions' }, [
    createActionBtn('New File', 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M12 18v-6M9 15h6', async () => {
      await startProjectInlineCreate(project, section, false);
    }),
    createActionBtn('New Folder', 'M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM12 11v6M9 14h6', async () => {
      await startProjectInlineCreate(project, section, true);
    }),
    createActionBtn('New Terminal', 'M4 17l6-6-6-6M12 19h8', () => {
      createTerminal(project.root_path, project.name);
    }),
    createActionBtn('Refresh', 'M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15', () => {
      refreshProject(project.root_path);
    }),
    createActionBtn('Remove Project', 'M18 6L6 18M6 6l12 12', () => {
      removeProject(project.id);
    }),
  ]);

  header.appendChild(headerLeft);
  header.appendChild(actions);
  section.appendChild(header);

  // File tree (if expanded)
  if (project.isExpanded) {
    const tree = createFileTree(project.root_path, 0, project.name);
    section.appendChild(tree);
  }

  return section;
}

async function startProjectInlineCreate(project, section, isFolder) {
  // Ensure project is expanded
  if (!project.isExpanded) {
    toggleProject(project.id);
  }

  // Ensure children are loaded
  await loadChildren(project.root_path);

  // Find the current section (may have been re-rendered)
  const targetSection = document.querySelector(`[data-project-id="${project.id}"]`) || section;
  let fileTree = targetSection.querySelector('.file-tree');

  // If still no file tree (shouldn't happen), bail
  if (!fileTree) return;

  // Wait for any pending renderItems to complete
  await new Promise(r => setTimeout(r, 0));

  // Re-query in case renderItems replaced content
  fileTree = targetSection.querySelector('.file-tree');
  if (!fileTree) return;

  insertInlineInput(fileTree, 0, isFolder, async (name) => {
    try {
      if (isFolder) {
        await api.createFolder(project.root_path, name);
      } else {
        const fullPath = await api.createFile(project.root_path, name);
        if (fullPath) {
          window.dispatchEvent(new CustomEvent('rustic:open-file', {
            detail: { path: fullPath, name, projectName: project.name },
          }));
        }
      }
      clearChildrenCache(project.root_path);
      await loadChildren(project.root_path);
      // Refresh the file tree
      const currentSection = document.querySelector(`[data-project-id="${project.id}"]`);
      if (currentSection) {
        const oldTree = currentSection.querySelector('.file-tree');
        if (oldTree) {
          const newTree = createFileTree(project.root_path, 0, project.name);
          oldTree.replaceWith(newTree);
        }
      }
    } catch (e) {
      console.error('Failed to create:', e);
    }
  });
}

function createActionBtn(title, iconPath, onClick) {
  const btn = el('button', {
    class: 'project-section__action-btn',
    title,
    onClick,
  });
  btn.appendChild(icon(iconPath, 14));
  return btn;
}
