import { create } from 'zustand';

export const SIDEBAR_PANELS = {
  EXPLORER: 'explorer',
  SEARCH: 'search',
  SCM: 'scm',
  AGENT: 'agent',
  SETTINGS: 'settings',
};

export const useLayout = create((set) => ({
  activeSidebarPanel: SIDEBAR_PANELS.EXPLORER,
  sidebarVisible: true,
  bottomPanelVisible: false,
  bottomPanelTab: 'problems',
  settingsOpen: false,

  setActiveSidebarPanel: (panel) =>
    set((state) => ({
      activeSidebarPanel: panel,
      sidebarVisible: state.activeSidebarPanel === panel ? !state.sidebarVisible : true,
    })),
  toggleSidebar: () => set((state) => ({ sidebarVisible: !state.sidebarVisible })),
  toggleBottomPanel: () => set((state) => ({ bottomPanelVisible: !state.bottomPanelVisible })),
  setBottomPanelTab: (tab) => set({ bottomPanelTab: tab, bottomPanelVisible: true }),
  openSettings: () => set({ settingsOpen: true }),
  closeSettings: () => set({ settingsOpen: false }),
}));
