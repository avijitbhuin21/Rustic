import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import path from 'node:path';

export default defineConfig({
  root: 'src',
  clearScreen: false,
  plugins: [
    react(),
    tailwindcss(),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'esnext',
    outDir: '../dist',
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined;
          if (id.includes('pdfjs-dist')) return 'pdf';
          if (id.includes('xlsx')) return 'xlsx';
          if (id.includes('docx-preview')) return 'docx';
          if (id.includes('xterm')) return 'xterm';
          if (id.includes('marked') || id.includes('dompurify')) return 'markdown';
          if (id.includes('@codemirror') || id.includes('@uiw/react-codemirror')) return 'codemirror';
          if (id.includes('monaco-editor') || id.includes('@monaco-editor/react')) return 'monaco';
          if (id.includes('cmdk')) return 'cmdk';
          if (id.includes('react-diff-view') || id.includes('unidiff')) return 'diff';
          if (id.includes('lucide-react')) return 'icons';
          if (id.includes('@radix-ui') || id.includes('radix-ui')) return 'radix';
          if (id.includes('@tauri-apps')) return 'tauri';
          return 'vendor';
        },
      },
    },
  },
});
