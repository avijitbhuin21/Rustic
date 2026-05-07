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

function initApp() {
  // Capture unhandled rejections + window errors as visible toasts.
  installGlobalErrorToasts();

  // Restore provider config flags from the backend's persisted ai_config so a
  // wiped WebView localStorage (Tauri 2 dev rebuilds occasionally clear it)
  // doesn't make a real, keychain-backed provider look "Not connected" until
  // the user re-enters the key. Fire-and-forget — the AI Settings UI also
  // hydrates lazily, so we don't need to block boot on this.
  hydrateProviderConfigsFromBackend().catch((e) => {
    console.warn('[boot] provider hydrate failed:', e);
  });

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

  // When no editor files are open, the chat panel expands to fill the
  // editor column. Toggle a class on #app so CSS can collapse the editor
  // grid column and stretch the secondary sidebar.
  function syncNoOpenFiles() {
    const groups = editorStore.getState('groups');
    const buffers = editorStore.getState('openBuffers');
    // Cross-reference bufferIds against openBuffers — a group can hold a
    // stale id pointing to a buffer that was removed elsewhere (e.g. Settings
    // close path), and length-only would keep the editor column alive.
    const noFiles = !groups.some(g => g.bufferIds.some(id => buffers[id]));
    app.classList.toggle('no-open-files', noFiles);
    // In no-files mode, force the chat panel to be visible so the user
    // actually sees it expanded. When a file is opened, the class drops
    // and the sidebar returns to whatever visibility it had before.
    if (noFiles && !uiStore.getState('secondarySidebarVisible')) {
      uiStore.setState({ secondarySidebarVisible: true });
    }
  }
  editorStore.subscribe('groups', syncNoOpenFiles);
  editorStore.subscribe('openBuffers', syncNoOpenFiles);
  syncNoOpenFiles();

  // Initialize workspace (load saved projects)
  initWorkspace();

  // Detect available shells for the terminal dropdown
  loadAvailableShells();

  // Load settings, then apply theme + fonts. The active_theme name may
  // refer to a saved palette stored only in localStorage (not in the DB),
  // in which case the backend's getActiveTheme falls back to gruvbox dark
  // — so we check localStorage first and apply from there if it matches.
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
    // Apply saved font settings
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
        const fontFace = new FontFace(font.name, `url(${font.url})`);
        fontFace.load().then(() => document.fonts.add(fontFace)).catch(() => {});
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

  if (import.meta.env.PROD) {
    document.addEventListener('contextmenu', (e) => {
      e.preventDefault();
    });
  }

  // Register all global commands and start the keybinding dispatcher.
  // Per-shortcut keydown handlers used to live here; they now flow through
  // the central dispatcher so users can rebind them from Settings.
  registerBuiltinCommands();
  setOverrides(settingsStore.getState('settings')?.keybindings || []);
  installKeybindingListener();

  // First-run wizard. Guarded by a localStorage flag — the wizard sets it
  // when the user clicks Skip or completes the final step. We wait one tick
  // so the rest of the layout has rendered before showing the overlay.
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
  // Reload overrides whenever settings change (e.g. user edited a shortcut).
  settingsStore.subscribe('settings', (s) => {
    setOverrides(s?.keybindings || []);
  });

  // Listen for file open events from explorer
  window.addEventListener('rustic:open-file', (e) => {
    const { path, projectName } = e.detail;
    openFile(path, projectName);
  });

  // ───── App lifecycle: dirty-buffer prompt on quit ─────────────────────
  // Backend prevents the close and emits "rustic:close-requested". We check
  // for dirty buffers, prompt the user, then either let the app exit or
  // leave the window open.
  api.onEvent('rustic:close-requested', async () => {
    const buffers = editorStore.getState('openBuffers') || {};
    const dirty = Object.values(buffers).filter((b) => b && b.isModified);

    if (dirty.length === 0) {
      await api.confirmQuit();
      return;
    }

    const { showUnsavedDialog } = await import('./components/confirm-dialog.js');
    // For multiple dirty files, prompt for each; if any is cancelled, abort.
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
