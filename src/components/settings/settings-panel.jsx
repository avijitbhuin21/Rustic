import React, { useState, useRef, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { X, Settings2, Code2, Paintbrush, Keyboard, Sparkles, Search } from 'lucide-react';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import { useSettings } from '@/state/settings';
import { useLayout } from '@/state/layout';
import { GeneralSettings } from './general-settings';
import { EditorSettings } from './editor-settings';
import { AppearanceSettings } from './appearance-settings';
import { AgentSettings } from './agent-settings';
import { ShortcutsSettings } from './shortcuts-settings';
import { SettingsFilterProvider } from './setting-row';
import { useBreakpoint } from '@/lib/use-breakpoint';

const TABS = [
  { id: 'general',    label: 'General',    icon: Settings2  },
  { id: 'editor',     label: 'Editor',     icon: Code2      },
  { id: 'appearance', label: 'Appearance', icon: Paintbrush },
  { id: 'shortcuts',  label: 'Shortcuts',  icon: Keyboard   },
  { id: 'agent',      label: 'Agent',      icon: Sparkles   },
];

const TAB_ORDER = TABS.map((t) => t.id);

const slideVariants = {
  enter: (dir) => ({ x: dir * 32, opacity: 0 }),
  center: {
    x: 0,
    opacity: 1,
    transition: { duration: 0.2, ease: [0.25, 0.46, 0.45, 0.94] },
  },
  exit: (dir) => ({
    x: dir * -32,
    opacity: 0,
    transition: { duration: 0.14, ease: [0.36, 0, 0.66, 0] },
  }),
};

function tabContent(id) {
  switch (id) {
    case 'general':    return <GeneralSettings />;
    case 'editor':     return <EditorSettings />;
    case 'appearance': return <AppearanceSettings />;
    case 'shortcuts':  return <ShortcutsSettings />;
    case 'agent':      return <AgentSettings />;
    default:           return null;
  }
}

export function SettingsPanel({ onClose } = {}) {
  const load     = useSettings((s) => s.load);
  const loading  = useSettings((s) => s.loading);
  const settings = useSettings((s) => s.settings);
  const error    = useSettings((s) => s.error);

  // Read the deep-link target lazily so callers that don't supply one fall
  // back to General. The panel only mounts when the modal opens (Radix
  // unmounts content when closed), so this initializer runs fresh each open.
  const [activeTab, setActiveTab] = useState(
    () => useLayout.getState().settingsInitialTab || 'general',
  );
  const [query, setQuery] = useState('');
  const dirRef = useRef(0);
  const { isPhone } = useBreakpoint();

  useEffect(() => {
    if (!settings) load();
  }, [settings, load]);

  // One-shot consumption of the deep-link target. Cleared after first mount
  // so tab-switching back to a section later doesn't re-expand it.
  useEffect(() => {
    const s = useLayout.getState();
    if (s.settingsInitialTab || s.settingsInitialSection) {
      useLayout.setState({
        settingsInitialTab: null,
        settingsInitialSection: null,
      });
    }
  }, []);

  function switchTab(id) {
    if (id === activeTab) return;
    dirRef.current = TAB_ORDER.indexOf(id) > TAB_ORDER.indexOf(activeTab) ? 1 : -1;
    setActiveTab(id);
  }

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      {isPhone ? (
        <div className="flex shrink-0 flex-col border-b border-border/60">
          <div className="flex h-11 items-center justify-between px-4">
            <span className="text-[14px] font-semibold tracking-tight text-foreground">
              Settings
            </span>
            {onClose && (
              <Button
                variant="ghost"
                size="icon-sm"
                onClick={onClose}
                className="size-7 text-muted-foreground hover:text-foreground"
              >
                <X className="size-4" />
              </Button>
            )}
          </div>
          <div className="px-4 pb-2">
            <div className="relative w-full">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search settings..."
                className="h-8 pl-7 text-[12px]"
              />
            </div>
          </div>
          {/* Horizontal tab bar replaces the desktop left nav on phones. */}
          <nav className="flex gap-1 overflow-x-auto px-2 pb-2">
            {TABS.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                onClick={() => switchTab(id)}
                className={cn(
                  'flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg px-3 py-1.5 text-[13px] transition-colors',
                  activeTab === id
                    ? 'bg-accent text-accent-foreground font-medium'
                    : 'text-muted-foreground hover:bg-accent/40 hover:text-foreground'
                )}
              >
                <Icon className="size-4 shrink-0" />
                {label}
              </button>
            ))}
          </nav>
        </div>
      ) : (
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border/60 px-5">
          <span className="w-48 shrink-0 text-[14px] font-semibold tracking-tight text-foreground">
            Settings
          </span>
          <div className="flex flex-1 justify-center">
            <div className="relative w-full max-w-md">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search settings..."
                className="h-7 pl-7 text-[12px]"
              />
            </div>
          </div>
          <div className="w-48 shrink-0 flex justify-end">
          {onClose && (
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={onClose}
              className="size-7 text-muted-foreground hover:text-foreground"
            >
              <X className="size-4" />
            </Button>
          )}
          </div>
        </div>
      )}

      {/* Body */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left nav (desktop only — phones use the horizontal tab bar above) */}
        {!isPhone && (
          <nav className="flex w-48 shrink-0 flex-col gap-0.5 border-r border-border/60 p-2.5 pt-3">
            {TABS.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                onClick={() => switchTab(id)}
                className={cn(
                  'flex items-center gap-2.5 rounded-lg px-3 py-2 text-[13px] text-left transition-colors',
                  activeTab === id
                    ? 'bg-accent text-accent-foreground font-medium'
                    : 'text-muted-foreground hover:bg-accent/40 hover:text-foreground'
                )}
              >
                <Icon className="size-4 shrink-0" />
                {label}
              </button>
            ))}
          </nav>
        )}

        {/* Animated content */}
        <div className="relative flex-1 overflow-hidden">
          {error && (
            <div className="px-5 py-2 text-xs text-destructive">Error: {error}</div>
          )}
          {loading && !settings && (
            <div className="px-5 py-5 text-xs text-muted-foreground">Loading settings…</div>
          )}
          {settings && (
            <AnimatePresence initial={false} custom={dirRef.current} mode="wait">
              <motion.div
                key={activeTab}
                custom={dirRef.current}
                variants={slideVariants}
                initial="enter"
                animate="center"
                exit="exit"
                className="absolute inset-0"
              >
                <SettingsFilterProvider value={query}>
                  {activeTab === 'shortcuts' ? (
                    // Shortcuts manages its own scroll + uses the full pane.
                    <div className="h-full p-5">
                      {tabContent(activeTab)}
                    </div>
                  ) : (
                    <ScrollArea className="h-full">
                      <div className="p-5 pb-8">
                        {tabContent(activeTab)}
                      </div>
                    </ScrollArea>
                  )}
                </SettingsFilterProvider>
              </motion.div>
            </AnimatePresence>
          )}
        </div>
      </div>
    </div>
  );
}
