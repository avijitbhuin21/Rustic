import React from 'react';
import { Coins } from 'lucide-react';
import { cn } from '@/lib/utils';

function formatTokens(n) {
  if (!n || n < 1000) return `${n || 0}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

function formatUsd(n) {
  if (!n) return '$0.00';
  if (n < 0.01) return '<$0.01';
  return `$${n.toFixed(n < 1 ? 4 : 2)}`;
}

export function CostIndicator({ cost, className }) {
  const tokens =
    (cost?.input_tokens || 0) +
    (cost?.output_tokens || 0) +
    (cost?.cache_read_tokens || 0) +
    (cost?.cache_creation_tokens || 0);
  const usd = cost?.total_cost_usd ?? cost?.usd ?? 0;
  return (
    <div
      className={cn(
        'flex items-center gap-1 rounded border border-border bg-muted/40 px-1.5 py-0.5 text-[11px] text-muted-foreground',
        className
      )}
      title="Tokens · USD"
    >
      <Coins className="size-3" />
      <span className="font-mono">{formatTokens(tokens)}</span>
      <span className="text-foreground/60">·</span>
      <span className="font-mono">{formatUsd(usd)}</span>
    </div>
  );
}

export default CostIndicator;
