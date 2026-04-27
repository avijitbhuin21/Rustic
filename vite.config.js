export default {
  root: 'src',
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'esnext',
    outDir: '../dist',
    rollupOptions: {
      output: {
        // Split heavy / rarely-used libs into their own chunks so the initial
        // app shell loads faster. Preview libs (pdfjs / xlsx / docx-preview)
        // are already dynamically imported, so this just stabilises their
        // chunk names; xterm and marked get their own group too.
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined;
          if (id.includes('pdfjs-dist')) return 'pdf';
          if (id.includes('xlsx')) return 'xlsx';
          if (id.includes('docx-preview')) return 'docx';
          if (id.includes('xterm')) return 'xterm';
          if (id.includes('marked') || id.includes('dompurify')) return 'markdown';
          if (id.includes('@tauri-apps')) return 'tauri';
          return 'vendor';
        },
      },
    },
  },
};
