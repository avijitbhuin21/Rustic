import React, { useEffect, useMemo, useRef } from 'react';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetBody,
} from '@/components/ui/sheet';
import { useAgent } from '@/state/agent';
import { ChatTurn } from './chat-turn';
import { cn } from '@/lib/utils';
import { Bot, Loader2, CheckCircle2, XCircle, Eye } from 'lucide-react';

// Group flat sub-agent messages into ChatTurn-shaped turns, mirroring the
// buildTurns / groupToolResults logic in chat-view.jsx. Sub-agents only ever
// have a single opening user message (the spawn prompt) — the rest of the
// transcript alternates assistant ↔ tool just like the main agent's. Reusing
// the same shape means we can drop the messages straight into <ChatTurn />.
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
        current = { user: null, blocks: [] };
        turns.push(current);
      }
      for (const block of m.content || []) {
        current.blocks.push({
          block,
          messageId: m.id,
          streaming: !!m.streaming,
        });
      }
    }
  }
  return turns;
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

function StatusPill({ status }) {
  const map = {
    running: {
      label: 'Running',
      icon: <Loader2 className="size-3 animate-spin" />,
      cls: 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300',
    },
    completed: {
      label: 'Completed',
      icon: <CheckCircle2 className="size-3" />,
      cls: 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300',
    },
    failed: {
      label: 'Failed',
      icon: <XCircle className="size-3" />,
      cls: 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300',
    },
  };
  const cfg = map[status] || map.running;
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium',
        cfg.cls,
      )}
    >
      {cfg.icon}
      {cfg.label}
    </span>
  );
}

// Read-only chat view for a spawned sub-agent. The user cannot type into it —
// sub-agents take a single prompt at spawn and run autonomously. This sheet
// just mirrors what the sub-agent's own chat would look like: streamed text,
// extended thinking, every tool call (with input + output), and the final
// summary. Backed by the live subagentsByTask map populated from the
// agent-subagent-* event stream.
export function SubagentChatSheet() {
  const open = useAgent((s) => s.openSubagent);
  const closeSubagentSheet = useAgent((s) => s.closeSubagentSheet);
  const sub = useAgent((s) => {
    if (!s.openSubagent) return null;
    const { taskId, agentId } = s.openSubagent;
    return s.subagentsByTask?.[taskId]?.[agentId] || null;
  });

  const scrollRef = useRef(null);
  const isOpen = !!open;

  const messages = sub?.messages || [];
  const turns = useMemo(() => buildTurns(messages), [messages]);
  const toolResults = useMemo(() => groupToolResults(messages), [messages]);

  // Auto-scroll to the latest delta. Sub-agent chats are watched, not driven,
  // so we don't need the "pinned to bottom" logic the main chat has — the
  // user only opens this sheet to follow live progress.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [messages.length, sub?.lastUpdate]);

  return (
    <Sheet open={isOpen} onOpenChange={(v) => !v && closeSubagentSheet()}>
      <SheetContent
        side="right"
        className="w-[640px] max-w-[95vw]"
        onOpenAutoFocus={(e) => e.preventDefault()}
      >
        <SheetHeader>
          <SheetTitle className="flex items-center gap-2 pr-8">
            <span className="flex size-5 items-center justify-center rounded bg-primary/10 text-primary">
              <Bot className="size-3.5" />
            </span>
            <span className="truncate">
              Sub-agent
              {sub?.agentId ? (
                <span className="ml-1.5 font-mono text-xs font-normal text-muted-foreground">
                  {sub.agentId.slice(0, 12)}
                </span>
              ) : null}
            </span>
            {sub && <StatusPill status={sub.status} />}
          </SheetTitle>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pt-1 text-[11px] text-muted-foreground">
            {sub?.model && (
              <span>
                Model: <span className="text-foreground">{sub.model}</span>
              </span>
            )}
            {sub?.cost?.estimated_cost_usd > 0 && (
              <span>
                Cost:{' '}
                <span className="text-foreground">
                  ${sub.cost.estimated_cost_usd.toFixed(4)}
                </span>
              </span>
            )}
            <span className="inline-flex items-center gap-1">
              <Eye className="size-3" /> Read-only
            </span>
          </div>
        </SheetHeader>

        <SheetBody>
          <div ref={scrollRef} className="h-full overflow-y-auto">
            {!sub ? (
              <div className="px-4 py-6 text-xs text-muted-foreground">
                Sub-agent not found.
              </div>
            ) : turns.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-2 px-4 py-10 text-xs text-muted-foreground">
                <Loader2 className="size-4 animate-spin" />
                Waiting for sub-agent to start streaming…
              </div>
            ) : (
              <div className="flex flex-col pb-4">
                {turns.map((turn, idx) => (
                  <ChatTurn
                    key={turn.user?.id ?? `sub-turn-${idx}`}
                    turn={turn}
                    toolResults={toolResults}
                  />
                ))}
              </div>
            )}
          </div>
        </SheetBody>
      </SheetContent>
    </Sheet>
  );
}

export default SubagentChatSheet;
