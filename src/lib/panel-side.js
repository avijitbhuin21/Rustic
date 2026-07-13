import { createContext, useContext } from 'react';

// Identifies which side of the workbench a sidebar panel instance is mounted
// on, so per-side UI state (project expansion, section collapse) stays
// independent between the left docked sidebar and the right floating dock.
export const PanelSideContext = createContext('left');

export function usePanelSide() {
  return useContext(PanelSideContext);
}
