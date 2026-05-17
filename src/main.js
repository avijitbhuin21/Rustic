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
import { loadSettings, settingsStore } from './state/settings.js';
import { loadAvailableShells } from './state/terminal.js';
import { initZoom } from './lib/zoom.js';
import { registerBuiltinCommands } from './lib/builtin-commands.js';
import { installKeybindingListener, setOverrides } from './lib/keybindings.js';
import { installGlobalErrorToasts, showToast, showErrorToast } from './components/toast.js';
import { hydrateProviderConfigsFromBackend } from './components/settings/ai-settings.js';
import { installLongTaskObserver, installHeartbeat } from './lib/perf-debug.js';
import { initMcpConsentListener } from './components/mcp-consent-dialog.js';
import { workspaceStore } from './state/workspace.js';

function initApp() {
  // Capture unhandled rejections + window errors as visible toasts.
  installGlobalErrorToasts();

  installLongTaskObserver();
  installHeartbeat();

  // Restore provider config from backend so a wiped WebView localStorage doesn't
  // make a keychain-backed provider look disconnected until the user re-enters the key.
  hydrateProviderConfigsFromBackend().catch((e) => {
    console.warn('[boot] provider hydrate failed:', e);
  });

  const app = document.getElementById('app');

  syncCssVariables();

  // Top bar lives outside #app so zoom never affects it
  document.body.insertBefore(createTopBar(), app);

  app.appendChild(createActivityBar());

  const sidebarContainer = createPrimarySidebar();
  const sidebarHandle = createResizeHandle('v', 'sidebar');
  sidebarContainer.style.position = 'relative';
  sidebarContainer.appendChild(sidebarHandle);
  app.appendChild(sidebarContainer);

  app.appendChild(createEditorArea());

  const secondarySidebar = createSecondarySidebar();
  secondarySidebar.style.position = 'relative';
  const secondaryHandle = createResizeHandle('v', 'secondary');
  // Handle on left edge for secondary sidebar
  secondaryHandle.style.right = '';
  secondaryHandle.style.left = '0';
  secondarySidebar.appendChild(secondaryHandle);
  app.appendChild(secondarySidebar);

  const bottomPanel = createBottomPanel();
  bottomPanel.style.position = 'relative';
  const panelHandle = createResizeHandle('h', 'panel');
  bottomPanel.appendChild(panelHandle);
  app.appendChild(bottomPanel);

  document.body.appendChild(createStatusBar());

  uiStore.subscribe('primarySidebarVisible', syncCssVariables);
  uiStore.subscribe('bottomPanelVisible', syncCssVariables);
  uiStore.subscribe('secondarySidebarVisible', syncCssVariables);
  uiStore.subscribe('sidebarWidth', syncCssVariables);
  uiStore.subscribe('panelHeight', syncCssVariables);
  uiStore.subscribe('secondarySidebarWidth', syncCssVariables);

  // Collapse editor column and expand chat panel when no files are open.
  function syncNoOpenFiles() {
    const groups = editorStore.getState('groups');
    const buffers = editorStore.getState('openBuffers');
    // Cross-reference against openBuffers — a group can hold a stale id after
    // a buffer is removed, so length-only would keep the editor column alive.
    const noFiles = !groups.some(g => g.bufferIds.some(id => buffers[id]));
    app.classList.toggle('no-open-files', noFiles);
    if (noFiles && !uiStore.getState('secondarySidebarVisible')) {
      uiStore.setState({ secondarySidebarVisible: true });
    }
  }
  editorStore.subscribe('groups', syncNoOpenFiles);
  editorStore.subscribe('openBuffers', syncNoOpenFiles);
  syncNoOpenFiles();

  initWorkspace();
  loadAvailableShells();

  // Check localStorage for saved palettes first — backend falls back to gruvbox dark.
  loadSettings().then(() => {
    initZoom();
    const settings = settingsStore.getState('settings');
    const activeName = settings?.theme?.active_theme;
    const savedPalettes = settingsStore.getState('savedPalettes') || [];
    const savedMatch = activeName ? savedPalettes.find(p => p.name === activeName) : null;
    if (savedMatch) {
      const root = document.documentElement;
      const varMap = {
        bg_hard: '--bg-hard', bg: '--bg', bg_soft: '--bg-soft',
        bg1: '--bg1', bg2: '--bg2', bg3: '--bg3', bg4: '--bg4',
        fg: '--fg', fg1: '--fg1', fg2: '--fg2', fg3: '--fg3', fg4: '--fg4',
        accent: '--accent', border: '--border',
        bright_red: '--bright-red', bright_green: '--bright-green',
        bright_yellow: '--bright-yellow', bright_blue: '--bright-blue',
        bright_purple: '--bright-purple', bright_aqua: '--bright-aqua',
        bright_orange: '--bright-orange',
        token_keyword: '--token-keyword', token_string: '--token-string',
        token_comment: '--token-comment', token_function: '--token-function',
        token_type: '--token-type', token_variable: '--token-variable',
        token_number: '--token-number', token_operator: '--token-operator',
        token_punctuation: '--token-punctuation',
      };
      for (const [k, v] of Object.entries(varMap)) {
        if (savedMatch.data[k]) root.style.setProperty(v, savedMatch.data[k]);
      }
    } else {
      api.getActiveTheme().then((theme) => {
        if (theme) applyTheme(theme);
      }).catch(() => {});
    }
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
    const fontLibrary = JSON.parse(localStorage.getItem('rustic_font_library') || '[]');
    for (const font of fontLibrary) {
      if (font.source === 'url' && font.url) {
        import('./lib/font-loader.js').then(({ loadFontFromUrl }) => {
          loadFontFromUrl(font.url).catch(() => {});
        });
      } else if (font.source === 'file' && font.url) {
        const fontFace = new FontFace(font.name, `url(${font.url})`);
        fontFace.load().then(() => document.fonts.add(fontFace)).catch(() => {});
      }
    }
    const fontConfig = JSON.parse(localStorage.getItem('rustic_font_config') || 'null');
    if (fontConfig) {
      const root = document.documentElement;
      if (fontConfig.editor) root.style.setProperty('--font-family-mono', fontConfig.editor);
      if (fontConfig.terminal) root.style.setProperty('--font-family-terminal', fontConfig.terminal);
      if (fontConfig.folderNames) root.style.setProperty('--font-family-folders', fontConfig.folderNames);
      if (fontConfig.fileNames) root.style.setProperty('--font-family-files', fontConfig.fileNames);
      if (fontConfig.agentChat) {
        root.style.setProperty('--font-family-chat', fontConfig.agentChat);
        const styleEl = document.createElement('style');
        styleEl.id = 'rustic-chat-font-style';
        styleEl.textContent = '.chat-messages { font-family: var(--font-family-chat, inherit); }\n.chat-message__text { font-family: var(--font-family-chat, inherit); }';
        document.head.appendChild(styleEl);
      }
    }
  });

  if (import.meta.env.PROD) {
    document.addEventListener('contextmenu', (e) => {
      e.preventDefault();
    });
  }

  registerBuiltinCommands();
  setOverrides(settingsStore.getState('settings')?.keybindings || []);
  installKeybindingListener();

  // Wait one tick so layout is rendered before showing the overlay.
  setTimeout(() => {
    import('./components/onboarding/onboarding-wizard.js').then(({
      isOnboardingComplete,
      showOnboardingWizard,
    }) => {
      if (!isOnboardingComplete()) showOnboardingWizard();
    }).catch((e) => {
      console.error('Failed to load onboarding wizard:', e);
    });
  }, 50);
  settingsStore.subscribe('settings', (s) => {
    setOverrides(s?.keybindings || []);
  });

  window.addEventListener('rustic:open-file', (e) => {
    const { path, projectName } = e.detail;
    openFile(path, projectName);
  });

  api.onEvent('rustic:close-requested', async () => {
    const buffers = editorStore.getState('openBuffers') || {};
    const dirty = Object.values(buffers).filter((b) => b && b.isModified);

    if (dirty.length === 0) {
      await api.confirmQuit();
      return;
    }

    const { showUnsavedDialog } = await import('./components/confirm-dialog.js');
    for (const buf of dirty) {
      const result = await showUnsavedDialog(buf.fileName);
      if (result === 'cancel') return; // user cancelled — stay open
      if (result === 'save') {
        try {
          await api.saveFile(buf.id, true);
        } catch (e) {
          console.error('Failed to save before quit:', e);
          return; // don't quit on save failure
        }
      }
      // 'discard' — drop changes, continue
    }
    await api.confirmQuit();
  }).catch(() => {});

  initMcpConsentListener((projectPath) => {
    const norm = (p) => (p || '').replace(/\\/g, '/').toLowerCase();
    const target = norm(projectPath).replace(/\/\.mcp\.json$/, '');
    const projects = workspaceStore.getState('projects') || [];
    const match = projects.find((p) => norm(p.path) === target || norm(p.root_path) === target);
    return match ? match.id : null;
  });

  // Second-instance forwarding: open the path argument that was passed to
  // the secondary launcher.
  api.onEvent('rustic:open-path', (e) => {
    const path = typeof e?.payload === 'string' ? e.payload : null;
    if (!path) return;
    openFile(path);
  }).catch(() => {});

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
  const app = document.getElementById('app');

  root.style.setProperty('--sidebar-width',
    state.primarySidebarVisible ? state.sidebarWidth + 'px' : '0px'
  );
  root.style.setProperty('--panel-height',
    state.bottomPanelVisible ? state.panelHeight + 'px' : '0px'
  );
  root.style.setProperty('--secondary-width',
    state.secondarySidebarVisible ? state.secondarySidebarWidth + 'px' : '0px'
  );
  // The `panel-visible` class lets CSS branch its grid template based on
  // whether the bottom panel is showing. Specifically, in `no-open-files`
  // mode we want the chat to dock back into its narrow right-hand column
  // (instead of stretching across the editor area) when the terminal is
  // up — so the user sees: [sidebar][empty editor][chat] on top, and
  // [sidebar][      terminal      ][chat] on the bottom. Without this
  // class the chat would float over the terminal area.
  if (app) app.classList.toggle('panel-visible', !!state.bottomPanelVisible);
}

// --- Resize handles ---

/**
 * Returns the current CSS zoom scale applied to #app.
 * getBoundingClientRect() returns visual (post-zoom) coordinates while offsetWidth
 * returns the layout (pre-zoom) width, so their ratio equals the zoom scale.
 */
function getZoomScale() {
  const app = document.getElementById('app');
  if (!app || !app.offsetWidth) return 1.0;
  return app.getBoundingClientRect().width / app.offsetWidth;
}

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
    // Body class disables panel width/height transitions during the drag —
    // otherwise every frame's width change triggers a 150ms tween, the eye
    // sees the panel "chasing" the cursor instead of tracking it.
    document.body.classList.add('is-resizing');
    document.body.style.cursor = direction === 'v' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';

    const onMouseMove = (e) => {
      const ACTIVITY_BAR = 36;
      const MIN_EDITOR = 120;
      // e.clientX/Y are in visual (zoomed) viewport pixels.
      // offsetWidth/offsetHeight are in CSS (pre-zoom) layout pixels.
      // Divide visual coords by scale to get CSS pixel values for state.
      const scale = getZoomScale();
      if (target === 'sidebar') {
        const appWidth = document.getElementById('app').offsetWidth;
        const secondaryVisible = uiStore.getState('secondarySidebarVisible');
        const secondaryWidth = secondaryVisible ? (uiStore.getState('secondarySidebarWidth') || 0) : 0;
        const maxWidth = appWidth - ACTIVITY_BAR - MIN_EDITOR - secondaryWidth;
        const width = Math.max(160, Math.min(maxWidth, e.clientX / scale - ACTIVITY_BAR));
        uiStore.setState({ sidebarWidth: width });
      } else if (target === 'panel') {
        const appHeight = document.getElementById('app').offsetHeight;
        // Panel CSS height = appHeight - (e.clientY - topBarHeight) / scale
        const height = Math.max(100, Math.min(appHeight - 200, appHeight - (e.clientY - 35) / scale));
        uiStore.setState({ panelHeight: height });
      } else if (target === 'secondary') {
        const appWidth = document.getElementById('app').offsetWidth;
        const primaryVisible = uiStore.getState('primarySidebarVisible');
        const primaryWidth = primaryVisible ? (uiStore.getState('sidebarWidth') || 0) : 0;
        const maxWidth = appWidth - ACTIVITY_BAR - MIN_EDITOR - primaryWidth;
        const width = Math.max(200, Math.min(maxWidth, appWidth - e.clientX / scale));
        uiStore.setState({ secondarySidebarWidth: width });
      }
    };

    const onMouseUp = () => {
      handle.classList.remove('active');
      document.body.classList.remove('is-resizing');
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
