import React, { useEffect, useState } from 'react';
import { Minus, Square, X, Copy } from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';

export const WINDOW_CONTROLS_WIDTH = 130;
// Width plus a small breathing gap — what layouts reserve so their top-right
// buttons clear the fixed OS window controls.
export const WINDOW_CONTROLS_OFFSET = WINDOW_CONTROLS_WIDTH + 8;

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten;
    win.isMaximized().then(setMaximized).catch(() => {});
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => {});
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  const onMinimize = () => getCurrentWindow().minimize().catch(() => {});
  const onToggleMax = () => getCurrentWindow().toggleMaximize().catch(() => {});
  const onClose = () => getCurrentWindow().close().catch(() => {});

  return (
    <div
      className="fixed right-0 top-0 z-50 flex h-8 select-none items-stretch justify-end"
      style={{ width: WINDOW_CONTROLS_WIDTH }}
    >
      <div className="flex h-[31px] items-stretch bg-sidebar">
        <button
          type="button"
          onClick={onMinimize}
          aria-label="Minimize"
          className="flex w-11 items-center justify-center text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          <Minus className="size-3.5" />
        </button>
        <button
          type="button"
          onClick={onToggleMax}
          aria-label={maximized ? 'Restore' : 'Maximize'}
          className="flex w-11 items-center justify-center text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          {maximized ? <Copy className="size-3 -scale-x-100" /> : <Square className="size-3" />}
        </button>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          className="flex w-11 items-center justify-center text-muted-foreground hover:bg-red-600 hover:text-white"
        >
          <X className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
