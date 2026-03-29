import { createTopBar } from './components/top-bar.js';
import { createActivityBar } from './components/activity-bar.js';
import { createPrimarySidebar } from './components/primary-sidebar.js';
import { createEditorArea } from './components/editor-area.js';
import { createSecondarySidebar } from './components/secondary-sidebar.js';
import { createBottomPanel } from './components/bottom-panel.js';
import { createStatusBar } from './components/status-bar.js';
import { uiStore } from './state/ui.js';
import { initWorkspace } from './state/workspace.js';
import { openFile, editorStore } from './state/editor.js';
import { revealFileInExplorer } from './components/explorer/file-tree-item.js';
import { applyTheme } from './lib/theme.js';
import * as api from './lib/tauri-api.js';
import { loadSettings, settingsStore, updateSetting } from './state/settings.js';
import { loadAvailableShells } from './state/terminal.js';
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

  // Secondary sidebar with resize handle
  const secondarySidebar = createSecondarySidebar();
  secondarySidebar.style.position = 'relative';
  const secondaryHandle = createResizeHandle('v', 'secondary');
  // Position handle on left edge for secondary sidebar
  secondaryHandle.style.right = '';
  secondaryHandle.style.left = '0';
  secondarySidebar.appendChild(secondaryHandle);
  app.appendChild(secondarySidebar);

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

  // Detect available shells for the terminal dropdown
  loadAvailableShells();

  // Load and apply saved theme
  api.getActiveTheme().then((theme) => {
    if (theme) applyTheme(theme);
  }).catch(() => {});

  // Load settings and init zoom, apply saved fonts
  loadSettings().then(() => {
    initZoom();
    // Apply saved font settings
    const settings = settingsStore.getState('settings');
    const savedFont = settings?.appearance?.font_family;
    if (savedFont) {
      const root = document.documentElement;
      const family = `"${savedFont}", monospace`;
      const uiFamily = `"${savedFont}", -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif`;
      root.style.setProperty('--font-family', uiFamily);
      root.style.setProperty('--font-family-mono', family);
    }
    if (settings?.appearance?.font_size) {
      document.documentElement.style.setProperty('--font-size-editor', settings.appearance.font_size + 'px');
    }
    // Reload fonts from library (URL-based and file-based)
    const fontLibrary = JSON.parse(localStorage.getItem('rustic_font_library') || '[]');
    for (const font of fontLibrary) {
      if (font.source === 'url' && font.url) {
        import('./lib/font-loader.js').then(({ loadFontFromUrl }) => {
          loadFontFromUrl(font.url).catch(() => {});
        });
      } else if (font.source === 'file' && font.url) {
        // Re-load file-based fonts from disk
        import('./lib/tauri-api.js').then((api) => {
          api.readFileBase64(font.url).then((response) => {
            const base64 = response?.data || response;
            if (!base64) return;
            const ext = font.url.split('.').pop().toLowerCase();
            const mimeMap = { ttf: 'font/ttf', otf: 'font/otf', woff: 'font/woff', woff2: 'font/woff2' };
            const mime = mimeMap[ext] || 'font/opentype';
            const dataUrl = `data:${mime};base64,${base64}`;
            const fontFace = new FontFace(font.name, `url(${dataUrl})`);
            fontFace.load().then(() => document.fonts.add(fontFace)).catch(() => {});
          }).catch(() => {});
        });
      }
    }
    // Apply saved per-element font config (overrides global font for specific targets)
    const fontConfig = JSON.parse(localStorage.getItem('rustic_font_config') || 'null');
    if (fontConfig) {
      const root = document.documentElement;
      if (fontConfig.editor) root.style.setProperty('--font-family-mono', fontConfig.editor);
      if (fontConfig.terminal) root.style.setProperty('--font-family-terminal', fontConfig.terminal);
      if (fontConfig.folderNames) root.style.setProperty('--font-family-folders', fontConfig.folderNames);
      if (fontConfig.fileNames) root.style.setProperty('--font-family-files', fontConfig.fileNames);
    }
  });

  // Disable default browser context menu everywhere.
  // Custom context menus are set per-element (file tree, terminal, etc.).
  // Areas with no custom menu show nothing on right-click.
  // NOTE: temporarily commented out to allow DevTools access via right-click → Inspect.
  // document.addEventListener('contextmenu', (e) => {
  //   e.preventDefault();
  // });

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

  // Ctrl+B: Toggle sidebar
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key === 'b') {
      e.preventDefault();
      uiStore.setState({ primarySidebarVisible: !uiStore.getState('primarySidebarVisible') });
    }
  });

  // Ctrl+J: Toggle bottom panel
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key === 'j') {
      e.preventDefault();
      uiStore.setState({ bottomPanelVisible: !uiStore.getState('bottomPanelVisible') });
    }
  });

  // Alt+Z: Toggle word wrap
  document.addEventListener('keydown', (e) => {
    if (e.altKey && !e.ctrlKey && !e.shiftKey && e.key === 'z') {
      e.preventDefault();
      const s = settingsStore.getState('settings');
      const current = s?.editor?.word_wrap ?? false;
      updateSetting('editor.word_wrap', !current);
    }
  });

  // Listen for file open events from explorer
  window.addEventListener('rustic:open-file', (e) => {
    const { path, projectName } = e.detail;
    openFile(path, projectName);
  });

  // Auto-reveal active file in explorer sidebar
  editorStore.subscribe('activeBufferId', (bufferId) => {
    if (!bufferId) return;
    // Only reveal when explorer panel is visible
    if (uiStore.getState('activePanel') !== 'explorer') return;
    if (!uiStore.getState('primarySidebarVisible')) return;
    const buffers = editorStore.getState('openBuffers');
    const buffer = buffers[bufferId];
    if (buffer && buffer.filePath) {
      revealFileInExplorer(buffer.filePath);
    }
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
    // Vertical splitter — edge of sidebar (inside container to avoid overflow clipping)
    Object.assign(handle.style, {
      position: 'absolute', top: '0', right: '0',
      width: '4px', height: '100%', cursor: 'col-resize', zIndex: '50',
    });
  } else {
    // Horizontal splitter — top edge of bottom panel
    Object.assign(handle.style, {
      position: 'absolute', top: '0', left: '0',
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
