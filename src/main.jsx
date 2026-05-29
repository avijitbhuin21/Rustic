import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles/globals.css';
import App from './App.jsx';
import { ErrorBoundary } from '@/components/error-boundary';
import { installGlobalErrorHandlers } from '@/lib/crash-logger';

// Capture uncaught errors / promise rejections to the backend log before
// anything renders, so even a crash during initial mount leaves a trace.
installGlobalErrorHandlers();

ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
