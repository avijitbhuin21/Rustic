import React, { useState, useRef, useCallback, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Files, Search, GitBranch, Settings, SquareTerminal, FolderOpen } from 'lucide-react';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useTerminal } from '@/state/terminal';
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

export function ActivityBar() {
  const activePanel = useLayout((s) => s.activeSidebarPanel);
  const sidebarVisible = useLayout((s) => s.sidebarVisible);
  const setActivePanel = useLayout((s) => s.setActiveSidebarPanel);
  const openSettings = useLayout((s) => s.openSettings);
  const [visible, setVisible] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const hideTimerRef = useRef(null);

  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);

  useEffect(() => {
    wireListeners();
    refreshSessions();
  }, [wireListeners, refreshSessions]);

  const show = useCallback(() => {
    clearTimeout(hideTimerRef.current);
    setVisible(true);
  }, []);

  // Don't start the hide timer while the project picker is open
  const scheduleHide = useCallback(() => {
    if (pickerOpen) return;
    hideTimerRef.current = setTimeout(() => setVisible(false), 500);
  }, [pickerOpen]);

  // When the picker closes the mouse may be over the portal (outside the island's
  // DOM), so onMouseLeave never fires. Kick off the hide timer manually here.
  useEffect(() => {
    if (!pickerOpen) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = setTimeout(() => setVisible(false), 600);
    }
  }, [pickerOpen]);

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
          {visible && (
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
