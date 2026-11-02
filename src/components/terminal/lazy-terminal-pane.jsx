import React, { Suspense } from 'react';

const TerminalPaneImpl = React.lazy(() =>
  import('./terminal-pane').then((m) => ({ default: m.TerminalPane })),
);

/** TerminalPane loaded on demand so xterm and its addons stay out of the entry chunk. */
export function TerminalPane(props) {
  return (
    <Suspense fallback={<div className="h-full w-full bg-background" />}>
      <TerminalPaneImpl {...props} />
    </Suspense>
  );
}
