import React, { useMemo } from 'react';
import { useAgent } from '@/state/agent';
import { useCustomModels } from '@/state/custom-models';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

// ContextUsageCapsule — a pill next to the send button whose border is a
// progress ring showing how much of the auto-condense budget the task's
// context has consumed. 100% = the point where auto-condense triggers.
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

export function ContextUsageCapsule({ className }) {
  const activeTaskId = useAgent((s) => s.activeTaskId);
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

  const { pct, tokens, threshold, contextWindow, approx } = useMemo(() => {
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

  if (pct === null) return null;

  const angle = Math.max(0, Math.min(360, pct * 3.6));

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          className={cn('rounded-full p-px', className)}
          style={{
            background: `conic-gradient(color-mix(in oklab, var(--foreground) 55%, transparent) ${angle}deg, var(--border) ${angle}deg)`,
          }}
          aria-label={`Context ${pct}% used`}
        >
          <div className="flex h-[22px] min-w-[38px] select-none items-center justify-center rounded-full bg-background px-2 text-[10px] tabular-nums text-muted-foreground">
            {approx ? '~' : ''}{pct}%
          </div>
        </div>
      </TooltipTrigger>
      <TooltipContent side="top" className="text-center">
        <div>
          {approx ? '~' : ''}{fmtTokens(tokens)} / {fmtTokens(threshold)} tokens ({pct}% of the auto-condense budget{approx ? ', estimated' : ''})
        </div>
        <div className="text-muted-foreground">
          Auto-condenses at {fmtTokens(threshold)}
          {contextWindow > 0 ? ` · context window ${fmtTokens(contextWindow)}` : ''}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

export default ContextUsageCapsule;
