import React from 'react';
import { logReactError } from '@/lib/crash-logger';

// Catches render/lifecycle errors anywhere below it, logs them to the backend
// rolling log (so a crash leaves a trace), and shows a minimal recovery screen
// instead of a blank white window.
export class ErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    logReactError(error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-screen w-screen flex-col items-center justify-center gap-3 bg-background p-6 text-center text-foreground">
          <div className="text-lg font-semibold">Something went wrong</div>
          <div className="max-w-lg whitespace-pre-wrap break-words text-sm text-muted-foreground">
            {String(this.state.error?.message || this.state.error)}
          </div>
          <button
            type="button"
            className="rounded bg-primary px-3 py-1.5 text-sm text-primary-foreground hover:opacity-90"
            onClick={() => {
              this.setState({ error: null });
              try {
                window.location.reload();
              } catch {}
            }}
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

export default ErrorBoundary;
