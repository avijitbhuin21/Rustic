import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { motion, AnimatePresence, LayoutGroup } from 'framer-motion';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Button } from '@/components/ui/button';
import { IS_WEB } from '@/lib/platform';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import {
  Plus,
  MoreHorizontal,
  Server,
  Scroll,
  BookOpen,
  Workflow,
  PanelRightClose,
  FolderGit2,
  FolderPlus,
  ChevronDown,
  Check,
  ArrowLeft,
  ArrowDown,
  Bot,
  Loader2,
  CheckCircle2,
  XCircle,
  Eye,
} from 'lucide-react';
import { open as openFolderDialog } from '@tauri-apps/plugin-dialog';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { ChatTurn } from './chat-turn';
import { CostIndicator } from './cost-indicator';
import { AgentToolsSheet } from './agent-tools-sheet';
import { PromptBox } from './prompt-box';
import { AgentToolDock } from './agent-tool-dock';
import { StreamRetryBanner } from './stream-retry-banner';
import { ProviderErrorBanner } from './provider-error-banner';
import { CondenseBanner } from './condense-banner';
import { ModelChangeDivider } from './model-change-divider';
import { parseSpawnedAgentIds } from './tool-call-card';
import { EmptyState } from './empty-state';

const EMPTY_MESSAGES = [];
const EMPTY_MARKERS = [];

// Transcript font scale (D6). Sets CSS vars the chat-turn markdown classes
// read via text-[length:var(--chat-fs,...)]; 'default' leaves them unset so
// the baseline sizes apply.
const CHAT_FONT_KEY = 'rustic.agent.chatFontSize';
const CHAT_FONT_SIZES = [
  { id: 'default', label: 'Default', vars: null },
  { id: 'medium', label: 'Medium', vars: { '--chat-fs': '13px', '--chat-code-fs': '12px' } },
  { id: 'large', label: 'Large', vars: { '--chat-fs': '14px', '--chat-code-fs': '13px' } },
];
function loadChatFontSize() {
  try {
    const v = localStorage.getItem(CHAT_FONT_KEY);
    return CHAT_FONT_SIZES.some((s) => s.id === v) ? v : 'default';
  } catch {
    return 'default';
  }
}

// Same folder-picker + addProject flow the explorer's AddProjectButton uses,
// shared by the welcome CTA and the project-picker empty state.
async function pickAndAddProject(addProject) {
  try {
    const path = await openFolderDialog({ directory: true, multiple: false });
    if (typeof path === 'string') await addProject(path);
  } catch (err) {
    console.error('add project failed:', err);
  }
}

const STARTER_PROMPTS = [
  { label: 'Explore the codebase', text: 'Give me a tour of this codebase — structure, key modules, and how things fit together.' },
  { label: 'Fix a bug', text: 'Help me track down and fix a bug: ' },
  { label: 'Write tests', text: 'Write tests for ' },
  { label: 'Review my changes', text: 'Review my uncommitted changes and point out problems or improvements.' },
];
// Shared layoutId for the PromptBox wrapper. Using a single id across both the
// centered (empty) and docked (active) trees lets framer-motion run a single
// continuous slide animation when the first message lands, instead of swapping
// one input out and another in.
const PROMPT_LAYOUT_ID = 'agent-prompt-box';
// Exported so AgentPanel's outer wrapper can use the same spring — that way
// the panel's slide and the prompt's slide are choreographed (same easing,
// same duration) instead of feeling like two unrelated motions.
export const PROMPT_SPRING = { type: 'spring', stiffness: 260, damping: 30, mass: 0.7 };

// Animated agent mark shown on the empty chat screen. Layered motions:
//   - the whole mark floats gently up/down
//   - a soft halo behind it breathes (scale + opacity pulse)
//   - a dashed conic ring orbits clockwise
//   - the two "eyes" blink in unison every ~5s
// Built from divs + framer-motion so we don't need an SVG library. Sized to
// roughly match the previous icon's footprint so the surrounding layout
// doesn't shift.
function AnimatedAgentMark() {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.45, ease: [0.2, 0.65, 0.3, 0.9] }}
      className="relative grid size-20 place-items-center"
    >
      {/* Breathing halo */}
      <motion.div
        aria-hidden
        className="absolute inset-0 rounded-full bg-primary/15 blur-2xl"
        animate={{ scale: [1, 1.18, 1], opacity: [0.5, 0.85, 0.5] }}
        transition={{ duration: 3.2, repeat: Infinity, ease: 'easeInOut' }}
      />
      {/* Orbiting dashed ring */}
      <motion.div
        aria-hidden
        className="absolute inset-1 rounded-full border border-dashed border-primary/30"
        animate={{ rotate: 360 }}
        transition={{ duration: 22, repeat: Infinity, ease: 'linear' }}
      />
      {/* Floating robot head */}
      <motion.div
        animate={{ y: [0, -3, 0] }}
        transition={{ duration: 4, repeat: Infinity, ease: 'easeInOut' }}
        className="relative"
      >
        {/* Head */}
        <div className="flex size-12 items-center justify-center gap-1.5 rounded-2xl bg-gradient-to-br from-primary/85 via-primary/65 to-primary/30 shadow-lg shadow-primary/20 ring-1 ring-primary/40 backdrop-blur">
          {/* Eyes — coordinated blink. Two motion.divs share the same
              transition so they blink together; the `times` array shapes
              the blink as a quick downward squish then back. */}
          <motion.div
            aria-hidden
            className="size-1.5 rounded-full bg-primary-foreground"
            animate={{ scaleY: [1, 0.1, 1, 1] }}
            transition={{
              duration: 5,
              repeat: Infinity,
              times: [0, 0.06, 0.12, 1],
              ease: 'easeInOut',
            }}
          />
          <motion.div
            aria-hidden
            className="size-1.5 rounded-full bg-primary-foreground"
            animate={{ scaleY: [1, 0.1, 1, 1] }}
            transition={{
              duration: 5,
              repeat: Infinity,
              times: [0, 0.06, 0.12, 1],
              ease: 'easeInOut',
            }}
          />
        </div>
      </motion.div>
    </motion.div>
  );
}

// Top-of-chat project picker. Surfaces the active project alongside the cost
// so it's the first thing the user sees, and lets them switch project at any
// time. Switching projects doesn't destroy the current chat — it stays in the
// per-project task tree on the sidebar — it just clears the chat view back
// to the welcome state for the newly-picked project, where the user can pick
// up an existing task or start a fresh one.
function ProjectHeaderPicker() {
  const projects = useExplorer((s) => s.projects);
  const setExplorerProject = useExplorer((s) => s.setActiveProject);
  const addProject = useExplorer((s) => s.addProject);
  const activeProject = useAgent((s) => s.activeProject);
  const label = activeProject?.name || 'No project';
  const [open, setOpen] = useState(false);

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Project"
          className="flex h-7 max-w-[220px] items-center gap-1.5 rounded-md px-2 text-xs font-medium text-foreground transition-colors hover:bg-muted"
        >
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate">{label}</span>
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[220px]">
        <DropdownMenuLabel className="text-[10px] uppercase tracking-wide text-muted-foreground">
          Working in
        </DropdownMenuLabel>
        {projects.length === 0 && (
          <>
            <div className="px-2 py-1.5 text-xs text-muted-foreground">
              No projects open
            </div>
            <DropdownMenuItem onSelect={() => pickAndAddProject(addProject)}>
              <FolderPlus className="size-3.5 text-muted-foreground" /> Add project…
            </DropdownMenuItem>
          </>
        )}
        {projects.map((p) => {
          const isActive = p.id === activeProject?.id;
          return (
            <DropdownMenuItem
              key={p.id}
              onSelect={() => {
                setExplorerProject(p.id);
                setOpen(false);
              }}
              className="flex items-center gap-2"
            >
              <FolderGit2 className="size-3.5 text-muted-foreground" />
              <span className="flex-1 truncate">{p.name}</span>
              {isActive && <Check className="size-3.5 text-primary" />}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function groupToolResults(messages) {
  const map = {};
  for (const m of messages || []) {
    for (const block of m.content || []) {
      if (block.type === 'tool_result') {
        map[block.tool_use_id] = {
          output: block.output,
          is_error: block.is_error,
        };
      }
    }
  }
  return map;
}

// Group flat message stream into turns. A turn = one user message + all the
// assistant blocks (text, thinking, tool_use) that follow it before the next
// user message. tool_result blocks are skipped here — they're folded into
// the tool_use card via the toolResults map. Returning a stable shape so
// ChatTurn can render each turn with its own sticky user header.
function buildTurns(messages) {
  const turns = [];
  let current = null;
  for (const m of messages || []) {
    if (m.role === 'tool') continue;
    if (m.role === 'user') {
      current = { user: m, blocks: [] };
      turns.push(current);
      continue;
    }
    if (m.role === 'assistant') {
      if (!current) {
        // Assistant content with no preceding user message — rare, but render
        // it in its own headerless turn rather than dropping it.
        current = { user: null, blocks: [] };
        turns.push(current);
      }
      for (const block of m.content || []) {
        current.blocks.push({
          block,
          messageId: m.id,
          streaming: !!m.streaming,
          // Forward the message's wall-clock timestamp so per-block timestamps
          // (e.g. "5s ago" on a tool-call card) can render in the chat.
          timestamp: m.timestamp || 0,
        });
      }
    }
  }
  return turns;
}

/** Virtualized transcript: only rows near the viewport are mounted (PERF-01). Rows are absolutely positioned via `top` (not transform) so the sticky user headers inside ChatTurn keep working. */
function VirtualTurnList({ rows, toolResults, taskId, projectRoot, scrollRef, stickRef }) {
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].type === 'divider' ? 36 : 160),
    getItemKey: (i) => rows[i].key,
    overscan: 3,
    // Re-measuring rows above the viewport (as they mount during upward
    // scrolling) shifts content; compensate so the view doesn't jump. Skip
    // while pinned to bottom — the snap effect below owns the offset then.
    shouldAdjustScrollPositionOnItemSizeChange: (item, _delta, instance) =>
      !stickRef?.current && item.start < (instance.scrollOffset ?? 0),
  });

  const totalSize = virtualizer.getTotalSize();

  // Re-measured rows change scrollHeight without a messages update — keep
  // the viewport glued to the bottom while the user is pinned there.
  useLayoutEffect(() => {
    if (!stickRef?.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [totalSize, rows.length, scrollRef, stickRef]);

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1, transition: { duration: 0.2, delay: 0.05 } }}
      className="relative w-full"
      style={{ height: totalSize }}
    >
      {virtualizer.getVirtualItems().map((vi) => {
        const row = rows[vi.index];
        return (
          <div
            key={vi.key}
            data-index={vi.index}
            ref={virtualizer.measureElement}
            className="absolute left-0 w-full"
            style={{ top: vi.start }}
          >
            {row.type === 'divider' ? (
              <ModelChangeDivider marker={row.marker} />
            ) : (
              <ChatTurn
                turn={row.turn}
                toolResults={toolResults}
                taskId={taskId}
                projectRoot={projectRoot}
              />
            )}
          </div>
        );
      })}
    </motion.div>
  );
}

// Status chip mirroring the badges used by ToolCallCard so the sub-agent
// view feels visually consistent with how the main chat reports a tool call's
// state. Kept inline rather than in a shared module because it's only a few
// lines and only used here.
function SubagentStatusPill({ status }) {
  const cfg =
    status === 'completed'
      ? {
          label: 'Completed',
          icon: <CheckCircle2 className="size-3" />,
          cls: 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300',
        }
      : status === 'failed'
        ? {
            label: 'Failed',
            icon: <XCircle className="size-3" />,
            cls: 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300',
          }
        : {
            label: 'Running',
            icon: <Loader2 className="size-3 animate-spin" />,
            cls: 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300',
          };
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium',
        cfg.cls,
      )}
    >
      {cfg.icon}
      {cfg.label}
    </span>
  );
}

// In-place sub-agent transcript view. Replaces the main chat content (header
// + body + prompt) when `openSubagent` is set in agent state. Reuses the
// same <ChatTurn /> rendering as the main chat — the only differences are
// the back-button header and the absence of a prompt box / agent dock /
// retry banner (sub-agents take a single prompt at spawn and run autonomously).
function SubagentInlineView({ sub, agentId, name, onBack, projectRoot, fontStyle }) {
  const closeChatDock = useLayout((s) => s.closeChatDock);
  const scrollRef = useRef(null);
  const stickToBottomRef = useRef(true);

  const messages = sub?.messages || EMPTY_MESSAGES;
  const hasMessages = messages.length > 0;
  const turns = useMemo(() => buildTurns(messages), [messages]);
  const toolResults = useMemo(() => groupToolResults(messages), [messages]);
  const rows = useMemo(
    () =>
      turns.map((turn, idx) => ({
        type: 'turn',
        turn,
        key: turn.user?.id ?? `sub-turn-${idx}`,
      })),
    [turns],
  );

  // Same pin-to-bottom behaviour as the main chat: keep snapping to bottom
  // while the user is parked there, yield the moment they scroll up to read.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      stickToBottomRef.current = distanceFromBottom < 32;
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [agentId, hasMessages]);

  useEffect(() => {
    if (!stickToBottomRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, sub?.status]);

  // Entering a new sub-agent: reset to bottom in follow-mode.
  useEffect(() => {
    stickToBottomRef.current = true;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [agentId]);

  return (
    <>
      <div
        className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2"
        style={{ paddingRight: IS_WEB ? undefined : 138 }}
      >
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-7"
              onClick={onBack}
            >
              <ArrowLeft className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Back to main chat</TooltipContent>
        </Tooltip>
        <span className="flex size-5 shrink-0 items-center justify-center rounded bg-primary/10 text-primary">
          <Bot className="size-3.5" />
        </span>
        <span
          className="min-w-0 truncate text-xs font-medium text-foreground"
          title={agentId}
        >
          {name || 'Sub-agent'}
          {!name && agentId && (
            <span className="ml-1.5 font-mono text-[11px] font-normal text-muted-foreground">
              {agentId.slice(0, 12)}
            </span>
          )}
        </span>
        {sub?.model && (
          <span className="hidden shrink-0 text-[11px] text-muted-foreground md:inline">
            · {sub.model}
          </span>
        )}
        <SubagentStatusPill status={sub?.status || 'running'} />
        {sub?.cost && <CostIndicator cost={sub.cost} />}
        <span className="ml-auto flex items-center gap-1 text-[10px] text-muted-foreground">
          <Eye className="size-3" /> Read-only
        </span>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-7"
              onClick={closeChatDock}
            >
              <PanelRightClose className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Close chat dock</TooltipContent>
        </Tooltip>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {!sub ? (
          <div className="flex flex-1 items-center justify-center px-4 text-xs text-muted-foreground">
            Sub-agent not found.
          </div>
        ) : !hasMessages ? (
          <EmptyState
            icon={Loader2}
            iconClassName="animate-spin"
            title="Waiting for sub-agent to start streaming…"
            hint="The transcript appears here as soon as the first tokens arrive."
            className="flex-1"
          />
        ) : (
          <div
            ref={scrollRef}
            style={fontStyle}
            className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
          >
            <VirtualTurnList
              rows={rows}
              toolResults={toolResults}
              taskId={sub?.taskId}
              projectRoot={projectRoot}
              scrollRef={scrollRef}
              stickRef={stickToBottomRef}
            />
            <div className="h-4" />
          </div>
        )}
      </div>
    </>
  );
}

export function ChatView() {
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const messages = useAgent((s) =>
    (s.activeTaskId && s.messagesByTask[s.activeTaskId]) || EMPTY_MESSAGES
  );
  const isStreaming = useAgent((s) =>
    s.activeTaskId ? !!s.streamingByTask[s.activeTaskId] : false
  );
  const isCondensing = useAgent((s) =>
    s.activeTaskId ? !!s.condensingByTask[s.activeTaskId] : false
  );
  const cost = useAgent((s) =>
    s.activeTaskId ? s.costByTask[s.activeTaskId] : null
  );
  const sendMessage = useAgent((s) => s.sendMessage);
  const abortActive = useAgent((s) => s.abortActive);
  const createTaskForProject = useAgent((s) => s.createTaskForProject);
  const activeProject = useAgent((s) => s.activeProject);
  const projects = useExplorer((s) => s.projects);
  const addProject = useExplorer((s) => s.addProject);
  const closeChatDock = useLayout((s) => s.closeChatDock);

  // Sub-agent navigation: when openSubagent is set we render the sub-agent's
  // transcript inline (back-button header + ChatTurn body, no input) in place
  // of the main chat. Single-level — back always returns to the main chat,
  // since only the main agent can spawn sub-agents.
  const openSubagent = useAgent((s) => s.openSubagent);
  const subagent = useAgent((s) => {
    if (!s.openSubagent) return null;
    const { taskId, agentId } = s.openSubagent;
    return s.subagentsByTask?.[taskId]?.[agentId] || null;
  });
  const closeSubagentView = useAgent((s) => s.closeSubagentView);

  // Switching tasks while a sub-agent view is open would leave us viewing a
  // sub-agent that doesn't belong to the current task — pop back to main.
  useEffect(() => {
    if (openSubagent && openSubagent.taskId !== activeTaskId) {
      closeSubagentView();
    }
  }, [activeTaskId, openSubagent, closeSubagentView]);

  const scrollRef = useRef(null);
  // Tracks whether the user is "pinned" to the bottom of the chat. As long as
  // they are, we keep snapping to bottom when new content arrives (so streamed
  // tokens stay visible). The moment they scroll up to read earlier content,
  // we set this false and STOP auto-scrolling — otherwise every streaming
  // token would yank them back down and the chat would feel un-scrollable.
  const stickToBottomRef = useRef(true);
  // State mirror of the ref so the jump-to-bottom pill can render/hide.
  const [pinned, setPinned] = useState(true);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [toolsTab, setToolsTab] = useState('mcp');

  // The scroll container is only mounted once there are messages — before
  // that, the welcome screen renders instead. We key listener-attachment on
  // `hasMessages` so the listener actually binds when the container appears,
  // not just when the task id changes (which fires while the container is
  // still null).
  const hasMessages = messages.length > 0;

  // Track whether the user is pinned to the bottom of the chat. Slack (32px)
  // covers rounding nudges so near-bottom still counts as pinned. Once they
  // scroll up, this flips false and auto-scroll yields — otherwise every
  // streamed token would yank them back down.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      const next = distanceFromBottom < 32;
      stickToBottomRef.current = next;
      setPinned(next);
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [activeTaskId, hasMessages]);

  // Snap to bottom only when the user is already pinned there. Unconditional
  // snapping would break scroll-up during streaming.
  useEffect(() => {
    if (!stickToBottomRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, isStreaming]);

  // Switching tasks (or first mount of the scroll container): jump to bottom
  // and reset pin so the new transcript starts in follow-mode.
  useEffect(() => {
    stickToBottomRef.current = true;
    setPinned(true);
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [activeTaskId, hasMessages]);

  const jumpToBottom = () => {
    stickToBottomRef.current = true;
    setPinned(true);
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  const toolResults = useMemo(() => groupToolResults(messages), [messages]);
  const turns = useMemo(() => buildTurns(messages), [messages]);

  // agentId → human name, recovered from spawn_subagent tool calls: the
  // model's `agents` input carries the names, the tool output carries the
  // spawned ids in matching positions. The backend doesn't persist the name,
  // so this is the only place it can come from.
  const subagentNames = useMemo(() => {
    const map = {};
    for (const m of messages) {
      if (m.role !== 'assistant') continue;
      for (const b of m.content || []) {
        if (b.type !== 'tool_use' || b.name !== 'spawn_subagent') continue;
        let agents = b.input?.agents;
        if (typeof agents === 'string') {
          try {
            agents = JSON.parse(agents);
          } catch {
            agents = null;
          }
        }
        if (!Array.isArray(agents)) continue;
        const ids = parseSpawnedAgentIds(toolResults[b.id]?.output);
        ids.forEach((id, i) => {
          if (id && agents[i]?.name) map[id] = agents[i].name;
        });
      }
    }
    return map;
  }, [messages, toolResults]);

  // Data backing the running-status strip: when the current run started (the
  // last user message), how many tool calls it has made, and which tool is
  // still awaiting its result (if any).
  const runInfo = useMemo(() => {
    if (!isStreaming) return null;
    let startedAt = null;
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'user') {
        startedAt = messages[i].timestamp || null;
        break;
      }
    }
    let toolCount = 0;
    let runningTool = null;
    for (const m of messages) {
      if (m.role !== 'assistant') continue;
      if (startedAt && (m.timestamp || 0) < startedAt) continue;
      for (const b of m.content || []) {
        if (b.type !== 'tool_use') continue;
        toolCount += 1;
        if (toolResults[b.id] === undefined) runningTool = b.name;
      }
    }
    return { startedAt, toolCount, runningTool };
  }, [isStreaming, messages, toolResults]);

  // Mid-chat model/effort switches render as labelled dividers between turns.
  // Markers are anchored to the user-turn index they precede; index into them
  // by that position so the render loop can splice a divider before the turn.
  const modelMarkers = useAgent((s) =>
    (s.activeTaskId && s.modelMarkersByTask[s.activeTaskId]) || EMPTY_MARKERS,
  );
  const markersByTurnIndex = useMemo(() => {
    const map = {};
    for (const mk of modelMarkers) map[mk.turnIndex] = mk;
    return map;
  }, [modelMarkers]);

  // Flat virtualizer row list: model-change dividers spliced before the
  // user-turn they precede (headerless turns don't advance the counter).
  const rows = useMemo(() => {
    const out = [];
    let userTurnIdx = 0;
    for (let idx = 0; idx < turns.length; idx++) {
      const turn = turns[idx];
      if (turn.user) {
        const marker = markersByTurnIndex[userTurnIdx];
        if (marker) out.push({ type: 'divider', marker, key: `mk-${marker.id}` });
        userTurnIdx++;
      }
      out.push({ type: 'turn', turn, key: turn.user?.id ?? `turn-${idx}` });
    }
    return out;
  }, [turns, markersByTurnIndex]);

  // True while the user has sent a message but the backend hasn't streamed
  // any assistant content yet — the cold-start setup window on the first
  // send. Shows a small "Preparing…" pill below the last turn so the chat
  // doesn't look frozen for the few seconds before the first delta lands.
  const isPreparing =
    isStreaming &&
    turns.length > 0 &&
    turns[turns.length - 1].user &&
    turns[turns.length - 1].blocks.length === 0;

  const openTools = (tab) => {
    setToolsTab(tab);
    setToolsOpen(true);
  };

  const [chatFontSize, setChatFontSizeState] = useState(loadChatFontSize);
  const setChatFontSize = (id) => {
    setChatFontSizeState(id);
    try {
      localStorage.setItem(CHAT_FONT_KEY, id);
    } catch {}
  };
  const chatFontStyle =
    CHAT_FONT_SIZES.find((s) => s.id === chatFontSize)?.vars || undefined;

  const handleNewChat = () => {
    // Just clear the active task — don't materialize a backend task yet.
    // sendMessage → ensureTask creates the task lazily on first send, so
    // spamming "+" without sending no longer leaves a trail of empty tasks
    // in the sidebar / DB.
    useAgent.setState({ activeTaskId: null });
  };

  if (openSubagent) {
    return (
      <div className="flex h-full flex-col">
        <SubagentInlineView
          sub={subagent}
          agentId={openSubagent.agentId}
          name={subagent?.name || subagentNames[openSubagent.agentId]}
          onBack={closeSubagentView}
          projectRoot={activeProject?.root}
          fontStyle={chatFontStyle}
        />
        <AgentToolsSheet
          open={toolsOpen}
          onOpenChange={setToolsOpen}
          initialTab={toolsTab}
        />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* Header. When the chat dock is mounted it always sits at the top-right
          of the window, under the fixed window-control strip (130 px wide).
          Reserve room on the right so the close-dock / agent-tools buttons
          aren't trapped under min/max/close. */}
      <div
        className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2"
        style={{ paddingRight: IS_WEB ? undefined : 138 }}
      >
        <ProjectHeaderPicker />
        {messages.length > 0 && cost && <CostIndicator cost={cost} />}
        <div className="ml-auto flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="size-7"
                onClick={handleNewChat}
              >
                <Plus className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">New chat in this project</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="size-7"
                  >
                    <MoreHorizontal className="size-3.5" />
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom">Agent tools</TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="end" className="w-44">
              <DropdownMenuLabel>Agent tools</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => openTools('mcp')}>
                <Server className="mr-2 size-3.5" /> MCP servers
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => openTools('rules')}>
                <Scroll className="mr-2 size-3.5" /> Rules
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => openTools('skills')}>
                <BookOpen className="mr-2 size-3.5" /> Skills
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => openTools('workflows')}>
                <Workflow className="mr-2 size-3.5" /> Workflows
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuLabel className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                Text size
              </DropdownMenuLabel>
              <div className="flex gap-1 px-2 pb-1.5">
                {CHAT_FONT_SIZES.map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    title={s.label}
                    onClick={() => setChatFontSize(s.id)}
                    className={cn(
                      'flex-1 rounded border px-1.5 py-1 text-center transition-colors',
                      chatFontSize === s.id
                        ? 'border-primary/50 bg-primary/10 text-primary'
                        : 'border-border/60 text-muted-foreground hover:bg-muted hover:text-foreground',
                    )}
                  >
                    <span
                      className={cn(
                        'font-medium',
                        s.id === 'default' && 'text-[11px]',
                        s.id === 'medium' && 'text-xs',
                        s.id === 'large' && 'text-sm',
                      )}
                    >
                      A
                    </span>
                  </button>
                ))}
              </div>
            </DropdownMenuContent>
          </DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="size-7"
                onClick={closeChatDock}
              >
                <PanelRightClose className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Close chat dock</TooltipContent>
          </Tooltip>
        </div>
      </div>

      <LayoutGroup>
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
          {messages.length === 0 ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-6 px-6 py-10">
              <AnimatePresence>
                <motion.div
                  key="welcome"
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -16, transition: { duration: 0.18 } }}
                  className="flex flex-col items-center gap-4 text-center"
                >
                  <AnimatedAgentMark />
                  <div className="text-lg font-medium tracking-tight text-foreground">
                    Start a conversation
                  </div>
                  <div className="max-w-md text-sm italic text-muted-foreground">
                    {activeProject?.name
                      ? `Ask the agent to read, edit, or build in ${activeProject.name}.`
                      : 'Ask the agent to read code, run tools, or build something.'}
                  </div>
                  <div className="max-w-md text-xs italic text-muted-foreground/70">
                    Tip: tag files with @ · use skills & workflows with /
                  </div>
                  {projects.length === 0 && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="gap-1.5"
                      onClick={() => pickAndAddProject(addProject)}
                    >
                      <FolderPlus className="size-3.5" /> Add project
                    </Button>
                  )}
                </motion.div>
              </AnimatePresence>
              <motion.div
                layoutId={PROMPT_LAYOUT_ID}
                transition={PROMPT_SPRING}
                className="w-full max-w-2xl"
              >
                <PromptBox
                  onSubmit={sendMessage}
                  onAbort={abortActive}
                  isStreaming={isStreaming}
                  variant="hero"
                  autoFocus
                  placeholder="Ask the agent…"
                  chatStarted={false}
                />
              </motion.div>
              {activeProject?.name && (
                <motion.div
                  initial={{ opacity: 0, y: 4 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: 0.15, duration: 0.25 }}
                  className="flex max-w-2xl flex-wrap items-center justify-center gap-1.5"
                >
                  {STARTER_PROMPTS.map((p) => (
                    <button
                      key={p.label}
                      type="button"
                      onClick={() =>
                        window.dispatchEvent(
                          new CustomEvent('prompt-insert', {
                            detail: { text: p.text },
                          }),
                        )
                      }
                      className="rounded-full border border-border/60 bg-muted/30 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:border-border hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
                    >
                      {p.label}
                    </button>
                  ))}
                </motion.div>
              )}
            </div>
          ) : (
            <>
              <div className="relative min-h-0 flex-1">
                <div
                  ref={scrollRef}
                  style={chatFontStyle}
                  className="h-full overflow-y-auto overflow-x-hidden"
                >
                  <VirtualTurnList
                    rows={rows}
                    toolResults={toolResults}
                    taskId={activeTaskId}
                    projectRoot={activeProject?.root}
                    scrollRef={scrollRef}
                    stickRef={stickToBottomRef}
                  />
                  <AnimatePresence>
                    {isPreparing && (
                      <motion.div
                        key="preparing"
                        initial={{ opacity: 0, y: 4 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, transition: { duration: 0.15 } }}
                        transition={{ duration: 0.2 }}
                        className="mx-auto flex w-full max-w-3xl items-center gap-2 px-6 pb-2 pt-1 text-xs text-muted-foreground"
                      >
                        <Loader2 className="size-3.5 animate-spin text-blue-500" />
                        <span>Preparing…</span>
                      </motion.div>
                    )}
                  </AnimatePresence>
                </div>
                <AnimatePresence>
                  {!pinned && (
                    <motion.button
                      key="jump-bottom"
                      type="button"
                      initial={{ opacity: 0, y: 8 }}
                      animate={{ opacity: 1, y: 0 }}
                      exit={{ opacity: 0, y: 8, transition: { duration: 0.15 } }}
                      style={{ x: '-50%' }}
                      onClick={jumpToBottom}
                      className="absolute bottom-3 left-1/2 z-30 flex items-center gap-1.5 rounded-full bg-primary px-3 py-1.5 text-[11px] font-medium text-primary-foreground shadow-lg ring-1 ring-black/10 transition-colors hover:bg-primary/90"
                    >
                      <ArrowDown className="size-3" />
                      {isStreaming ? 'New messages' : 'Jump to bottom'}
                    </motion.button>
                  )}
                </AnimatePresence>
              </div>
              {/* Stream-retry banner sits above the dock so the user can
                  see "Retrying in 60s — Rate limit (429)" while the agent
                  is mid-backoff. Renders nothing when no retry is pending. */}
              <StreamRetryBanner />
              {/* Provider-error banner: a deterministic 4xx the provider will
                  always reject. Offers one-click history repair + resume.
                  Renders nothing when no such error is pending. */}
              <ProviderErrorBanner />
              {/* Condense banner shows when the agent is compacting the
                  context. Renders nothing when not condensing. */}
              <CondenseBanner />
              {/* Three-tab dock fused to the top of the prompt box: Plan
                  (todos), Files (placeholder), Terminals (placeholder). The
                  dock's bottom border is removed and the prompt's top border
                  is flattened so they read as one unified container. */}
              <AgentToolDock />
              <motion.div
                layoutId={PROMPT_LAYOUT_ID}
                transition={PROMPT_SPRING}
                className="mx-auto w-full max-w-3xl shrink-0 px-3 pb-3 pt-0"
              >
                <PromptBox
                  onSubmit={sendMessage}
                  onAbort={abortActive}
                  isStreaming={isStreaming}
                  runInfo={runInfo}
                  variant="default"
                  placeholder="Ask the agent…"
                  chatStarted
                  flatTop
                />
              </motion.div>
            </>
          )}
        </div>
      </LayoutGroup>

      <AgentToolsSheet
        open={toolsOpen}
        onOpenChange={setToolsOpen}
        initialTab={toolsTab}
      />
    </div>
  );
}

export default ChatView;
