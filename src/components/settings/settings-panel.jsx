import React, { useMemo, useState, useRef, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  X, Settings2, Code2, Paintbrush, Keyboard, Sparkles, Search,
  Wrench, Library, Gauge, ChevronRight,
} from 'lucide-react';
import { GithubIcon } from '@/components/github/icon';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import { useSettings } from '@/state/settings';
import { useLayout } from '@/state/layout';
import { IS_WEB } from '@/lib/platform';
import { GeneralSettings } from './general-settings';
import { EditorSettings } from './editor-settings';
import { AppearanceSettings } from './appearance-settings';
import {
  AgentProvidersTab, AgentToolsTab, AgentLibraryTab, AgentModelsTab, AgentGithubTab,
} from './agent-settings';
import { ShortcutsSettings } from './shortcuts-settings';
import { useBreakpoint } from '@/lib/use-breakpoint';

const NAV_GROUPS = [
  {
    label: 'Workspace',
    items: [
      { id: 'general',    label: 'General',    icon: Settings2  },
      { id: 'editor',     label: 'Editor',     icon: Code2      },
      { id: 'appearance', label: 'Appearance', icon: Paintbrush },
      { id: 'shortcuts',  label: 'Shortcuts',  icon: Keyboard   },
    ],
  },
  {
    label: 'Agent',
    items: [
      { id: 'agent-providers', label: 'Providers',       icon: Sparkles },
      { id: 'agent-tools',     label: 'Tools & MCP',     icon: Wrench   },
      { id: 'agent-library',   label: 'Library',         icon: Library  },
      { id: 'agent-models',    label: 'Models & Budget', icon: Gauge    },
      ...(IS_WEB ? [{ id: 'agent-github', label: 'GitHub', icon: GithubIcon }] : []),
    ],
  },
];

const TABS = NAV_GROUPS.flatMap((g) => g.items);
const TAB_ORDER = TABS.map((t) => t.id);
const TAB_LABELS = Object.fromEntries(TABS.map((t) => [t.id, t.label]));

// Static search index: every section across every tab, with its row labels /
// salient terms as keywords. Anchors must match the data-settings-anchor slugs
// emitted by Section / SettingsSection (lowercased title, non-alnum → '-').
const SEARCH_INDEX = [
  { tab: 'general', section: 'Auto Save & UI', anchor: 'auto-save-ui', keywords: ['Auto Save', 'Auto Save Delay', 'UI Scale', 'Zoom'] },
  { tab: 'general', section: 'Startup', anchor: 'startup', keywords: ['Restore last session', 'Confirm before quit'] },
  { tab: 'general', section: 'Session & Power', anchor: 'session-power', keywords: ['Keep session alive', 'Idle timeout', 'Logout'], web: true },
  { tab: 'general', section: 'Preview Tunnel', anchor: 'preview-tunnel', keywords: ['Open-in-browser mode', 'Auto-expose dev servers', 'Preview domain', 'Cookie domain', 'Port forward'], web: true },
  { tab: 'editor', section: 'Tab & Indentation', anchor: 'tab-indentation', keywords: ['Tab Size', 'Insert Spaces', 'Auto Indent'] },
  { tab: 'editor', section: 'Display', anchor: 'display', keywords: ['Word Wrap', 'Line Numbers', 'Minimap', 'Render Whitespace', 'Show Zero-Width Characters', 'Bracket Pair Colorization', 'Format on Save', 'Formatters', 'Sticky Scroll', 'Smooth Scrolling', 'Indent Guides'] },
  { tab: 'editor', section: 'Cursor', anchor: 'cursor', keywords: ['Cursor Blink', 'Cursor Style', 'Smooth Caret Animation'] },
  { tab: 'appearance', section: 'Fonts', anchor: 'fonts', keywords: ['Font family', 'Google Fonts', 'Load font', 'Apply font'] },
  { tab: 'appearance', section: 'Color Palette', anchor: 'color-palette', keywords: ['Theme', 'Import theme', 'Dark', 'Light', 'Colors'] },
  { tab: 'shortcuts', section: 'Keyboard Shortcuts', anchor: null, keywords: ['Keybinding', 'Remap', 'Hotkey', 'Import keybindings', 'Reset keybindings'] },
  { tab: 'agent-providers', section: 'AI Providers', anchor: 'ai-providers', keywords: ['Anthropic', 'Claude', 'OpenAI', 'Gemini', 'OpenRouter', 'FreeBuff', 'API key', 'Model', 'OpenAI-compatible', 'Base URL', 'Connect'] },
  { tab: 'agent-tools', section: 'Tools', anchor: 'tools', keywords: ['Web Search', 'Web Fetch', 'Tavily', 'Image creator', 'Video creator', 'Animator', 'Media', 'image_create', 'video_create', 'animate'] },
  { tab: 'agent-tools', section: 'MCP Servers', anchor: 'mcp-servers', keywords: ['MCP', 'Server', 'mcp.json', 'Transport'] },
  { tab: 'agent-library', section: 'Skills', anchor: 'skills', keywords: ['Skill', 'Add skill'] },
  { tab: 'agent-library', section: 'Workflows', anchor: 'workflows', keywords: ['Workflow', 'Add workflow'] },
  { tab: 'agent-library', section: 'Rules', anchor: 'rules', keywords: ['Rule', 'Global rules', 'Project rules'] },
  { tab: 'agent-models', section: 'Sub Agent', anchor: 'sub-agent', keywords: ['Sub-agent model', 'Fast model', 'Routing', 'Cheaper model'] },
  { tab: 'agent-models', section: 'Audio Input', anchor: 'audio-input', keywords: ['Speech to text', 'Whisper', 'Transcribe', 'Microphone', 'Voice', 'Dictation'] },
  { tab: 'agent-models', section: 'Source Control', anchor: 'source-control', keywords: ['Commit message', 'Generate commit', 'AI commit', 'Conventional commits', 'Git message'] },
  { tab: 'agent-models', section: 'Budget', anchor: 'budget', keywords: ['Spend limit', 'Cost', 'Parallel tasks', 'Token limit'] },
  { tab: 'agent-models', section: 'Worktrees', anchor: 'worktrees', keywords: ['Worktree', 'Validation command', 'Validation timeout', 'Per-project validation', 'Linked directories', 'node_modules', 'Create hook', 'Remove hook', 'worktreeinclude', 'Merge queue'] },
  { tab: 'agent-github', section: 'GitHub Auto-Resolve', anchor: 'github-auto-resolve', keywords: ['Issues', 'Label', 'Auto resolve', 'Queue'], web: true },
];

function searchSettings(query) {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const results = [];
  for (const entry of SEARCH_INDEX) {
    if (entry.web && !IS_WEB) continue;
    const hits = entry.keywords.filter((k) => k.toLowerCase().includes(q));
    const sectionHit =
      entry.section.toLowerCase().includes(q) ||
      (TAB_LABELS[entry.tab] || '').toLowerCase().includes(q);
    if (sectionHit || hits.length) results.push({ ...entry, hits });
  }
  return results;
}

function SearchResults({ query, onNavigate }) {
  const results = useMemo(() => searchSettings(query), [query]);
  if (!results.length) {
    return (
      <div className="flex h-full items-center justify-center">
        <span className="text-[12px] text-muted-foreground">
          No settings match {'\u201c'}{query.trim()}{'\u201d'}
        </span>
      </div>
    );
  }
  return (
    <ScrollArea className="h-full">
      <div className="p-5 pb-8 space-y-1">
        {results.map((r) => (
          <button
            key={`${r.tab}-${r.section}`}
            onClick={() => onNavigate(r)}
            className="flex w-full items-center gap-3 rounded-lg border border-transparent px-3 py-2.5 text-left transition-colors hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium">{r.section}</div>
              <div className="mt-0.5 truncate text-[11px] text-muted-foreground">
                {r.hits.length ? r.hits.join(' · ') : r.keywords.slice(0, 5).join(' · ')}
              </div>
            </div>
            <span className="shrink-0 text-[11px] text-muted-foreground">{TAB_LABELS[r.tab]}</span>
            <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60" />
          </button>
        ))}
      </div>
    </ScrollArea>
  );
}

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
    case 'general':         return <GeneralSettings />;
    case 'editor':          return <EditorSettings />;
    case 'appearance':      return <AppearanceSettings />;
    case 'shortcuts':       return <ShortcutsSettings />;
    case 'agent-providers': return <AgentProvidersTab />;
    case 'agent-tools':     return <AgentToolsTab />;
    case 'agent-library':   return <AgentLibraryTab />;
    case 'agent-models':    return <AgentModelsTab />;
    case 'agent-github':    return <AgentGithubTab />;
    default:                return null;
  }
}

// Legacy deep-links used tab 'agent' (one monolithic tab). Map them onto the
// split agent tabs so old callers keep working.
function resolveInitialTab() {
  const { settingsInitialTab: tab, settingsInitialSection: section } = useLayout.getState();
  if (tab === 'agent') {
    return section === 'tools' || section === 'mcp' ? 'agent-tools' : 'agent-providers';
  }
  if (tab && TAB_ORDER.includes(tab)) return tab;
  return 'general';
}

export function SettingsPanel({ onClose } = {}) {
  const load     = useSettings((s) => s.load);
  const loading  = useSettings((s) => s.loading);
  const settings = useSettings((s) => s.settings);
  const error    = useSettings((s) => s.error);

  // Read the deep-link target lazily so callers that don't supply one fall
  // back to General. The panel only mounts when the modal opens (Radix
  // unmounts content when closed), so this initializer runs fresh each open.
  const [activeTab, setActiveTab] = useState(resolveInitialTab);
  const [query, setQuery] = useState('');
  const dirRef = useRef(0);
  const scrollTimerRef = useRef(null);
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

  function goToResult(entry) {
    setQuery('');
    switchTab(entry.tab);
    if (!entry.anchor) return;
    clearTimeout(scrollTimerRef.current);
    // Wait out the tab slide animation (~200ms) before scrolling to the anchor.
    scrollTimerRef.current = setTimeout(() => {
      document
        .querySelector(`[data-settings-anchor="${entry.anchor}"]`)
        ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }, 280);
  }

  useEffect(() => () => clearTimeout(scrollTimerRef.current), []);

  function onSearchKeyDown(e) {
    if (e.key === 'Enter') {
      const first = searchSettings(query)[0];
      if (first) goToResult(first);
    } else if (e.key === 'Escape' && query) {
      e.stopPropagation();
      setQuery('');
    }
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
                onKeyDown={onSearchKeyDown}
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
                onKeyDown={onSearchKeyDown}
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
          <nav className="flex w-48 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border/60 p-2.5 pt-3">
            {NAV_GROUPS.map((group, gi) => (
              <React.Fragment key={group.label}>
                <div className={cn('px-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60', gi > 0 && 'pt-3')}>
                  {group.label}
                </div>
                {group.items.map(({ id, label, icon: Icon }) => (
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
              </React.Fragment>
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
          {settings && query.trim() && (
            <SearchResults query={query} onNavigate={goToResult} />
          )}
          {settings && !query.trim() && (
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
              </motion.div>
            </AnimatePresence>
          )}
        </div>
      </div>
    </div>
  );
}
