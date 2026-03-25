export default {
  root: 'src',
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_'],
  build: { target: 'esnext', outDir: '../dist' }
};
