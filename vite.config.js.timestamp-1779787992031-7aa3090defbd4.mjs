// vite.config.js
import { defineConfig } from "file:///D:/Programming/Projects/Personal/Rustic/node_modules/vite/dist/node/index.js";
import react from "file:///D:/Programming/Projects/Personal/Rustic/node_modules/@vitejs/plugin-react/dist/index.js";
import tailwindcss from "file:///D:/Programming/Projects/Personal/Rustic/node_modules/@tailwindcss/vite/dist/index.mjs";
import path from "node:path";
var __vite_injected_original_dirname = "D:\\Programming\\Projects\\Personal\\Rustic";
var vite_config_default = defineConfig({
  root: "src",
  clearScreen: false,
  plugins: [
    react(),
    tailwindcss()
  ],
  resolve: {
    alias: {
      "@": path.resolve(__vite_injected_original_dirname, "./src")
    }
  },
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    outDir: "../dist",
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return void 0;
          if (id.includes("pdfjs-dist")) return "pdf";
          if (id.includes("xlsx")) return "xlsx";
          if (id.includes("docx-preview")) return "docx";
          if (id.includes("xterm")) return "xterm";
          if (id.includes("marked") || id.includes("dompurify")) return "markdown";
          if (id.includes("@codemirror") || id.includes("@uiw/react-codemirror")) return "codemirror";
          if (id.includes("monaco-editor") || id.includes("@monaco-editor/react")) return "monaco";
          if (id.includes("cmdk")) return "cmdk";
          if (id.includes("react-diff-view") || id.includes("unidiff")) return "diff";
          if (id.includes("lucide-react")) return "icons";
          if (id.includes("@radix-ui") || id.includes("radix-ui")) return "radix";
          if (id.includes("@tauri-apps")) return "tauri";
          return "vendor";
        }
      }
    }
  }
});
export {
  vite_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZS5jb25maWcuanMiXSwKICAic291cmNlc0NvbnRlbnQiOiBbImNvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9kaXJuYW1lID0gXCJEOlxcXFxQcm9ncmFtbWluZ1xcXFxQcm9qZWN0c1xcXFxQZXJzb25hbFxcXFxSdXN0aWNcIjtjb25zdCBfX3ZpdGVfaW5qZWN0ZWRfb3JpZ2luYWxfZmlsZW5hbWUgPSBcIkQ6XFxcXFByb2dyYW1taW5nXFxcXFByb2plY3RzXFxcXFBlcnNvbmFsXFxcXFJ1c3RpY1xcXFx2aXRlLmNvbmZpZy5qc1wiO2NvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9pbXBvcnRfbWV0YV91cmwgPSBcImZpbGU6Ly8vRDovUHJvZ3JhbW1pbmcvUHJvamVjdHMvUGVyc29uYWwvUnVzdGljL3ZpdGUuY29uZmlnLmpzXCI7aW1wb3J0IHsgZGVmaW5lQ29uZmlnIH0gZnJvbSAndml0ZSc7XG5pbXBvcnQgcmVhY3QgZnJvbSAnQHZpdGVqcy9wbHVnaW4tcmVhY3QnO1xuaW1wb3J0IHRhaWx3aW5kY3NzIGZyb20gJ0B0YWlsd2luZGNzcy92aXRlJztcbmltcG9ydCBwYXRoIGZyb20gJ25vZGU6cGF0aCc7XG5cbmV4cG9ydCBkZWZhdWx0IGRlZmluZUNvbmZpZyh7XG4gIHJvb3Q6ICdzcmMnLFxuICBjbGVhclNjcmVlbjogZmFsc2UsXG4gIHBsdWdpbnM6IFtcbiAgICByZWFjdCgpLFxuICAgIHRhaWx3aW5kY3NzKCksXG4gIF0sXG4gIHJlc29sdmU6IHtcbiAgICBhbGlhczoge1xuICAgICAgJ0AnOiBwYXRoLnJlc29sdmUoX19kaXJuYW1lLCAnLi9zcmMnKSxcbiAgICB9LFxuICB9LFxuICBzZXJ2ZXI6IHsgcG9ydDogMTQyMCwgc3RyaWN0UG9ydDogdHJ1ZSB9LFxuICBlbnZQcmVmaXg6IFsnVklURV8nLCAnVEFVUklfJ10sXG4gIGJ1aWxkOiB7XG4gICAgdGFyZ2V0OiAnZXNuZXh0JyxcbiAgICBvdXREaXI6ICcuLi9kaXN0JyxcbiAgICByb2xsdXBPcHRpb25zOiB7XG4gICAgICBvdXRwdXQ6IHtcbiAgICAgICAgbWFudWFsQ2h1bmtzKGlkKSB7XG4gICAgICAgICAgaWYgKCFpZC5pbmNsdWRlcygnbm9kZV9tb2R1bGVzJykpIHJldHVybiB1bmRlZmluZWQ7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdwZGZqcy1kaXN0JykpIHJldHVybiAncGRmJztcbiAgICAgICAgICBpZiAoaWQuaW5jbHVkZXMoJ3hsc3gnKSkgcmV0dXJuICd4bHN4JztcbiAgICAgICAgICBpZiAoaWQuaW5jbHVkZXMoJ2RvY3gtcHJldmlldycpKSByZXR1cm4gJ2RvY3gnO1xuICAgICAgICAgIGlmIChpZC5pbmNsdWRlcygneHRlcm0nKSkgcmV0dXJuICd4dGVybSc7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdtYXJrZWQnKSB8fCBpZC5pbmNsdWRlcygnZG9tcHVyaWZ5JykpIHJldHVybiAnbWFya2Rvd24nO1xuICAgICAgICAgIGlmIChpZC5pbmNsdWRlcygnQGNvZGVtaXJyb3InKSB8fCBpZC5pbmNsdWRlcygnQHVpdy9yZWFjdC1jb2RlbWlycm9yJykpIHJldHVybiAnY29kZW1pcnJvcic7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdtb25hY28tZWRpdG9yJykgfHwgaWQuaW5jbHVkZXMoJ0Btb25hY28tZWRpdG9yL3JlYWN0JykpIHJldHVybiAnbW9uYWNvJztcbiAgICAgICAgICBpZiAoaWQuaW5jbHVkZXMoJ2NtZGsnKSkgcmV0dXJuICdjbWRrJztcbiAgICAgICAgICBpZiAoaWQuaW5jbHVkZXMoJ3JlYWN0LWRpZmYtdmlldycpIHx8IGlkLmluY2x1ZGVzKCd1bmlkaWZmJykpIHJldHVybiAnZGlmZic7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdsdWNpZGUtcmVhY3QnKSkgcmV0dXJuICdpY29ucyc7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdAcmFkaXgtdWknKSB8fCBpZC5pbmNsdWRlcygncmFkaXgtdWknKSkgcmV0dXJuICdyYWRpeCc7XG4gICAgICAgICAgaWYgKGlkLmluY2x1ZGVzKCdAdGF1cmktYXBwcycpKSByZXR1cm4gJ3RhdXJpJztcbiAgICAgICAgICByZXR1cm4gJ3ZlbmRvcic7XG4gICAgICAgIH0sXG4gICAgICB9LFxuICAgIH0sXG4gIH0sXG59KTtcbiJdLAogICJtYXBwaW5ncyI6ICI7QUFBaVQsU0FBUyxvQkFBb0I7QUFDOVUsT0FBTyxXQUFXO0FBQ2xCLE9BQU8saUJBQWlCO0FBQ3hCLE9BQU8sVUFBVTtBQUhqQixJQUFNLG1DQUFtQztBQUt6QyxJQUFPLHNCQUFRLGFBQWE7QUFBQSxFQUMxQixNQUFNO0FBQUEsRUFDTixhQUFhO0FBQUEsRUFDYixTQUFTO0FBQUEsSUFDUCxNQUFNO0FBQUEsSUFDTixZQUFZO0FBQUEsRUFDZDtBQUFBLEVBQ0EsU0FBUztBQUFBLElBQ1AsT0FBTztBQUFBLE1BQ0wsS0FBSyxLQUFLLFFBQVEsa0NBQVcsT0FBTztBQUFBLElBQ3RDO0FBQUEsRUFDRjtBQUFBLEVBQ0EsUUFBUSxFQUFFLE1BQU0sTUFBTSxZQUFZLEtBQUs7QUFBQSxFQUN2QyxXQUFXLENBQUMsU0FBUyxRQUFRO0FBQUEsRUFDN0IsT0FBTztBQUFBLElBQ0wsUUFBUTtBQUFBLElBQ1IsUUFBUTtBQUFBLElBQ1IsZUFBZTtBQUFBLE1BQ2IsUUFBUTtBQUFBLFFBQ04sYUFBYSxJQUFJO0FBQ2YsY0FBSSxDQUFDLEdBQUcsU0FBUyxjQUFjLEVBQUcsUUFBTztBQUN6QyxjQUFJLEdBQUcsU0FBUyxZQUFZLEVBQUcsUUFBTztBQUN0QyxjQUFJLEdBQUcsU0FBUyxNQUFNLEVBQUcsUUFBTztBQUNoQyxjQUFJLEdBQUcsU0FBUyxjQUFjLEVBQUcsUUFBTztBQUN4QyxjQUFJLEdBQUcsU0FBUyxPQUFPLEVBQUcsUUFBTztBQUNqQyxjQUFJLEdBQUcsU0FBUyxRQUFRLEtBQUssR0FBRyxTQUFTLFdBQVcsRUFBRyxRQUFPO0FBQzlELGNBQUksR0FBRyxTQUFTLGFBQWEsS0FBSyxHQUFHLFNBQVMsdUJBQXVCLEVBQUcsUUFBTztBQUMvRSxjQUFJLEdBQUcsU0FBUyxlQUFlLEtBQUssR0FBRyxTQUFTLHNCQUFzQixFQUFHLFFBQU87QUFDaEYsY0FBSSxHQUFHLFNBQVMsTUFBTSxFQUFHLFFBQU87QUFDaEMsY0FBSSxHQUFHLFNBQVMsaUJBQWlCLEtBQUssR0FBRyxTQUFTLFNBQVMsRUFBRyxRQUFPO0FBQ3JFLGNBQUksR0FBRyxTQUFTLGNBQWMsRUFBRyxRQUFPO0FBQ3hDLGNBQUksR0FBRyxTQUFTLFdBQVcsS0FBSyxHQUFHLFNBQVMsVUFBVSxFQUFHLFFBQU87QUFDaEUsY0FBSSxHQUFHLFNBQVMsYUFBYSxFQUFHLFFBQU87QUFDdkMsaUJBQU87QUFBQSxRQUNUO0FBQUEsTUFDRjtBQUFBLElBQ0Y7QUFBQSxFQUNGO0FBQ0YsQ0FBQzsiLAogICJuYW1lcyI6IFtdCn0K
