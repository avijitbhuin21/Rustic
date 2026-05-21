import { create } from 'zustand';

export const useClipboard = create((set) => ({
  paths: [],
  isCut: false,

  copy: (paths) => set({ paths, isCut: false }),
  cut: (paths) => set({ paths, isCut: true }),
  clear: () => set({ paths: [], isCut: false }),
}));
