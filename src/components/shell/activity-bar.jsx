import React, { useState, useRef, useCallback, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Files, Search, GitBranch, Settings, SquareTerminal, FolderOpen, Globe, Plus, ExternalLink, Copy, X } from 'lucide-react';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useTerminal } from '@/state/terminal';
import { useBrowser } from '@/state/browser';
import { useTunnels } from '@/state/tunnels';
import { tabExternalUrl } from '@/lib/proxy-url';
import { useEditor } from '@/state/editor';
import { useExplorer } from '@/state/explorer';

// Mini robot-head mark that echoes the AnimatedAgentMark in the chat empty
// state — same silhouette (rounded square head + two dot eyes), no antenna,
// no animation. Inherits `currentColor` so the active/hover color transitions
// on the button still apply.
function AgentMarkIcon({ className }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={className}
    >
      <rect x="4" y="6" width="16" height="14" rx="4" />
      <circle cx="9" cy="13" r="1.25" fill="currentColor" stroke="none" />
      <circle cx="15" cy="13" r="1.25" fill="currentColor" stroke="none" />
    </svg>
  );
}

const ITEMS = [
  { id: SIDEBAR_PANELS.EXPLORER, label: 'Explorer', icon: Files },
  { id: SIDEBAR_PANELS.SEARCH, label: 'Search', icon: Search },
  { id: SIDEBAR_PANELS.SCM, label: 'Source Control', icon: GitBranch },
  { id: SIDEBAR_PANELS.AGENT, label: 'Agent', icon: AgentMarkIcon },
];

const BTN = 42;
const GAP = 4;
const INSET = 5;

const islandVariants = {
  hidden: { x: '-110%', opacity: 0 },
  visible: {
    x: 0,
    opacity: 1,
    transition: { type: 'spring', stiffness: 380, damping: 28, mass: 0.8 },
  },
  exit: {
    x: '-110%',
    opacity: 0,
    transition: { duration: 0.18, ease: [0.36, 0, 0.66, 0] },
  },
};

// Shorten a path for display: show last 2 segments only
function shortPath(p) {
  if (!p) return '';
  const norm = p.replace(/\\/g, '/');
  const parts = norm.split('/').filter(Boolean);
  return parts.length <= 2 ? norm : '…/' + parts.slice(-2).join('/');
}

function ProjectPicker({ onSelect, onClose }) {
  const projects = useExplorer((s) => s.projects);

  if (projects.length === 0) {
    return (
      <div className="flex flex-col gap-1">
        <p className="px-2 py-1 text-xs text-muted-foreground">No projects open</p>
        <Button
          variant="ghost"
          size="sm"
          className="w-full justify-start gap-2 text-xs"
          onClick={() => { onSelect(null); onClose(); }}
        >
          <SquareTerminal className="size-3.5 shrink-0" />
          Open in default directory
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-0.5">
      <p className="mb-1 px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        Open terminal in…
      </p>
      {projects.map((project) => (
        <button
          key={project.id}
          onClick={() => { onSelect(project); onClose(); }}
          className="flex w-full flex-col items-start gap-0 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-muted"
        >
          <span className="flex items-center gap-1.5 text-xs font-medium text-foreground">
            <FolderOpen className="size-3.5 shrink-0 text-primary/70" />
            {project.name}
          </span>
          <span className="ml-5 text-[11px] text-muted-foreground">{shortPath(project.root_path)}</span>
        </button>
      ))}
    </div>
  );
}

// Web-only: the embedded VM browser island popover. Lists open Chromium tabs
// and lets the user open the window or spawn a new tab. Mirrors ProjectPicker.
function BrowserPicker({ onClose }) {
  const running = useBrowser((s) => s.running);
  const tabs = useBrowser((s) => s.tabs);
  const previewDomain = useBrowser((s) => s.previewDomain);

  const openTab = (id) => {
    useBrowser.getState().openTab(id);
    onClose();
  };
  const newTab = async () => {
    onClose();
    // `open` ensures Chromium is up, the window is visible, and ≥1 tab exists.
    // When already running, add a fresh tab instead.
    if (running) await useBrowser.getState().newTab();
    else await useBrowser.getState().open();
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
                className="hidden shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground group-hover:block"
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
            className="hidden shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground group-hover:block"
          >
            <Copy className="size-3.5" />
          </button>
          <button
            title="Stop tunnel"
            onClick={() => useTunnels.getState().close(t.port)}
            className="hidden shrink-0 rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-foreground group-hover:block"
          >
            <X className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}

export function ActivityBar() {
  const activePanel = useLayout((s) => s.activeSidebarPanel);
  const sidebarVisible = useLayout((s) => s.sidebarVisible);
  const setActivePanel = useLayout((s) => s.setActiveSidebarPanel);
  const openSettings = useLayout((s) => s.openSettings);
  // Pinned-open state, driven by the status-bar toggle (for touch devices that
  // can't hover the left edge). The island shows when hovered OR pinned.
  const islandOpen = useLayout((s) => s.islandOpen);
  const [visible, setVisible] = useState(false);
  const open = visible || islandOpen;
  const [pickerOpen, setPickerOpen] = useState(false);
  const [browserPickerOpen, setBrowserPickerOpen] = useState(false);
  const hideTimerRef = useRef(null);

  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);

  useEffect(() => {
    wireListeners();
    refreshSessions();
    // NB: the embedded browser + tunnel hub listeners are wired at the App
    // level (see App.jsx) so they're live in the mobile/tablet shells too,
    // where this desktop activity bar never mounts.
  }, [wireListeners, refreshSessions]);

  const show = useCallback(() => {
    clearTimeout(hideTimerRef.current);
    setVisible(true);
  }, []);

  // Don't start the hide timer while either picker popover is open
  const scheduleHide = useCallback(() => {
    if (pickerOpen || browserPickerOpen) return;
    hideTimerRef.current = setTimeout(() => setVisible(false), 500);
  }, [pickerOpen, browserPickerOpen]);

  // When a picker closes the mouse may be over the portal (outside the island's
  // DOM), so onMouseLeave never fires. Kick off the hide timer manually here.
  useEffect(() => {
    if (!pickerOpen && !browserPickerOpen) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = setTimeout(() => setVisible(false), 600);
    }
  }, [pickerOpen, browserPickerOpen]);

  const handleProjectSelect = useCallback(async (project) => {
    try {
      const cwd = project?.root_path ?? undefined;
      const label = project?.name ?? 'shell';
      const info = await useTerminal.getState().createTerminal({ cwd, label });
      const pid = info.pid;
      const tabTitle = pid != null ? `${label} • ${pid}` : label;
      useEditor.getState().openTerminal(info.id, tabTitle);
    } catch (e) {
      console.error('Failed to create terminal', e);
    }
  }, []);

  const activeMainIndex = sidebarVisible
    ? ITEMS.findIndex(({ id }) => id === activePanel)
    : -1;

  return (
    <>
      {/* Invisible left-edge trigger strip */}
      <div
        className="fixed left-0 top-0 bottom-6 w-2 z-[60]"
        onMouseEnter={show}
        onMouseLeave={scheduleHide}
      />

      {/* Vertical centering wrapper */}
      <div className="pointer-events-none fixed left-0 top-0 bottom-6 z-50 flex items-center">
        <AnimatePresence>
          {open && (
            <motion.div
              key="island"
              variants={islandVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className={cn(
                'pointer-events-auto ml-1.5',
                'flex flex-col items-center px-1.5 py-3',
                'rounded-[14px]',
                'border border-white/[0.09]',
                'bg-background/65 backdrop-blur-2xl',
                'shadow-[0_8px_32px_rgba(0,0,0,0.55),inset_0_1px_0_rgba(255,255,255,0.05)]',
              )}
              onMouseEnter={show}
              onMouseLeave={scheduleHide}
            >
              {/* Main nav — single sliding indicator */}
              <div className="relative flex flex-col items-center gap-1">
                {activeMainIndex >= 0 && (
                  <span
                    className="absolute left-0 w-0.5 rounded-full bg-primary transition-transform duration-200 ease-out"
                    style={{
                      height: BTN - INSET * 2,
                      top: INSET,
                      transform: `translateY(${activeMainIndex * (BTN + GAP)}px)`,
                    }}
                  />
                )}
                {ITEMS.map(({ id, label, icon: Icon }) => {
                  const isActive = sidebarVisible && activePanel === id;
                  return (
                    <Tooltip key={id}>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => setActivePanel(id)}
                          className={cn(
                            'size-[42px] rounded-[10px] text-muted-foreground',
                            'hover:bg-white/10 hover:text-foreground transition-colors',
                            isActive && 'text-foreground'
                          )}
                        >
                          <Icon className="size-5" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent side="right">{label}</TooltipContent>
                    </Tooltip>
                  );
                })}
              </div>

              {/* Divider */}
              <div className="my-2 h-px w-7 rounded-full bg-white/[0.08]" />

              {/* Terminal — project picker popover */}
              <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <PopoverTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className={cn(
                          'size-[42px] rounded-[10px] text-muted-foreground',
                          'hover:bg-white/10 hover:text-foreground transition-colors',
                          pickerOpen && 'bg-white/10 text-foreground'
                        )}
                      >
                        <SquareTerminal className="size-5" />
                      </Button>
                    </PopoverTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="right">New Terminal</TooltipContent>
                </Tooltip>
                <PopoverContent
                  side="right"
                  align="center"
                  sideOffset={10}
                  className="w-60 p-2"
                  onInteractOutside={() => setPickerOpen(false)}
                >
                  <ProjectPicker
                    onSelect={handleProjectSelect}
                    onClose={() => setPickerOpen(false)}
                  />
                </PopoverContent>
              </Popover>

              {/* Browser — web/server build only (desktop has a real browser) */}
              {IS_WEB && (
                <Popover open={browserPickerOpen} onOpenChange={setBrowserPickerOpen}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <PopoverTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className={cn(
                            'mt-1 size-[42px] rounded-[10px] text-muted-foreground',
                            'hover:bg-white/10 hover:text-foreground transition-colors',
                            browserPickerOpen && 'bg-white/10 text-foreground'
                          )}
                        >
                          <Globe className="size-5" />
                        </Button>
                      </PopoverTrigger>
                    </TooltipTrigger>
                    <TooltipContent side="right">Browser</TooltipContent>
                  </Tooltip>
                  <PopoverContent
                    side="right"
                    align="center"
                    sideOffset={10}
                    className="w-60 p-2"
                    onInteractOutside={() => setBrowserPickerOpen(false)}
                  >
                    <BrowserPicker onClose={() => setBrowserPickerOpen(false)} />
                  </PopoverContent>
                </Popover>
              )}

              {/* Divider */}
              <div className="my-2 h-px w-7 rounded-full bg-white/[0.08]" />

              {/* Settings */}
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={openSettings}
                    className={cn(
                      'size-[42px] rounded-[10px] text-muted-foreground',
                      'hover:bg-white/10 hover:text-foreground transition-colors',
                    )}
                  >
                    <Settings className="size-5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="right">Settings</TooltipContent>
              </Tooltip>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </>
  );
}
