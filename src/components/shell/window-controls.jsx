import React, { useEffect, useState } from 'react';
import { Minus, Square, X, Copy, Maximize2, Minimize2 } from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { IS_MAC_DESKTOP } from '@/lib/platform';

export const WINDOW_CONTROLS_WIDTH = 130;
// Width plus a small breathing gap — what layouts reserve so their top-right
// buttons clear the fixed OS window controls.
export const WINDOW_CONTROLS_OFFSET = WINDOW_CONTROLS_WIDTH + 8;

export function WindowControls() {
  // On macOS the middle button drives native fullscreen (its own Space)
  // instead of maximize — the window stays borderless for square corners.
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    // Simple fullscreen isn't reported by isFullscreen(); the toggle command's
    // return value is the source of truth on macOS.
    if (IS_MAC_DESKTOP) return undefined;
    const win = getCurrentWindow();
    let unlisten;
    win.isMaximized().then(setExpanded).catch(() => {});
    win
      .onResized(() => {
        win.isMaximized().then(setExpanded).catch(() => {});
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
  const onToggleMax = () => {
    if (IS_MAC_DESKTOP) {
      invoke('toggle_simple_fullscreen').then(setExpanded).catch(() => {});
    } else {
      getCurrentWindow().toggleMaximize().catch(() => {});
    }
  };
  const onClose = () => getCurrentWindow().close().catch(() => {});

  const toggleLabel = IS_MAC_DESKTOP
    ? (expanded ? 'Exit Full Screen' : 'Enter Full Screen')
    : (expanded ? 'Restore' : 'Maximize');
  let toggleIcon;
  if (IS_MAC_DESKTOP) toggleIcon = expanded ? <Minimize2 className="size-3" /> : <Maximize2 className="size-3" />;
  else toggleIcon = expanded ? <Copy className="size-3 -scale-x-100" /> : <Square className="size-3" />;

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
          aria-label={toggleLabel}
          title={toggleLabel}
          className="flex w-11 items-center justify-center text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          {toggleIcon}
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
