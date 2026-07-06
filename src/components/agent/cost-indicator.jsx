import React, { useState } from 'react';
import { Coins } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogTrigger,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';

function formatTokens(n) {
  const v = Number(n) || 0;
  if (v < 1000) return `${v}`;
  if (v < 1_000_000) return `${(v / 1000).toFixed(1)}k`;
  return `${(v / 1_000_000).toFixed(2)}M`;
}

function formatTokensExact(n) {
  return (Number(n) || 0).toLocaleString();
}

function formatUsd(n) {
  const v = Number(n) || 0;
  if (v === 0) return '$0.00';
  if (v < 0.01) return '<$0.01';
  return `$${v.toFixed(v < 1 ? 4 : 2)}`;
}

function formatUsdExact(n) {
  const v = Number(n) || 0;
  if (v === 0) return '$0.00';
  if (v >= 1) return `$${v.toFixed(2)}`;
  return `$${v.toFixed(6)}`;
}

// One line in the breakdown. Tokens column is optional — image/video/sub-agent
// rows leave it blank since they don't have a meaningful token count.
function BreakdownRow({ label, tokens, cost, dim }) {
  return (
    <div
      className={cn(
        'grid grid-cols-[1fr_auto_auto] items-baseline gap-x-4 py-1 text-sm',
        dim && 'text-muted-foreground',
      )}
    >
      <span>{label}</span>
      <span className="font-mono text-xs tabular-nums text-muted-foreground">
        {tokens != null ? formatTokensExact(tokens) : ''}
      </span>
      <span className="font-mono tabular-nums">{formatUsdExact(cost)}</span>
    </div>
  );
}

// One line in the per-model section. Shows the model id (middle-truncated
// via CSS), its total token count, and its share of the spend.
function ModelRow({ entry }) {
  const tokens =
    (entry.input_tokens || 0) +
    (entry.output_tokens || 0) +
    (entry.cache_read_tokens || 0) +
    (entry.cache_write_tokens || 0);
  return (
    <div className="grid grid-cols-[1fr_auto_auto] items-baseline gap-x-4 py-1 text-sm">
      <span className="truncate font-mono text-xs" title={entry.model}>
        {entry.model}
      </span>
      <span className="font-mono text-xs tabular-nums text-muted-foreground">
        {formatTokensExact(tokens)}
      </span>
      <span className="font-mono tabular-nums">{formatUsdExact(entry.cost_usd)}</span>
    </div>
  );
}

// CostIndicator surfaces a quick at-a-glance summary in the chat header
// (cumulative context tokens + cost), and opens a dialog with the full
// breakdown on click. Backend emits TaskCost which is cumulative across all
// turns in the active task, so all numbers below are lifetime totals for the
// conversation — not per-turn.
export function CostIndicator({ cost, className }) {
  const [open, setOpen] = useState(false);
  const input = cost?.total_input_tokens ?? 0;
  const output = cost?.total_output_tokens ?? 0;
  const cacheRead = cost?.total_cache_read_tokens ?? 0;
  const cacheWrite = cost?.total_cache_write_tokens ?? 0;
  const usd = cost?.estimated_cost_usd ?? 0;
  const turns = cost?.turn_count ?? 0;

  const inputCost = cost?.input_cost_usd ?? 0;
  const outputCost = cost?.output_cost_usd ?? 0;
  const cacheReadCost = cost?.cache_read_cost_usd ?? 0;
  const cacheWriteCost = cost?.cache_write_cost_usd ?? 0;
  const imageCost = cost?.image_cost_usd ?? 0;
  const videoCost = cost?.video_cost_usd ?? 0;
  const subagentCost = cost?.subagent_cost_usd ?? 0;
  const byModel = Array.isArray(cost?.by_model) ? cost.by_model : [];

  // "Context" tokens = anything the model had to read in. Input + cache_read
  // gives the user a sense of how much they're spending on conversation
  // history; output is billed separately so it's not part of context.
  const totalContext = input + cacheRead;
  const totalTokens = input + output + cacheRead + cacheWrite;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <button
          type="button"
          title="Token usage & cost — click for details"
          className={cn(
            'flex h-6 items-center gap-1 rounded-md border border-border bg-muted/40 px-1.5 text-[11px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground',
            className,
          )}
        >
          <Coins className="size-3" />
          <span className="font-mono">{formatTokens(totalContext)}</span>
          <span className="text-foreground/40">·</span>
          <span className="font-mono">{formatUsd(usd)}</span>
        </button>
      </DialogTrigger>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Token usage & cost</DialogTitle>
          <DialogDescription>
            Cumulative across {turns} {turns === 1 ? 'turn' : 'turns'} in this
            task.
          </DialogDescription>
        </DialogHeader>

        {/* Tight grid layout. Column header row sets expectations so the
            empty token cells on image/video/sub-agent rows don't read as bugs. */}
        <div className="flex flex-col">
          <div className="grid grid-cols-[1fr_auto_auto] items-baseline gap-x-4 border-b border-border pb-1 text-[11px] uppercase tracking-wide text-muted-foreground">
            <span>Category</span>
            <span className="font-mono">Tokens</span>
            <span className="font-mono">Cost</span>
          </div>

          <BreakdownRow label="Input" tokens={input} cost={inputCost} />
          <BreakdownRow label="Output" tokens={output} cost={outputCost} />
          <BreakdownRow
            label="Cache read"
            tokens={cacheRead}
            cost={cacheReadCost}
            dim
          />
          <BreakdownRow
            label="Cache write"
            tokens={cacheWrite}
            cost={cacheWriteCost}
            dim
          />
          {imageCost > 0 && (
            <BreakdownRow label="Image gen" cost={imageCost} />
          )}
          {videoCost > 0 && (
            <BreakdownRow label="Video gen" cost={videoCost} />
          )}
          {subagentCost > 0 && (
            <BreakdownRow label="Sub-agents" cost={subagentCost} />
          )}

          {byModel.length > 0 && (
            <>
              <div className="mt-3 grid grid-cols-[1fr_auto_auto] items-baseline gap-x-4 border-b border-border pb-1 text-[11px] uppercase tracking-wide text-muted-foreground">
                <span>By model</span>
                <span className="font-mono">Tokens</span>
                <span className="font-mono">Cost</span>
              </div>
              {byModel.map((m) => (
                <ModelRow key={m.model} entry={m} />
              ))}
            </>
          )}

          <div className="my-2 h-px bg-border" />

          <div className="grid grid-cols-[1fr_auto_auto] items-baseline gap-x-4 py-1 text-sm font-medium">
            <span>Total</span>
            <span className="font-mono text-xs tabular-nums text-muted-foreground">
              {formatTokensExact(totalTokens)}
            </span>
            <span className="font-mono font-semibold tabular-nums text-primary">
              {formatUsdExact(usd)}
            </span>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export default CostIndicator;
