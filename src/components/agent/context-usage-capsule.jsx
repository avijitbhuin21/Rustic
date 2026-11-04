import React, { useMemo } from 'react';
import { ArrowUp } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { useCustomModels } from '@/state/custom-models';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { AgentMascot } from './agent-mascot';

// Context usage for the active task, drawn as a ring around the mascot send
// button. 100% = the point where auto-condense triggers.
//
// Data source: the 'agent-request-usage' event (exact, from the last provider
// call). For reopened tasks that haven't made a call yet this session, we fall
// back to a chars/4 estimate over the loaded transcript (marked "~"), mirroring
// the backend's own pre-call heuristic in executor.rs.

const EMPTY_MESSAGES = [];

// Matches CONDENSE_THRESHOLD_PCT in crates/rustic-agent/src/task/condense.rs.
const CONDENSE_THRESHOLD_PCT = 0.93;
// Rough allowance for the system prompt + tool schemas, which aren't part of
// the visible transcript but do occupy context. Only used for the estimate.
const ESTIMATE_OVERHEAD_TOKENS = 12_000;

const RING_SIZE = 32;
const RING_R = 14.5;
const RING_C = 2 * Math.PI * RING_R;

/** Formats a token count compactly: 950, 86.4k, 1.2M. */
function fmtTokens(n) {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1).replace(/\.0$/, '')}k`;
  return String(Math.round(n));
}

/** Estimates the token size of a loaded transcript (chars/4 + fixed overhead). */
function estimateTranscriptTokens(messages) {
  let chars = 0;
  for (const m of messages) {
    if (!m || !Array.isArray(m.content)) continue;
    for (const b of m.content) {
      if (!b || typeof b !== 'object') continue;
      if (typeof b.text === 'string') chars += b.text.length;
      if (typeof b.output === 'string') chars += b.output.length;
      if (b.input !== undefined) {
        try { chars += JSON.stringify(b.input).length; } catch {}
      }
    }
  }
  return Math.round(chars / 4) + ESTIMATE_OVERHEAD_TOKENS;
}

/** Ring stroke colour for a fill percentage: neutral, then warning, then danger near auto-condense. */
function ringColor(pct) {
  if (pct >= 90) return 'var(--danger)';
  if (pct >= 75) return 'var(--warning)';
  return 'color-mix(in oklab, var(--foreground) 60%, transparent)';
}

/** Context usage of the active task: `{ pct, tokens, threshold, contextWindow, approx }` (`pct` null when unknown). */
export function useContextUsage() {
  const live = useAgent((s) =>
    s.activeTaskId ? s.contextUsageByTask[s.activeTaskId] : null,
  );
  const messages = useAgent((s) =>
    (s.activeTaskId && s.messagesByTask[s.activeTaskId]) || EMPTY_MESSAGES,
  );
  const selectedModel = useAgent((s) => s.selectedModel);
  const builtinModels = useAgent((s) => s.models);
  const customMap = useCustomModels((s) => s.models);

  // Fallback spec for when no live event has arrived yet this session.
  const spec = useMemo(() => {
    const builtin = (builtinModels || []).find((m) => m.id === selectedModel);
    if (builtin?.context_window) {
      return { ctx: builtin.context_window, maxOut: builtin.max_output_tokens || 0 };
    }
    const custom = customMap?.[selectedModel];
    if (custom?.contextWindow) {
      return { ctx: custom.contextWindow, maxOut: custom.maxOutputTokens || 0 };
    }
    return null;
  }, [builtinModels, customMap, selectedModel]);

  return useMemo(() => {
    if (live?.threshold > 0) {
      const t = live.tokens || 0;
      return {
        pct: Math.min(100, Math.round((t / live.threshold) * 100)),
        tokens: t,
        threshold: live.threshold,
        contextWindow: live.contextWindow || 0,
        approx: false,
      };
    }
    if (!spec) return { pct: null, tokens: 0, threshold: 0, contextWindow: 0, approx: false };
    const th = Math.round((spec.ctx - spec.maxOut) * CONDENSE_THRESHOLD_PCT);
    if (th <= 0) return { pct: null, tokens: 0, threshold: 0, contextWindow: spec.ctx, approx: false };
    const est = messages.length > 0 ? estimateTranscriptTokens(messages) : 0;
    return {
      pct: Math.min(100, Math.round((est / th) * 100)),
      tokens: est,
      threshold: th,
      contextWindow: spec.ctx,
      approx: messages.length > 0,
    };
  }, [live, spec, messages]);
}

/**
 * Send button drawn as the agent mascot inside a context-usage ring. The ring
 * is an indicator only (hover shows the numbers); clicking performs the
 * button's action. `badge="send"` shows a small arrow when there is something
 * to send; while running, the mascot's own animation signals work (no badge).
 */
export function MascotSendButton({
  usage,
  mascotState = 'idle',
  subagents = 0,
  petKey = 0,
  badge = null,
  label,
  onClick,
  inactive = false,
  className,
}) {
  const pct = usage?.pct ?? null;
  const fill = pct == null ? 0 : Math.max(0, Math.min(100, pct));
  const ctxLine = pct == null
    ? null
    : `Context ${usage.approx ? '~' : ''}${pct}% · ${usage.approx ? '~' : ''}${fmtTokens(usage.tokens)} / ${fmtTokens(usage.threshold)} tokens`;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => { if (!inactive) onClick?.(); }}
          aria-label={ctxLine ? `${label}. ${ctxLine}` : label}
          aria-disabled={inactive || undefined}
          className={cn(
            'relative shrink-0 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring',
            inactive ? 'cursor-default' : 'cursor-pointer',
            className,
          )}
          style={{ width: RING_SIZE, height: RING_SIZE }}
        >
          <svg viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`} className="absolute inset-0 -rotate-90" aria-hidden>
            <circle cx={RING_SIZE / 2} cy={RING_SIZE / 2} r={RING_R} fill="var(--background)" stroke="var(--border)" strokeWidth="2" />
            <circle
              cx={RING_SIZE / 2}
              cy={RING_SIZE / 2}
              r={RING_R}
              fill="none"
              stroke={ringColor(fill)}
              strokeWidth="2"
              strokeLinecap="round"
              strokeDasharray={RING_C}
              strokeDashoffset={RING_C * (1 - fill / 100)}
              style={{ transition: 'stroke-dashoffset 400ms ease, stroke 400ms ease' }}
            />
          </svg>
          {/* Canvas is taller than the ring so the hop + heart can rise above it. */}
          <AgentMascot
            size="ring"
            state={mascotState}
            subagents={subagents}
            petKey={petKey}
            interactive={false}
            className="absolute left-1/2 -translate-x-1/2"
            style={{ top: -4 }}
          />
          {badge === 'send' && (
            <span
              aria-hidden
              className="absolute -bottom-0.5 -right-0.5 flex size-3.5 items-center justify-center rounded-full bg-foreground text-background ring-2 ring-background"
            >
              <ArrowUp className="size-2.5" strokeWidth={3} />
            </span>
          )}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="text-center">
        <div>{label}</div>
        {ctxLine && <div className="text-muted-foreground">{ctxLine}</div>}
        {pct != null && usage.threshold > 0 && (
          <div className="text-muted-foreground">
            Auto-condenses at {fmtTokens(usage.threshold)}
            {usage.contextWindow > 0 ? ` · context window ${fmtTokens(usage.contextWindow)}` : ''}
          </div>
        )}
      </TooltipContent>
    </Tooltip>
  );
}

export default MascotSendButton;
