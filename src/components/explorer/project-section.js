import { el, icon, iconMulti, onDetached } from '../../utils/dom.js';
import { workspaceStore, toggleProject, removeProject, refreshProject, refreshAffectedDirectory, clearChildrenCache, loadChildren } from '../../state/workspace.js';
import { createFileTree } from './file-tree.js';
import { insertInlineInput, INDENT_PX } from './file-tree-item.js';
import { createTerminal } from '../../state/terminal.js';
import * as api from '../../lib/tauri-api.js';
import { showContextMenu } from '../dropdown-menu.js';
import { showConfirmDialog } from '../confirm-dialog.js';

async function confirmAndRemoveProject(project) {
  const ok = await showConfirmDialog(
    'Remove project?',
    `${project.name || project.root_path} will be removed from the workspace. ` +
    `Files on disk are not deleted, but any tasks and terminal ` +
    `sessions tied to this project will be cleared.`,
    { confirmLabel: 'Remove', cancelLabel: 'Keep', danger: true },
  );
  if (ok) removeProject(project.id);
}
import {
  pasteIntoDir as clipPasteIntoDir,
  hasClipboard as clipHasClipboard,
} from '../../state/explorer-clipboard.js';
import { debug } from '../../lib/log.js';


export function createProjectSection(project) {
  const section = el('div', { class: 'project-section', dataset: { projectId: String(project.id) } });

  function handleFileTreeRefresh(e) {
    const { projectPath } = e.detail || {};
    if (!projectPath) return;
    const normalize = (p) => p.replace(/\\/g, '/');
    if (normalize(projectPath) !== normalize(project.root_path)) return;
    debug('FileTree', 'handleFileTreeRefresh FULL', { project: project.name, sectionInDOM: document.body.contains(section) });
    const oldTree = section.querySelector(':scope > .file-tree');
    if (!oldTree) return;
    const newTree = createFileTree(project.root_path, 0, project.name);
    oldTree.replaceWith(newTree);
  }

  /**
   * Targeted refresh: only re-render a single directory's children in-place,
   * leaving the rest of the tree untouched.
   */
  function handleDirRefresh(e) {
    const { dirPath, projectPath } = e.detail || {};
    if (!dirPath || !projectPath) return;
    const normalize = (p) => p.replace(/\\/g, '/');
    if (normalize(projectPath) !== normalize(project.root_path)) return;

    const normDir = normalize(dirPath);
    const normRoot = normalize(project.root_path);

    debug('FileTree', 'handleDirRefresh', { dirPath, project: project.name, sectionInDOM: document.body.contains(section) });

    if (normDir === normRoot) {
      // The changed dir IS the project root — re-render the root file-tree
      debug('FileTree', 'handleDirRefresh ROOT refresh');
      const oldTree = section.querySelector(':scope > .file-tree');
      if (!oldTree) return;
      const newTree = createFileTree(project.root_path, 0, project.name);
      oldTree.replaceWith(newTree);
      return;
    }

    // Find the directory's wrapper element in the DOM
    // data-path may use backslashes (Windows) while dirPath uses forward slashes
    let wrapper = section.querySelector(
      `.file-tree-item-wrapper[data-path="${CSS.escape(dirPath)}"]`
    );
    if (!wrapper) {
      const backslashPath = dirPath.replace(/\//g, '\\');
      wrapper = section.querySelector(
        `.file-tree-item-wrapper[data-path="${CSS.escape(backslashPath)}"]`
      );
    }
    if (!wrapper) return;

    // Compute depth from the item's padding
    const item = wrapper.querySelector('.file-tree-item');
    const paddingPx = parseInt(item?.style.paddingLeft) || INDENT_PX;
    const depth = Math.round(paddingPx / INDENT_PX) - 1;

    // Replace only this directory's child tree (only if expanded)
    const oldSubTree = wrapper.querySelector(':scope > .file-tree');
    if (!oldSubTree) return; // dir not expanded — nothing to update visually
    const newSubTree = createFileTree(dirPath, depth + 1, project.name);
    oldSubTree.replaceWith(newSubTree);
  }

  window.addEventListener('rustic:file-tree-refresh', handleFileTreeRefresh);
  window.addEventListener('rustic:file-tree-dir-refresh', handleDirRefresh);

  onDetached(section, () => {
    window.removeEventListener('rustic:file-tree-refresh', handleFileTreeRefresh);
    window.removeEventListener('rustic:file-tree-dir-refresh', handleDirRefresh);
  });

  // Header
  const header = el('div', { class: 'project-section__header' });

  const caretIcon = icon(
    project.isExpanded ? 'M6 9l6 6 6-6' : 'M9 18l6-6-6-6',
    12,
  );
  const caret = el('span', { class: 'project-section__caret' }, caretIcon);

  const nameEl = el('span', { class: 'project-section__name' }, project.name);

  // M2.3: symbol-index status pill. Hidden by default; shows a small
  // spinner while the index is warming up and disappears once ready.
  // Subscribed via the workspaceStore subscription below so external
  // status changes (build completes, file watcher refreshes) update
  // in place without re-rendering the whole project row.
  const indexPill = el('span', {
    class: 'project-section__index-pill',
    style: 'display: none;',
    title: 'Symbol index status',
  });

  const headerLeft = el('div', {
    class: 'project-section__header-left',
    onClick: () => toggleProject(project.id),
  }, [caret, nameEl, indexPill]);

  // Apply the current status immediately + subscribe to changes. The
  // store key is `indexStatus[project.id]`. Buildings show a tiny
  // spinner; failed shows a red dot with a tooltip; ready/missing hide
  // the pill entirely.
  function paintIndexStatus(status) {
    if (!indexPill) return;
    indexPill.replaceChildren();
    switch (status) {
      case 'building':
        indexPill.appendChild(el('span', { class: 'project-section__index-spinner' }));
        indexPill.title = 'Indexing project for find_symbol / outline / call_sites…';
        indexPill.style.display = '';
        break;
      case 'failed':
        indexPill.textContent = '✕';
        indexPill.title = 'Symbol-index build failed — find_symbol and friends will return partial results.';
        indexPill.style.display = '';
        break;
      default:
        // not_started / ready / undefined → hide.
        indexPill.style.display = 'none';
    }
  }
  paintIndexStatus(
    (workspaceStore.getState('indexStatus') || {})[String(project.id)],
  );
  const unsubIndexStatus = workspaceStore.subscribe('indexStatus', (next) => {
    paintIndexStatus((next || {})[String(project.id)]);
  });
  // Best-effort cleanup: when the section is removed from the DOM
  // (project closed / re-rendered), drop the subscription so the
  // closure doesn't pin the project row forever.
  if (typeof window !== 'undefined' && 'MutationObserver' in window) {
    queueMicrotask(() => {
      const root = section;
      if (!root || !root.parentNode) return;
      const obs = new MutationObserver(() => {
        if (!document.body.contains(root)) {
          unsubIndexStatus && unsubIndexStatus();
          obs.disconnect();
        }
      });
      obs.observe(document.body, { childList: true, subtree: true });
    });
  }

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
    createActionBtn('Refresh', 'M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15', async (e) => {
      const btn = e.currentTarget;
      btn.classList.add('spinning');
      const minSpin = new Promise(r => setTimeout(r, 600));
      await Promise.all([refreshProject(project.root_path), minSpin]);
      btn.classList.remove('spinning');
    }),
    createActionBtn('Remove Project', 'M18 6L6 18M6 6l12 12', () => {
      confirmAndRemoveProject(project);
    }),
  ]);

  header.appendChild(headerLeft);
  header.appendChild(actions);
  section.appendChild(header);

  // Right-click on the project header (or the empty file-tree area below it)
  // → menu with paste-into-root, new file/folder, refresh, etc. Mirrors what
  // a user would expect from VS Code's "Folder context menu".
  section.addEventListener('contextmenu', (e) => {
    // Only handle the event if it didn't originate from a child file-tree
    // item — those have their own context menus and call stopPropagation.
    if (e.target.closest('.file-tree-item')) return;
    e.preventDefault();
    e.stopPropagation();

    const menuItems = [
      {
        label: 'New File...',
        action: () => startProjectInlineCreate(project, section, false),
      },
      {
        label: 'New Folder...',
        action: () => startProjectInlineCreate(project, section, true),
      },
      { separator: true },
      {
        label: 'Paste',
        shortcut: 'Ctrl+V',
        // Always enabled — clipPasteIntoDir falls back to OS clipboard
        // paths if the internal explorer clipboard is empty.
        action: async () => {
          debug('project-section', 'paste', { root: project.root_path, internalClipEmpty: !clipHasClipboard() });
          const created = await clipPasteIntoDir(project.root_path);
          debug('project-section', 'paste created', { count: created.length, items: created });
          // Trigger a parent-dir refresh against an arbitrary child path so
          // the project root gets re-rendered.
          await refreshAffectedDirectory(project.root_path + '/.x');
        },
      },

      { separator: true },
      {
        label: 'Refresh Folder',
        action: () => refreshProject(project.root_path),
      },
      {
        label: 'Reveal in File Manager',
        action: () => {
          api.revealInFileManager(project.root_path)
            .catch((err) => console.error('Reveal failed:', err));
        },
      },
      { separator: true },
      {
        label: 'Open Terminal Here',
        action: () => createTerminal(project.root_path, project.name),
      },
      { separator: true },
      {
        label: 'Remove Project',
        action: () => confirmAndRemoveProject(project),
      },
    ];

    showContextMenu(menuItems, e.clientX, e.clientY);
  });

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
  let fileTree = targetSection.querySelector(':scope > .file-tree');

  // If still no file tree (shouldn't happen), bail
  if (!fileTree) return;

  // Wait for any pending renderItems to complete
  await new Promise(r => setTimeout(r, 0));

  // Re-query in case renderItems replaced content
  fileTree = targetSection.querySelector(':scope > .file-tree');
  if (!fileTree) return;

  insertInlineInput(fileTree, 0, isFolder, async (name) => {
    try {
      let createdPath = null;
      if (isFolder) {
        await api.createFolder(project.root_path, name);
      } else {
        createdPath = await api.createFile(project.root_path, name);
      }

      // Rebuild tree BEFORE opening the file so reveal finds the new entry
      clearChildrenCache(project.root_path);
      await loadChildren(project.root_path);
      const currentSection = document.querySelector(`[data-project-id="${project.id}"]`);
      if (currentSection) {
        const oldTree = currentSection.querySelector(':scope > .file-tree');
        if (oldTree) {
          const newTree = createFileTree(project.root_path, 0, project.name);
          oldTree.replaceWith(newTree);
        }
      }

      // Open the file AFTER tree is rebuilt so auto-reveal works correctly
      if (createdPath) {
        window.dispatchEvent(new CustomEvent('rustic:open-file', {
          detail: { path: createdPath, name, projectName: project.name },
        }));
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
