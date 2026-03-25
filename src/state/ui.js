import { createStore } from './store.js';

export const uiStore = createStore({
  activePanel: 'explorer',
  primarySidebarVisible: true,
  bottomPanelVisible: true,
  secondarySidebarVisible: false,
  sidebarWidth: 260,
  panelHeight: 200,
  secondarySidebarWidth: 350,
});
