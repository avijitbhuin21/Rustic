import { createStore } from './store.js';

export const uiStore = createStore({
  activePanel: 'agent',
  primarySidebarVisible: true,
  bottomPanelVisible: false,
  secondarySidebarVisible: false,
  sidebarWidth: 260,
  panelHeight: 200,
  secondarySidebarWidth: 350,
});
