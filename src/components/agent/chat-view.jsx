import React, { useEffect, useMemo, useRef, useState } from 'react';
import { motion, AnimatePresence, LayoutGroup } from 'framer-motion';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuLabel,
} from '@/components/ui/dropdown-menu';
import {
  Plus,
  MoreHorizontal,
  Server,
  Scroll,
  BookOpen,
  Workflow,
  PanelRightClose,
  FolderGit2,
  ChevronDown,
  Check,
  ArrowLeft,
  Bot,
  Loader2,
  CheckCircle2,
  XCircle,
  Eye,
} from 'lucide-react';
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

const EMPTY_MESSAGES = [];
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
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            No projects open
          </div>
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
function SubagentInlineView({ sub, agentId, onBack, projectRoot }) {
  const closeChatDock = useLayout((s) => s.closeChatDock);
  const scrollRef = useRef(null);
  const stickToBottomRef = useRef(true);

  const messages = sub?.messages || EMPTY_MESSAGES;
  const hasMessages = messages.length > 0;
  const turns = useMemo(() => buildTurns(messages), [messages]);
  const toolResults = useMemo(() => groupToolResults(messages), [messages]);

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
        style={{ paddingRight: 138 }}
      >
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-7"
          title="Back to main chat"
          onClick={onBack}
        >
          <ArrowLeft className="size-3.5" />
        </Button>
        <span className="flex size-5 shrink-0 items-center justify-center rounded bg-primary/10 text-primary">
          <Bot className="size-3.5" />
        </span>
        <span className="min-w-0 truncate text-xs font-medium text-foreground">
          Sub-agent
          {agentId && (
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
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-7"
          title="Close chat dock"
          onClick={closeChatDock}
        >
          <PanelRightClose className="size-3.5" />
        </Button>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {!sub ? (
          <div className="flex flex-1 items-center justify-center px-4 text-xs text-muted-foreground">
            Sub-agent not found.
          </div>
        ) : !hasMessages ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 px-4 text-xs text-muted-foreground">
            <Loader2 className="size-4 animate-spin" />
            Waiting for sub-agent to start streaming…
          </div>
        ) : (
          <div
            ref={scrollRef}
            className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
          >
            <motion.div
              initial={{ opacity: 0 }}
              animate={{ opacity: 1, transition: { duration: 0.2, delay: 0.05 } }}
              className="flex flex-col pb-4"
            >
              {turns.map((turn, idx) => (
                <ChatTurn
                  key={turn.user?.id ?? `sub-turn-${idx}`}
                  turn={turn}
                  toolResults={toolResults}
                  taskId={sub?.taskId}
                  projectRoot={projectRoot}
                />
              ))}
            </motion.div>
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
  const cost = useAgent((s) =>
    s.activeTaskId ? s.costByTask[s.activeTaskId] : null
  );
  const sendMessage = useAgent((s) => s.sendMessage);
  const abortActive = useAgent((s) => s.abortActive);
  const createTaskForProject = useAgent((s) => s.createTaskForProject);
  const activeProject = useAgent((s) => s.activeProject);
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
      stickToBottomRef.current = distanceFromBottom < 32;
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
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [activeTaskId, hasMessages]);

  const toolResults = useMemo(() => groupToolResults(messages), [messages]);
  const turns = useMemo(() => buildTurns(messages), [messages]);

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
          onBack={closeSubagentView}
          projectRoot={activeProject?.root}
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
        style={{ paddingRight: 138 }}
      >
        <ProjectHeaderPicker />
        {messages.length > 0 && cost && <CostIndicator cost={cost} />}
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon-sm"
            className="size-7"
            title="New chat in this project"
            onClick={handleNewChat}
          >
            <Plus className="size-3.5" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="size-7"
                title="Agent tools"
              >
                <MoreHorizontal className="size-3.5" />
              </Button>
            </DropdownMenuTrigger>
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
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            variant="ghost"
            size="icon-sm"
            className="size-7"
            title="Close chat dock"
            onClick={closeChatDock}
          >
            <PanelRightClose className="size-3.5" />
          </Button>
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
                  <div className="max-w-md text-sm text-muted-foreground">
                    {activeProject?.name
                      ? `Ask the agent to read, edit, or build in ${activeProject.name}.`
                      : 'Ask the agent to read code, run tools, or build something.'}
                  </div>
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
            </div>
          ) : (
            <>
              <div
                ref={scrollRef}
                className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
              >
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1, transition: { duration: 0.2, delay: 0.05 } }}
                  className="flex flex-col"
                >
                  {turns.map((turn, idx) => (
                    <ChatTurn
                      key={turn.user?.id ?? `turn-${idx}`}
                      turn={turn}
                      toolResults={toolResults}
                      taskId={activeTaskId}
                      projectRoot={activeProject?.root}
                    />
                  ))}
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
                </motion.div>
              </div>
              {/* Stream-retry banner sits above the dock so the user can
                  see "Retrying in 60s — Rate limit (429)" while the agent
                  is mid-backoff. Renders nothing when no retry is pending. */}
              <StreamRetryBanner />
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
