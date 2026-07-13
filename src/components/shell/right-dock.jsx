import React, { useState, useRef, useCallback, useEffect } from 'react';
import { motion } from 'framer-motion';
import { Files, Search, GitBranch } from 'lucide-react';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useLayout, SIDEBAR_PANELS } from '@/state/layout';
import { useEditor } from '@/state/editor';
import { PanelSideContext } from '@/lib/panel-side';
import { Explorer } from '@/components/explorer/explorer';
import { SearchPanel } from '@/components/search/search-panel';
import ScmPanel from '@/components/scm/scm-panel';
import { AgentTaskTree } from '@/components/agent/agent-task-tree';
import { AgentMarkIcon } from '@/components/shell/activity-bar';

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
  hidden: {
    x: '110%',
    opacity: 0,
    transition: { duration: 0.18, ease: [0.36, 0, 0.66, 0] },
    transitionEnd: { visibility: 'hidden' },
  },
  visible: {
    x: 0,
    opacity: 1,
    visibility: 'visible',
    transition: { type: 'spring', stiffness: 380, damping: 28, mass: 0.8 },
  },
};

function openFileInEditor(path, opts) {
  try { useEditor.getState().openFile(path, opts); } catch {}
}

function panelComponent(id) {
  switch (id) {
    case SIDEBAR_PANELS.EXPLORER:  return <Explorer onOpenFile={openFileInEditor} />;
    case SIDEBAR_PANELS.SEARCH:    return <SearchPanel onOpenFile={openFileInEditor} />;
    case SIDEBAR_PANELS.SCM:       return <ScmPanel />;
    case SIDEBAR_PANELS.AGENT:     return <AgentTaskTree />;
    default: return null;
  }
}

// Right-edge floating "dynamic island" mirroring the left activity bar.
// Reveals on hover of the screen's right edge (or pinned open via the
// status-bar "Pin dock" toggle). Clicking an icon expands a floating panel
// leftward out of the island — an overlay on top of the editor/chat, second
// instance of the same panel components the left sidebar hosts. Panels are
// mounted once on first visit and kept alive (hidden) so they never remount.
export function RightDock() {
  const rightIslandOpen = useLayout((s) => s.rightIslandOpen);
  const rightPanel = useLayout((s) => s.rightPanel);
  const toggleRightPanel = useLayout((s) => s.toggleRightPanel);
  const [visible, setVisible] = useState(false);
  const hideTimerRef = useRef(null);

  const open = visible || rightIslandOpen;
  const panelOpen = open && rightPanel != null;

  const [everMounted, setEverMounted] = useState(() => new Set());
  useEffect(() => {
    if (rightPanel == null) return;
    setEverMounted((prev) => {
      if (prev.has(rightPanel)) return prev;
      const next = new Set(prev);
      next.add(rightPanel);
      return next;
    });
  }, [rightPanel]);

  const show = useCallback(() => {
    clearTimeout(hideTimerRef.current);
    setVisible(true);
  }, []);

  const scheduleHide = useCallback(() => {
    hideTimerRef.current = setTimeout(() => setVisible(false), 500);
  }, []);

  useEffect(() => () => clearTimeout(hideTimerRef.current), []);

  const activeIndex = panelOpen
    ? ITEMS.findIndex(({ id }) => id === rightPanel)
    : -1;

  return (
    <>
      {/* Right-edge trigger strip — the sliver is the always-visible hint. */}
      <div
        className="group fixed right-0 top-0 bottom-6 z-[60] flex w-2 items-center justify-end"
        onMouseEnter={show}
        onMouseLeave={scheduleHide}
        onClick={show}
      >
        <div
          aria-hidden
          className={cn(
            'h-12 w-[3px] rounded-l-full bg-primary/40 transition-all duration-200',
            'group-hover:h-16 group-hover:bg-primary/80',
            open && 'opacity-0',
          )}
        />
      </div>

      {/* Vertical centering wrapper. Island and panel render as ONE connected
          unit: island on the left, panel growing out of its right edge toward
          the screen edge. When the panel opens the island slides left because
          the panel's width animates inside the same right-anchored flex row.
          The unit stays MOUNTED while hidden (translated off-screen) so panel
          state — expanded folders, scroll positions — survives hide/reveal. */}
      <div className="pointer-events-none fixed right-0 top-0 bottom-6 z-50 flex items-center justify-end overflow-hidden">
        <motion.div
          variants={islandVariants}
          initial={false}
          animate={open ? 'visible' : 'hidden'}
          className={cn('mr-1.5 flex items-center', open ? 'pointer-events-auto' : 'pointer-events-none')}
          aria-hidden={!open}
          onMouseEnter={show}
          onMouseLeave={scheduleHide}
        >
          {/* Island */}
          <div
            className={cn(
              'flex flex-col items-center px-1.5 py-3',
              'border border-white/[0.09]',
              'bg-background/80 backdrop-blur-2xl',
              'shadow-[0_8px_32px_rgba(0,0,0,0.55),inset_0_1px_0_rgba(255,255,255,0.05)]',
              'transition-[border-radius] duration-200',
              panelOpen ? 'rounded-l-[14px] rounded-r-none border-r-0' : 'rounded-[14px]',
            )}
          >
            <div className="relative flex flex-col items-center gap-1">
              {activeIndex >= 0 && (
                <span
                  className="absolute -right-1.5 w-0.5 rounded-full bg-primary transition-transform duration-200 ease-out"
                  style={{
                    height: BTN - INSET * 2,
                    top: INSET,
                    transform: `translateY(${activeIndex * (BTN + GAP)}px)`,
                  }}
                />
              )}
              {ITEMS.map(({ id, label, icon: Icon }) => {
                const isActive = rightPanel === id;
                return (
                  <Tooltip key={id}>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => toggleRightPanel(id)}
                        className={cn(
                          'size-[42px] rounded-[10px] text-muted-foreground',
                          'hover:bg-white/10 hover:text-foreground transition-colors',
                          isActive && 'bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary'
                        )}
                      >
                        <Icon className="size-5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="left">{label}</TooltipContent>
                  </Tooltip>
                );
              })}
            </div>
          </div>

          {/* Panel — width animates so the island glides left as it opens.
              Content stays mounted at zero width so its state persists. */}
          <motion.div
            initial={false}
            animate={
              rightPanel != null
                ? {
                    width: 288,
                    opacity: 1,
                    transition: { type: 'spring', stiffness: 380, damping: 32, mass: 0.8 },
                  }
                : {
                    width: 0,
                    opacity: 0,
                    transition: { duration: 0.18, ease: [0.36, 0, 0.66, 0] },
                  }
            }
            className={cn(
              'h-[80vh] max-h-[calc(100vh-4rem)] overflow-hidden',
              'rounded-[14px] rounded-l-none',
              'border border-l-0 border-white/[0.09]',
              'bg-background/80 backdrop-blur-2xl',
              'shadow-[0_8px_32px_rgba(0,0,0,0.55),inset_0_1px_0_rgba(255,255,255,0.05)]',
            )}
          >
            <PanelSideContext.Provider value="right">
              {/* Panels like the agent tree paint their own bg-sidebar (matches
                  the left SidebarHost); inside the glassy dock that reads as a
                  lighter gray, so neutralize it and let the dock bg show. */}
              <div className="h-full w-72 [&_.bg-sidebar]:bg-transparent">
                {ITEMS.map(({ id }) => {
                  if (!everMounted.has(id)) return null;
                  return (
                    <div key={id} className={cn('h-full w-full', rightPanel !== id && 'hidden')}>
                      {panelComponent(id)}
                    </div>
                  );
                })}
              </div>
            </PanelSideContext.Provider>
          </motion.div>
        </motion.div>
      </div>
    </>
  );
}
