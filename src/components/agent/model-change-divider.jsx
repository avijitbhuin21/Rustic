import React, { useMemo } from 'react';
import { ArrowLeftRight } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { useCustomModels } from '@/state/custom-models';

// Resolve a model id to its human label the same way the model picker does:
// built-in registry first, then user-saved custom specs, falling back to the
// raw id so a since-removed model still reads sensibly.
function resolveModelLabel(modelId, builtinModels, customMap) {
  if (!modelId) return 'Model';
  const builtin = (builtinModels || []).find(
    (m) => (m.id || m.model_id) === modelId,
  );
  if (builtin) return builtin.name || builtin.display_name || modelId;
  const custom = customMap?.[modelId];
  if (custom?.name) return custom.name;
  return modelId;
}

// Reasoning-effort tiers map to a short label; 'off' shows nothing so the chip
// stays compact when no thinking budget is set.
const TIER_LABELS = {
  off: null,
  low: 'Low effort',
  medium: 'Medium effort',
  high: 'High effort',
  max: 'Max effort',
};

// A labelled rule rendered between turns whenever the conversation switched
// model or reasoning effort. Purely a reference cue for the user — it doesn't
// change anything about how the surrounding turns render. The marker data
// lives in agent state (modelMarkersByTask) and persists across reloads.
export function ModelChangeDivider({ marker }) {
  const builtinModels = useAgent((s) => s.models);
  const customMap = useCustomModels((s) => s.models);

  const label = useMemo(
    () => resolveModelLabel(marker.modelId, builtinModels, customMap),
    [marker.modelId, builtinModels, customMap],
  );
  const effort = TIER_LABELS[marker.thinkingTier] || null;

  return (
    <div className="mx-auto flex w-full max-w-3xl select-none items-center gap-3 px-6 py-3">
      <div className="h-px flex-1 bg-border" />
      <div className="flex items-center gap-1.5 rounded-full border border-border/70 bg-muted/40 px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
        <ArrowLeftRight className="size-3 shrink-0 text-primary" />
        <span className="text-foreground/80">{label}</span>
        {effort && (
          <>
            <span className="opacity-40">·</span>
            <span>{effort}</span>
          </>
        )}
      </div>
      <div className="h-px flex-1 bg-border" />
    </div>
  );
}

export default ModelChangeDivider;
