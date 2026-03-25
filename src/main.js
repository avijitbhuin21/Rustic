import { createTopBar } from './components/top-bar.js';
import { createActivityBar } from './components/activity-bar.js';
import { createPrimarySidebar } from './components/primary-sidebar.js';
import { createEditorArea } from './components/editor-area.js';
import { createSecondarySidebar } from './components/secondary-sidebar.js';
import { createBottomPanel } from './components/bottom-panel.js';
import { createStatusBar } from './components/status-bar.js';
import { uiStore } from './state/ui.js';
import { initWorkspace } from './state/workspace.js';
import { openFile } from './state/editor.js';
import { applyTheme } from './lib/theme.js';
import * as api from './lib/tauri-api.js';
import { loadSettings } from './state/settings.js';
import { initZoom, zoomIn, zoomOut, resetZoom } from './lib/zoom.js';

function initApp() {
  const app = document.getElementById('app');

  // Apply initial CSS variables
  syncCssVariables();

  // Top bar lives outside #app so zoom never affects it
  document.body.insertBefore(createTopBar(), app);

  // Build layout
  app.appendChild(createActivityBar());

  // Primary sidebar with resize handle
  const sidebarContainer = createPrimarySidebar();
  const sidebarHandle = createResizeHandle('v', 'sidebar');
  sidebarContainer.style.position = 'relative';
  sidebarContainer.appendChild(sidebarHandle);
  app.appendChild(sidebarContainer);

  // Editor area
  app.appendChild(createEditorArea());

  // Secondary sidebar
  app.appendChild(createSecondarySidebar());

  // Bottom panel with resize handle
  const bottomPanel = createBottomPanel();
  bottomPanel.style.position = 'relative';
  const panelHandle = createResizeHandle('h', 'panel');
  bottomPanel.appendChild(panelHandle);
  app.appendChild(bottomPanel);

  // Status bar (fixed at bottom)
  document.body.appendChild(createStatusBar());

  // Subscribe to visibility changes for grid adjustments
  uiStore.subscribe('primarySidebarVisible', syncCssVariables);
  uiStore.subscribe('bottomPanelVisible', syncCssVariables);
  uiStore.subscribe('secondarySidebarVisible', syncCssVariables);
  uiStore.subscribe('sidebarWidth', syncCssVariables);
  uiStore.subscribe('panelHeight', syncCssVariables);
  uiStore.subscribe('secondarySidebarWidth', syncCssVariables);

  // Initialize workspace (load saved projects)
  initWorkspace();

  // Load and apply saved theme
  api.getActiveTheme().then((theme) => {
    if (theme) applyTheme(theme);
  }).catch(() => {});

  // Load settings and init zoom
  loadSettings().then(() => initZoom());

  // Global keyboard shortcuts for zoom
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && !e.shiftKey && !e.altKey) {
      if (e.key === '=' || e.key === '+') {
        e.preventDefault();
        zoomIn();
      } else if (e.key === '-') {
        e.preventDefault();
        zoomOut();
      } else if (e.key === '0') {
        e.preventDefault();
        resetZoom();
      }
    }
  });

  // Listen for file open events from explorer
  window.addEventListener('rustic:open-file', (e) => {
    const { path, projectName } = e.detail;
    openFile(path, projectName);
  });
}

function syncCssVariables() {
  const root = document.documentElement;
  const state = uiStore.getState();

  root.style.setProperty('--sidebar-width',
    state.primarySidebarVisible ? state.sidebarWidth + 'px' : '0px'
  );
  root.style.setProperty('--panel-height',
    state.bottomPanelVisible ? state.panelHeight + 'px' : '0px'
  );
  root.style.setProperty('--secondary-width',
    state.secondarySidebarVisible ? state.secondarySidebarWidth + 'px' : '0px'
  );
}

// --- Resize handles ---

function createResizeHandle(direction, target) {
  const handle = document.createElement('div');
  handle.className = `resize-handle resize-handle-${direction}`;

  if (direction === 'v') {
    // Vertical splitter — right edge of sidebar
    Object.assign(handle.style, {
      position: 'absolute', top: '0', right: '-2px',
      width: '4px', height: '100%', cursor: 'col-resize', zIndex: '50',
    });
  } else {
    // Horizontal splitter — top edge of bottom panel
    Object.assign(handle.style, {
      position: 'absolute', top: '-2px', left: '0',
      width: '100%', height: '4px', cursor: 'row-resize', zIndex: '50',
    });
  }

  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    handle.classList.add('active');
    document.body.style.cursor = direction === 'v' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';

    const onMouseMove = (e) => {
      if (target === 'sidebar') {
        const width = Math.max(160, Math.min(600, e.clientX - 48)); // 48 = activity bar
        uiStore.setState({ sidebarWidth: width });
      } else if (target === 'panel') {
        const appHeight = document.getElementById('app').offsetHeight;
        const height = Math.max(100, Math.min(appHeight - 200, appHeight - e.clientY + 35));
        uiStore.setState({ panelHeight: height });
      } else if (target === 'secondary') {
        const appWidth = document.getElementById('app').offsetWidth;
        const width = Math.max(200, Math.min(600, appWidth - e.clientX));
        uiStore.setState({ secondarySidebarWidth: width });
      }
    };

    const onMouseUp = () => {
      handle.classList.remove('active');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    };

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
  });

  return handle;
}

// Initialize when DOM is ready
document.addEventListener('DOMContentLoaded', initApp);
