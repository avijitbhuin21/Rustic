import { el, icon } from '../../utils/dom.js';
import { terminalStore, closeTerminal, setActiveSession, createTerminal, splitTerminal, setDefaultShell } from '../../state/terminal.js';
import { editorStore } from '../../state/editor.js';
import { workspaceStore } from '../../state/workspace.js';
import { showContextMenu } from '../dropdown-menu.js';

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

/**
 * Get the root path of the project associated with the active editor file.
 * Falls back to the first open project, or null.
 */
function getActiveProjectRoot() {
  const activeId = editorStore.getState('activeBufferId');
  const buffers = editorStore.getState('openBuffers');
  const projects = workspaceStore.getState('projects');

  if (activeId != null && buffers[activeId]) {
    const buf = buffers[activeId];
    // Match by project name
    if (buf.projectName) {
      const project = projects.find(p => p.name === buf.projectName);
      if (project?.root_path) return project.root_path;
    }
    // Match by file path prefix
    if (buf.filePath) {
      for (const p of projects) {
        if (buf.filePath.startsWith(p.root_path)) return p.root_path;
      }
    }
  }

  // Fallback: first project in workspace
  if (projects.length > 0 && projects[0].root_path) {
    return projects[0].root_path;
  }

  return null;
}

/** Extract a short display name from a full path */
function shortenCwd(cwd) {
  if (!cwd) return '';
  // Show last folder name
  const normalized = cwd.replace(/\\/g, '/').replace(/\/$/, '');
  const parts = normalized.split('/');
  return parts[parts.length - 1] || cwd;
}

export function createTerminalTabs() {
  const container = el('div', { class: 'terminal-tabs' });

  // Scrollable tabs list — action buttons stay outside this
  const tabsList = el('div', { class: 'terminal-tabs__list' });
  const scrollList = makeSmoothScroller(tabsList);
  tabsList.addEventListener('wheel', (e) => {
    if (e.deltaY !== 0) {
      e.preventDefault();
      scrollList(e.deltaY * 0.5);
    }
  }, { passive: false });
  container.appendChild(tabsList);

  // Action buttons — built once, stay fixed at the end
  const addBtn = el('button', { class: 'terminal-tabs__add', title: 'New Terminal' });
  addBtn.appendChild(icon('M12 5v14M5 12h14', 14));
  addBtn.addEventListener('click', () => {
    const cwd = getActiveProjectRoot();
    createTerminal(cwd);
  });
  container.appendChild(addBtn);

  const dropdownBtn = el('button', { class: 'terminal-tabs__add', title: 'Select Shell' });
  dropdownBtn.appendChild(icon('M6 9l6 6 6-6', 14));
  dropdownBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    showShellDropdown(e);
  });
  container.appendChild(dropdownBtn);

  const splitBtn = el('button', { class: 'terminal-tabs__add', title: 'Split Terminal' });
  splitBtn.appendChild(icon('M9 3H5a2 2 0 00-2 2v14a2 2 0 002 2h4M15 3h4a2 2 0 012 2v14a2 2 0 01-2 2h-4M12 3v18', 14));
  splitBtn.addEventListener('click', () => {
    const cwd = getActiveProjectRoot();
    splitTerminal(cwd);
  });
  container.appendChild(splitBtn);

  function render() {
    tabsList.innerHTML = '';

    const sessions = terminalStore.getState('sessions');
    const activeId = terminalStore.getState('activeSessionId');

    for (const session of sessions) {
      const isActive = session.id === activeId;

      const tab = el('button', {
        class: `terminal-tabs__tab ${isActive ? 'terminal-tabs__tab--active' : ''}`,
      });

      // Agent icon or terminal icon
      const tabIcon = session.is_agent
        ? icon('M12 2a2 2 0 0 1 2 2c0 .74-.4 1.39-1 1.73V7h1a7 7 0 0 1 7 7h1a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-1.07A7 7 0 0 1 14 22h-4a7 7 0 0 1-6.93-6H2a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1h1a7 7 0 0 1 7-7h1V5.73c-.6-.34-1-.99-1-1.73a2 2 0 0 1 2-2', 12)
        : icon('M4 17l6-6-6-6M12 19h8', 12);

      const cwdShort = shortenCwd(session.cwd);
      const labelText = cwdShort ? `${session.label}: ${cwdShort}` : session.label;
      const label = el('span', { class: 'terminal-tabs__label', title: session.cwd || '' }, labelText);

      const closeBtn = el('span', { class: 'terminal-tabs__close' });
      closeBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 10));
      closeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        closeTerminal(session.id);
      });

      tab.appendChild(tabIcon);
      tab.appendChild(label);
      tab.appendChild(closeBtn);

      tab.addEventListener('click', () => setActiveSession(session.id));

      tab.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        e.stopPropagation();
        showContextMenu([
          {
            label: 'Clear Terminal',
            action: () => {
              // Dispatch a custom event that terminal-pane can listen to
              window.dispatchEvent(new CustomEvent('rustic:clear-terminal', { detail: { sessionId: session.id } }));
            },
          },
          { separator: true },
          {
            label: 'Kill Terminal',
            action: () => closeTerminal(session.id),
          },
        ], e.clientX, e.clientY);
      });

      tabsList.appendChild(tab);
    }

    // Scroll active tab into view
    const activeTab = tabsList.querySelector('.terminal-tabs__tab--active');
    if (activeTab) {
      requestAnimationFrame(() => activeTab.scrollIntoView({ block: 'nearest', inline: 'nearest' }));
    }
  }

  function showShellDropdown(e) {
    const shells = terminalStore.getState('availableShells');
    const defaultPath = terminalStore.getState('defaultShellPath');

    if (!shells || shells.length === 0) {
      // No shells detected, just create a default terminal
      const cwd = getActiveProjectRoot();
      createTerminal(cwd);
      return;
    }

    const items = [];

    // Section header: "New Terminal With..."
    items.push({ label: '── New Terminal ──', disabled: true });

    for (const shell of shells) {
      const isDefault = shell.path === defaultPath;
      items.push({
        label: `${shell.name}${isDefault ? ' (default)' : ''}`,
        action: () => {
          const cwd = getActiveProjectRoot();
          createTerminal(cwd, shell.name, shell.path);
        },
      });
    }

    items.push({ separator: true });

    // Section header: "Set Default Shell"
    items.push({ label: '── Set Default ──', disabled: true });

    for (const shell of shells) {
      const isDefault = shell.path === defaultPath;
      items.push({
        label: `${isDefault ? '● ' : '○ '}${shell.name}`,
        action: () => {
          setDefaultShell(shell.path);
        },
      });
    }

    // Position dropdown above the button since terminal is at the bottom of the screen
    const rect = e.currentTarget.getBoundingClientRect();
    // Estimate menu height: ~28px per item + 8px padding
    const menuHeight = items.length * 28 + 8;
    showContextMenu(items, rect.left, rect.top - menuHeight);
  }

  terminalStore.subscribe('sessions', render);
  terminalStore.subscribe('activeSessionId', render);

  render();
  return container;
}
