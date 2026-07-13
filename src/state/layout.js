import { create } from 'zustand';

export const SIDEBAR_PANELS = {
  EXPLORER: 'explorer',
  SEARCH: 'search',
  SCM: 'scm',
  AGENT: 'agent',
  SETTINGS: 'settings',
};

// Mobile (web build only) renders one view at a time. These are the values the
// bottom tab bar / phone shell switches between.
export const MOBILE_TABS = {
  AGENT: 'agent',
  EXPLORER: 'explorer',
  EDITOR: 'editor',
  TERMINAL: 'terminal',
  SEARCH: 'search',
  SCM: 'scm',
};

export const useLayout = create((set) => ({
  activeSidebarPanel: SIDEBAR_PANELS.AGENT,
  sidebarVisible: true,
  // Click-to-toggle for the left activity-bar "dynamic island" (web build).
  // The island normally reveals on hover of the screen's left edge, which is
  // impossible on touch devices (iPad/tablet) — this pins it open via a
  // status-bar button so it works without a mouse.
  islandOpen: false,
  // Same pin-open mechanism for the right-edge dock island. Toggled via a
  // status-bar button ("Pin dock") for touch devices.
  rightIslandOpen: false,
  // Which panel the right-edge floating dock is showing (null = none). Fully
  // independent from the left sidebar's activeSidebarPanel — both sides can
  // show the same or different panels at once.
  rightPanel: null,
  // The chat dock is independent of which sidebar panel is active — once the
  // user enters Agent mode the dock stays in place even when they switch the
  // sidebar to Explorer / Search / Source Control. Closed via the X button in
  // the chat header, reopened by clicking the Agent activity again.
  chatDockOpen: true,
  bottomPanelVisible: false,
  bottomPanelTab: 'problems',
  bottomPanelFullscreen: false,
  settingsOpen: false,
  // Optional deep-link hints consumed by SettingsPanel / individual tab
  // components when the modal opens — lets callers say "open Settings, jump
  // to Agent tab, expand the Tools section." Both reset on close so a normal
  // openSettings() doesn't inherit a stale target from a previous deep-link.
  settingsInitialTab: null,
  settingsInitialSection: null,

  // Mobile-only (web build). `mobileTab` is the single active view; `mobileDrawer`
  // is the open overlay on the tablet layout (null | 'sidebar' | 'chat' | 'terminal').
  mobileTab: 'agent',
  mobileDrawer: null,
  setMobileTab: (tab) => set({ mobileTab: tab, mobileDrawer: null }),
  openMobileDrawer: (drawer) => set({ mobileDrawer: drawer }),
  closeMobileDrawer: () => set({ mobileDrawer: null }),
  toggleMobileDrawer: (drawer) =>
    set((state) => ({ mobileDrawer: state.mobileDrawer === drawer ? null : drawer })),

  setActiveSidebarPanel: (panel) =>
    set((state) => {
      // Clicking the Agent activity opens the chat dock and never closes it
      // implicitly (use closeChatDock for that). Clicking other activities
      // leaves the dock untouched — that's the persistence the UI relies on.
      const patch = {
        activeSidebarPanel: panel,
        sidebarVisible: state.activeSidebarPanel === panel ? !state.sidebarVisible : true,
      };
      if (panel === SIDEBAR_PANELS.AGENT) {
        patch.chatDockOpen = true;
      }
      return patch;
    }),
  toggleSidebar: () => set((state) => ({ sidebarVisible: !state.sidebarVisible })),
  setIslandOpen: (v) => set({ islandOpen: v }),
  toggleIsland: () => set((state) => ({ islandOpen: !state.islandOpen })),
  setRightIslandOpen: (v) => set({ rightIslandOpen: v }),
  toggleRightIsland: () => set((state) => ({ rightIslandOpen: !state.rightIslandOpen })),
  toggleRightPanel: (panel) =>
    set((state) => ({ rightPanel: state.rightPanel === panel ? null : panel })),
  toggleBottomPanel: () => set((state) => ({ bottomPanelVisible: !state.bottomPanelVisible })),
  setBottomPanelVisible: (v) => set({ bottomPanelVisible: v }),
  setBottomPanelTab: (tab) => set({ bottomPanelTab: tab, bottomPanelVisible: true }),
  toggleBottomPanelFullscreen: () => set((state) => ({ bottomPanelFullscreen: !state.bottomPanelFullscreen })),
  setBottomPanelFullscreen: (v) => set({ bottomPanelFullscreen: v }),
  openChatDock: () => set({ chatDockOpen: true }),
  closeChatDock: () => set({ chatDockOpen: false }),
  toggleChatDock: () => set((state) => ({ chatDockOpen: !state.chatDockOpen })),
  openSettings: () => set({ settingsOpen: true }),
  openSettingsAt: (tab = null, section = null) =>
    set({
      settingsOpen: true,
      settingsInitialTab: tab,
      settingsInitialSection: section,
    }),
  closeSettings: () =>
    set({
      settingsOpen: false,
      settingsInitialTab: null,
      settingsInitialSection: null,
    }),
}));
