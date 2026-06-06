import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

// Embedded VM browser store (web build only). Mirrors the terminal store's
// shape: backend-owned session state synced over the `/ws` hub, plus local
// window chrome (movable/resizable rect + min/max/close) that persists across
// reloads like the terminal's layout mode.

let listenersWired = false;

const WINDOW_RECT_KEY = 'rustic.browser.windowRect';

const DEFAULT_RECT = { x: 120, y: 90, w: 1024, h: 720 };

function loadRect() {
  try {
    const raw = localStorage.getItem(WINDOW_RECT_KEY);
    if (!raw) return { ...DEFAULT_RECT };
    const r = JSON.parse(raw);
    // Validate so a corrupt value can't wedge the window off-screen.
    if (
      r &&
      Number.isFinite(r.x) &&
      Number.isFinite(r.y) &&
      Number.isFinite(r.w) &&
      Number.isFinite(r.h) &&
      r.w > 200 &&
      r.h > 150
    ) {
      return r;
    }
  } catch {
    /* fall through to default */
  }
  return { ...DEFAULT_RECT };
}

function saveRect(rect) {
  try {
    localStorage.setItem(WINDOW_RECT_KEY, JSON.stringify(rect));
  } catch {
    /* ignore storage errors (private mode) */
  }
}

// Pick the active tab after a list refresh: keep the current one if it survived,
// otherwise fall back to the first tab (or null when there are none).
function resolveActive(tabs, current) {
  if (current && tabs.some((t) => t.id === current)) return current;
  return tabs[0]?.id ?? null;
}

export const useBrowser = create((set, get) => ({
  running: false,
  tabs: [],
  activeTabId: null,
  // 'closed' | 'normal' | 'minimized' | 'maximized'
  windowState: 'closed',
  windowRect: loadRect(),
  // True while an open / new-tab round-trip is in flight (drives a spinner).
  busy: false,
  // How "Open in my browser" reaches a VM dev server: 'path' (default,
  // same-origin /proxy/<port>), 'subdomain' (<port>.<previewDomain>), or
  // 'cloudflare' (on-demand quick tunnel). Synced from get_tunnel_config.
  tunnelMode: 'path',
  // Wildcard preview domain for subdomain port-forwarding (null = path mode).
  previewDomain: null,

  wireListeners: async () => {
    if (listenersWired) return;
    listenersWired = true;
    // Wire the browser event listeners FIRST — the embedded browser must work
    // regardless of the tunnel feature. The tunnel-config fetch below only
    // decides which URL the "Open in my browser" button builds, so it runs
    // fire-and-forget and can never delay or break the browser UI.
    await listen('browser-tabs-changed', () => {
      get().refreshTabs();
    });
    await listen('browser-stopped', () => {
      // Chromium is gone (crash, last-tab-close, idle watchdog, or our own
      // teardown). Reflect that: no tabs, window closed.
      set({ running: false, tabs: [], activeTabId: null, windowState: 'closed' });
    });
    invoke('get_tunnel_config')
      .then((cfg) => {
        // Store the mode so "Open in my browser" routes correctly — without
        // this it stays 'path' and cloudflare/subdomain selections are ignored.
        if (cfg?.mode) set({ tunnelMode: cfg.mode });
        if (cfg?.mode === 'subdomain' && cfg?.previewDomain) {
          set({ previewDomain: cfg.previewDomain });
        }
      })
      .catch(() => {
        /* path mode (no preview domain configured) */
      });
  },

  // Open the given tab's URL in the USER's own browser. Loopback dev-server
  // URLs route through the configured tunnel (path / subdomain / cloudflare);
  // public URLs open directly. Cloudflare mode spawns the tunnel on demand.
  openExternal: async (tab) => {
    const rawUrl = tab?.url;
    if (!rawUrl || rawUrl === 'about:blank') return;
    let u;
    try {
      u = new URL(rawUrl);
    } catch {
      return;
    }
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return;

    const LOOPBACK = ['localhost', '127.0.0.1', '0.0.0.0', '::1', '[::1]'];
    if (!LOOPBACK.includes(u.hostname)) {
      window.open(rawUrl, '_blank', 'noopener');
      return;
    }

    const port = u.port || (u.protocol === 'https:' ? '443' : '80');
    const tail = `${u.pathname}${u.search}${u.hash}`;
    const { tunnelMode, previewDomain } = get();

    if (tunnelMode === 'cloudflare') {
      try {
        const res = await invoke('tunnel_open', { port: Number(port) });
        if (res?.url) window.open(`${res.url}${tail}`, '_blank', 'noopener');
      } catch (e) {
        console.error('[browser] cloudflare tunnel_open failed', e);
      }
      return;
    }
    if (tunnelMode === 'subdomain' && previewDomain) {
      window.open(`https://${port}.${previewDomain}${tail}`, '_blank', 'noopener');
      return;
    }
    window.open(`${window.location.origin}/proxy/${port}${tail}`, '_blank', 'noopener');
  },

  refreshTabs: async () => {
    try {
      const res = await invoke('browser_status');
      const tabs = res?.tabs ?? [];
      set((s) => ({
        running: !!res?.running,
        tabs,
        activeTabId: resolveActive(tabs, s.activeTabId),
      }));
    } catch {
      /* transient; the next event re-syncs */
    }
  },

  // Open the window + start Chromium (idempotent on the backend). Ensures at
  // least one tab exists.
  open: async () => {
    set((s) => ({
      busy: true,
      windowState: s.windowState === 'closed' || s.windowState === 'minimized' ? 'normal' : s.windowState,
    }));
    try {
      const res = await invoke('browser_open');
      const tabs = res?.tabs ?? [];
      set((s) => ({
        running: !!res?.running,
        tabs,
        activeTabId: resolveActive(tabs, s.activeTabId),
      }));
    } catch (e) {
      console.error('browser_open failed', e);
    } finally {
      set({ busy: false });
    }
  },

  // Open the window focused on an existing tab (from the island popover).
  openTab: (id) => {
    set((s) => ({
      activeTabId: id,
      windowState: s.windowState === 'closed' || s.windowState === 'minimized' ? 'normal' : s.windowState,
    }));
  },

  newTab: async (url) => {
    set({ busy: true });
    try {
      const res = await invoke('browser_new_tab', { url: url ?? null });
      const tabs = res?.tabs ?? [];
      set({
        running: true,
        tabs,
        activeTabId: res?.activeTabId ?? resolveActive(tabs, get().activeTabId),
      });
    } catch (e) {
      console.error('browser_new_tab failed', e);
    } finally {
      set({ busy: false });
    }
  },

  closeTab: async (id) => {
    try {
      const res = await invoke('browser_close_tab', { targetId: id });
      const tabs = res?.tabs ?? [];
      if (!res?.running) {
        // Closing the last tab tears the whole browser down.
        set({ running: false, tabs: [], activeTabId: null, windowState: 'closed' });
        return;
      }
      set((s) => ({ running: true, tabs, activeTabId: resolveActive(tabs, s.activeTabId) }));
    } catch (e) {
      console.error('browser_close_tab failed', e);
    }
  },

  navigate: async (id, url) => {
    // Optimistically reflect the typed URL in the tab strip / address bar.
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id ? { ...t, url } : t)),
    }));
    try {
      await invoke('browser_navigate', { targetId: id, url });
    } catch (e) {
      console.error('browser_navigate failed', e);
    }
  },

  setActiveTab: (id) => set({ activeTabId: id }),

  // Window chrome ----------------------------------------------------------

  close: async () => {
    // Close the window AND terminate Chromium (the strict lifecycle rule).
    set({ windowState: 'closed' });
    try {
      await invoke('browser_close');
    } catch (e) {
      console.error('browser_close failed', e);
    }
    set({ running: false, tabs: [], activeTabId: null });
  },

  minimize: () => set({ windowState: 'minimized' }),
  restore: () => set({ windowState: 'normal' }),
  toggleMaximize: () =>
    set((s) => ({ windowState: s.windowState === 'maximized' ? 'normal' : 'maximized' })),

  setRect: (rect) => {
    saveRect(rect);
    set({ windowRect: rect });
  },
}));
