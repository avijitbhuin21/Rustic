import { create } from 'zustand';

export const SIDEBAR_PANELS = {
  EXPLORER: 'explorer',
  SEARCH: 'search',
  SCM: 'scm',
  AGENT: 'agent',
  SETTINGS: 'settings',
};

export const useLayout = create((set) => ({
  activeSidebarPanel: SIDEBAR_PANELS.AGENT,
  sidebarVisible: true,
  // The chat dock is independent of which sidebar panel is active — once the
  // user enters Agent mode the dock stays in place even when they switch the
  // sidebar to Explorer / Search / Source Control. Closed via the X button in
  // the chat header, reopened by clicking the Agent activity again.
  chatDockOpen: true,
  bottomPanelVisible: false,
  bottomPanelTab: 'problems',
  settingsOpen: false,
  // Optional deep-link hints consumed by SettingsPanel / individual tab
  // components when the modal opens — lets callers say "open Settings, jump
  // to Agent tab, expand the Tools section." Both reset on close so a normal
  // openSettings() doesn't inherit a stale target from a previous deep-link.
  settingsInitialTab: null,
  settingsInitialSection: null,

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
  toggleBottomPanel: () => set((state) => ({ bottomPanelVisible: !state.bottomPanelVisible })),
  setBottomPanelVisible: (v) => set({ bottomPanelVisible: v }),
  setBottomPanelTab: (tab) => set({ bottomPanelTab: tab, bottomPanelVisible: true }),
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
