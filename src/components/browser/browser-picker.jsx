import React from 'react';
import { Globe, Plus, ExternalLink, Copy, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useBrowser } from '@/state/browser';
import { useTunnels } from '@/state/tunnels';
import { tabExternalUrl } from '@/lib/proxy-url';

// Web-only: the embedded VM browser picker. Lists open Chromium tabs and lets
// the user open the window or spawn a new tab, plus any live public tunnels.
// Shared by the desktop activity bar and the phone/tablet shells so all three
// get the same "see what's open first, then pick" flow.
//
// `fullscreen` makes the open actions maximize the window — the right default
// on touch layouts, where the floating draggable window chrome doesn't fit and
// the default rect would land off-screen.
export function BrowserPicker({ onClose, fullscreen = false }) {
  const running = useBrowser((s) => s.running);
  const tabs = useBrowser((s) => s.tabs);
  const previewDomain = useBrowser((s) => s.previewDomain);

  const openTab = (id) => {
    useBrowser.getState().openTab(id);
    if (fullscreen) useBrowser.getState().maximize();
    onClose?.();
  };
  const newTab = async () => {
    onClose?.();
    // `open` ensures Chromium is up, the window is visible, and ≥1 tab exists.
    // When already running, add a fresh tab instead.
    if (running) {
      await useBrowser.getState().newTab();
      if (fullscreen) useBrowser.getState().maximize();
    } else {
      await (fullscreen ? useBrowser.getState().openMaximized() : useBrowser.getState().open());
    }
  };

  return (
    <div className="flex flex-col gap-0.5">
      <p className="mb-1 px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        Browser
      </p>
      {running && tabs.length > 0 ? (
        tabs.map((tab) => (
          <div
            key={tab.id}
            className="group flex w-full items-center gap-2 rounded-md pr-1 transition-colors hover:bg-muted"
          >
            <button
              onClick={() => openTab(tab.id)}
              className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left"
            >
              {tab.favicon ? (
                <img src={tab.favicon} alt="" className="size-3.5 shrink-0 rounded-sm" />
              ) : (
                <Globe className="size-3.5 shrink-0 text-primary/70" />
              )}
              <span className="truncate text-xs text-foreground">
                {tab.title || tab.url || 'New tab'}
              </span>
            </button>
            {tabExternalUrl(tab.url, previewDomain) && (
              <button
                title="Open in my browser"
                onClick={() => useBrowser.getState().openExternal(tab)}
                className="shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground"
              >
                <ExternalLink className="size-3.5" />
              </button>
            )}
          </div>
        ))
      ) : (
        <p className="px-2 py-1 text-xs text-muted-foreground">No tabs open</p>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="mt-0.5 w-full justify-start gap-2 text-xs"
        onClick={newTab}
      >
        <Plus className="size-3.5 shrink-0" />
        New tab
      </Button>
      <TunnelList />
    </div>
  );
}

// Web-only: live list of public Cloudflare tunnels (auto-exposed dev servers +
// any opened manually). The persistent place to find a tunnel URL again, with
// copy + open + stop. Hidden when there are none.
function TunnelList() {
  const tunnels = useTunnels((s) => s.tunnels);
  if (!tunnels.length) return null;
  return (
    <div className="mt-2 border-t border-white/[0.06] pt-2">
      <p className="mb-1 px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        Public tunnels
      </p>
      {tunnels.map((t) => (
        <div
          key={t.port}
          className="group flex items-center gap-2 rounded-md px-2 py-1 transition-colors hover:bg-muted"
        >
          <span className="w-9 shrink-0 text-[11px] tabular-nums text-muted-foreground">
            :{t.port}
          </span>
          <a
            href={t.url}
            target="_blank"
            rel="noopener noreferrer"
            title={t.url}
            className="min-w-0 flex-1 truncate text-xs text-primary hover:underline"
          >
            {t.url.replace(/^https?:\/\//, '')}
          </a>
          <button
            title="Copy URL"
            onClick={() => navigator.clipboard?.writeText(t.url)}
            className="shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground"
          >
            <Copy className="size-3.5" />
          </button>
          <button
            title="Stop tunnel"
            onClick={() => useTunnels.getState().close(t.port)}
            className="shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground"
          >
            <X className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}
