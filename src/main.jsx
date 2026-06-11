import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles/globals.css';
import App from './App.jsx';
import { ErrorBoundary } from '@/components/error-boundary';
import { installGlobalErrorHandlers } from '@/lib/crash-logger';

// Capture uncaught errors / promise rejections to the backend log before
// anything renders, so even a crash during initial mount leaves a trace.
installGlobalErrorHandlers();

// Safety net for OS file drags: if a dragged file is dropped anywhere no
// component handles, the webview's default action is to NAVIGATE to that
// file — replacing the entire app with the dropped video/image. These
// bubble-phase listeners run after every component handler (which call
// preventDefault/stopPropagation themselves), so they only catch the
// unhandled remainder. Internal drags (tabs, tree nodes) carry no 'Files'
// type and are untouched.
window.addEventListener('dragover', (e) => {
  if (Array.from(e.dataTransfer?.types || []).includes('Files')) e.preventDefault();
});
window.addEventListener('drop', (e) => {
  if (Array.from(e.dataTransfer?.types || []).includes('Files')) e.preventDefault();
});

ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
