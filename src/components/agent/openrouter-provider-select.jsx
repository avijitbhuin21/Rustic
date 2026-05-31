import React, { useEffect, useMemo, useState } from 'react';
import {
  Loader2,
  ChevronDown,
  ChevronUp,
  X,
  Check,
  Gauge,
  Timer,
  Activity,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  useOpenRouterProviders,
  useOpenRouterAllowlist,
} from '@/state/openrouter';

function fmtPrice(perM) {
  if (perM == null || !Number.isFinite(perM)) return '—';
  if (perM === 0) return 'Free';
  if (perM < 10) return `$${perM.toFixed(2)}`;
  return `$${perM.toFixed(1)}`;
}
function fmtSpeed(tps) {
  if (tps == null || !Number.isFinite(tps)) return null;
  return `${Math.round(tps)} tok/s`;
}
function fmtLatency(ms) {
  if (ms == null || !Number.isFinite(ms)) return null;
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms)}ms`;
}
function fmtUptime(pct) {
  if (pct == null || !Number.isFinite(pct)) return null;
  return `${pct.toFixed(1)}%`;
}

// Line-1 stats that sit right beside the provider name: output speed and the
// in · out price (per 1M tokens).
function ProviderNameStats({ p }) {
  const speed = fmtSpeed(p.throughput_tps);
  return (
    <>
      {speed && (
        <span
          className="flex shrink-0 items-center gap-1 text-[10px] tabular-nums text-muted-foreground"
          title="Median output speed"
        >
          <Gauge className="size-3 shrink-0 opacity-60" />
          {speed}
        </span>
      )}
      <span
        className="shrink-0 text-[10px] tabular-nums text-muted-foreground"
        title="Price / 1M tokens (in · out)"
      >
        {fmtPrice(p.input_cost_per_m)}
        <span className="opacity-50"> in</span>
        {' · '}
        {fmtPrice(p.output_cost_per_m)}
        <span className="opacity-50"> out</span>
      </span>
    </>
  );
}

// Line-2 stats under the provider name: time-to-first-token, uptime, and cache
// read / cache write price (only when the provider reports them — most don't,
// so those values are 0 and we hide them).
function ProviderStats({ p }) {
  const ttft = fmtLatency(p.latency_ms);
  const uptime = fmtUptime(p.uptime_30m);
  const cacheRead = p.cache_read_cost_per_m;
  const cacheWrite = p.cache_write_cost_per_m;
  const hasCacheRead = Number.isFinite(cacheRead) && cacheRead > 0;
  const hasCacheWrite = Number.isFinite(cacheWrite) && cacheWrite > 0;
  return (
    <div className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[10px] tabular-nums text-muted-foreground">
      <span className="flex items-center gap-1" title="Median time to first token">
        <Timer className="size-3 shrink-0 opacity-60" />
        {ttft ?? '—'}
      </span>
      <span className="flex items-center gap-1" title="Uptime, last 30 min">
        <Activity className="size-3 shrink-0 opacity-60" />
        {uptime ?? '—'}
      </span>
      {hasCacheRead && (
        <span title="Cache read price / 1M tokens">
          {fmtPrice(cacheRead)}
          <span className="opacity-50"> cache read</span>
        </span>
      )}
      {hasCacheWrite && (
        <span title="Cache write price / 1M tokens">
          {fmtPrice(cacheWrite)}
          <span className="opacity-50"> cache write</span>
        </span>
      )}
    </div>
  );
}

// Ordered multi-select of the sub-providers serving an OpenRouter model, used
// as the "Provider" field inside the edit-model modal. OpenRouter routes a
// single model id across several upstreams (WandB, Nebius, Fireworks, …); this
// lets the user pin which ones serve the model and in what PRIORITY. The order
// you tick = routing priority (1st ticked = first choice, no fallback). Empty
// selection = all providers eligible (default routing). Selection persists to
// the backend allow-list immediately (no Save needed) via `setForModel`.

function ProviderIcon({ p }) {
  return p.icon_url ? (
    <img
      src={p.icon_url}
      alt=""
      className="size-3.5 shrink-0 rounded-sm"
      loading="lazy"
      onError={(e) => {
        e.currentTarget.style.display = 'none';
      }}
    />
  ) : (
    <div className="size-3.5 shrink-0 rounded-sm bg-foreground/10" />
  );
}

export function OpenRouterProviderSelect({ modelId }) {
  const providers = useOpenRouterProviders((s) => s.byModel[modelId]);
  const loading = useOpenRouterProviders((s) => !!s.loadingByModel[modelId]);
  const error = useOpenRouterProviders((s) => s.errorByModel[modelId]);
  const load = useOpenRouterProviders((s) => s.load);

  const allowed = useOpenRouterAllowlist((s) => s.byModel[modelId]);
  const loadAllowlist = useOpenRouterAllowlist((s) => s.load);
  const setForModel = useOpenRouterAllowlist((s) => s.setForModel);

  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (modelId) load({ modelId }).catch(() => {});
    loadAllowlist().catch(() => {});
  }, [modelId, load, loadAllowlist]);

  const order = useMemo(
    () => (Array.isArray(allowed) ? allowed : []),
    [allowed],
  );
  const restricted = order.length > 0;

  // Selected providers in priority order, resolved against the live list so a
  // stale slug just drops out gracefully.
  const { selected, selectedSet } = useMemo(() => {
    const bySlug = new Map((providers || []).map((p) => [p.provider_slug, p]));
    const sel = order.map((slug) => bySlug.get(slug)).filter(Boolean);
    return { selected: sel, selectedSet: new Set(order) };
  }, [providers, order]);

  // Operate on the resolved order (slugs that still exist) so stale slugs drop
  // out on first edit and never desync the priority numbers.
  const cleanOrder = useMemo(
    () => selected.map((p) => p.provider_slug),
    [selected],
  );

  const toggle = (slug) => {
    if (selectedSet.has(slug)) {
      setForModel(modelId, cleanOrder.filter((s) => s !== slug));
    } else {
      setForModel(modelId, [...cleanOrder, slug]);
    }
  };
  const remove = (slug) => setForModel(modelId, cleanOrder.filter((s) => s !== slug));
  const move = (slug, dir) => {
    const i = cleanOrder.indexOf(slug);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= cleanOrder.length) return;
    const next = [...cleanOrder];
    [next[i], next[j]] = [next[j], next[i]];
    setForModel(modelId, next);
  };

  const priorityOf = useMemo(() => {
    const m = new Map();
    cleanOrder.forEach((slug, idx) => m.set(slug, idx + 1));
    return m;
  }, [cleanOrder]);

  const summary = restricted
    ? selected.map((p) => p.provider_name).join(', ')
    : 'All providers';

  return (
    <div className="flex flex-col gap-1.5">
      {/* Trigger: shows the selected provider names (not just "OpenRouter") and
          opens the checkbox list of every sub-provider. */}
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex h-9 items-center gap-2 rounded-md border border-input bg-transparent px-3 py-1 text-left transition-colors hover:bg-muted/40"
        title="Choose & prioritise providers"
      >
        <span
          className={cn(
            'min-w-0 flex-1 truncate text-sm',
            restricted ? 'text-foreground' : 'text-muted-foreground',
          )}
        >
          {summary}
        </span>
        {restricted && (
          <span className="shrink-0 rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[9px] font-medium text-amber-500">
            {selected.length}
          </span>
        )}
        <ChevronDown
          className="size-4 shrink-0 text-muted-foreground transition-transform duration-200"
          style={{ transform: open ? 'rotate(180deg)' : 'rotate(0deg)' }}
        />
      </button>

      {error && (
        <div className="px-1 text-[11px] italic text-destructive">{error}</div>
      )}
      {!error && loading && (!providers || providers.length === 0) && (
        <div className="flex items-center gap-1.5 px-1 py-1 text-[11px] italic text-muted-foreground">
          <Loader2 className="size-3 shrink-0 animate-spin" />
          Loading providers…
        </div>
      )}

      {/* Checkbox dropdown — tick in the order you want them prioritised. */}
      {open && providers && providers.length > 0 && (
        <div className="flex max-h-56 flex-col gap-0.5 overflow-y-auto rounded-md border border-border/60 bg-popover p-1 explorer-scroll">
          <div className="px-1 py-0.5 text-[10px] text-muted-foreground/70">
            Tick in priority order (1st ticked = first choice)
          </div>
          {providers.map((p) => {
            const checked = selectedSet.has(p.provider_slug);
            return (
              <button
                key={p.provider_slug}
                type="button"
                onClick={() => toggle(p.provider_slug)}
                className={cn(
                  'flex flex-col gap-1 rounded-md px-2 py-1.5 text-left transition-colors',
                  checked ? 'bg-amber-500/10' : 'hover:bg-muted/60',
                )}
              >
                <div className="flex items-center gap-2">
                  <span
                    className={cn(
                      'flex size-4 shrink-0 items-center justify-center rounded border transition-colors',
                      checked
                        ? 'border-amber-500 bg-amber-500 text-background'
                        : 'border-muted-foreground/50 text-transparent',
                    )}
                  >
                    <Check className="size-3" />
                  </span>
                  {checked && (
                    <span
                      className="flex size-4 shrink-0 items-center justify-center rounded-full bg-amber-500/80 text-[9px] font-bold text-background"
                      title={`Priority ${priorityOf.get(p.provider_slug)}`}
                    >
                      {priorityOf.get(p.provider_slug)}
                    </span>
                  )}
                  <ProviderIcon p={p} />
                  <span className="min-w-0 truncate text-xs font-medium text-foreground">
                    {p.provider_name}
                  </span>
                  <ProviderNameStats p={p} />
                  {p.quantization && (
                    <span className="ml-auto shrink-0 rounded bg-foreground/10 px-1 py-0.5 text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
                      {p.quantization}
                    </span>
                  )}
                </div>
                <div className="pl-6">
                  <ProviderStats p={p} />
                </div>
              </button>
            );
          })}
        </div>
      )}

      {/* Resolved priority list with ▲▼ / ✕ controls. */}
      {restricted && (
        <div className="flex flex-col gap-1">
          <div className="px-1 text-[10px] text-muted-foreground/70">
            Routing priority (1 = first choice)
          </div>
          {selected.map((p, idx) => (
            <div
              key={p.provider_slug}
              className="flex flex-col gap-1 rounded-md bg-amber-500/10 px-2 py-1.5"
            >
              <div className="flex items-center gap-2">
                <span
                  className="flex size-4 shrink-0 items-center justify-center rounded-full bg-amber-500/80 text-[9px] font-bold text-background"
                  title={`Priority ${idx + 1}`}
                >
                  {idx + 1}
                </span>
                <ProviderIcon p={p} />
                <span className="min-w-0 truncate text-xs font-medium text-foreground">
                  {p.provider_name}
                </span>
                <ProviderNameStats p={p} />
                {p.quantization && (
                  <span className="shrink-0 rounded bg-foreground/10 px-1 py-0.5 text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
                    {p.quantization}
                  </span>
                )}
                <div className="ml-auto flex items-center gap-0.5">
                  <button
                    type="button"
                    onClick={() => move(p.provider_slug, -1)}
                    disabled={idx === 0}
                    title="Higher priority"
                    className="flex size-4 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-30"
                  >
                    <ChevronUp className="size-3" />
                  </button>
                  <button
                    type="button"
                    onClick={() => move(p.provider_slug, 1)}
                    disabled={idx === selected.length - 1}
                    title="Lower priority"
                    className="flex size-4 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-30"
                  >
                    <ChevronDown className="size-3" />
                  </button>
                  <button
                    type="button"
                    onClick={() => remove(p.provider_slug)}
                    title="Remove from routing"
                    className="flex size-4 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive/20 hover:text-destructive"
                  >
                    <X className="size-3" />
                  </button>
                </div>
              </div>
              <div className="pl-6">
                <ProviderStats p={p} />
              </div>
            </div>
          ))}
        </div>
      )}

      <p className="px-1 text-[10px] leading-snug text-muted-foreground/70">
        {restricted
          ? 'Routed to the ranked providers in order; no fallback to the others.'
          : 'All providers eligible. Select some to pin routing priority (1st, 2nd, …).'}
      </p>
    </div>
  );
}

export default OpenRouterProviderSelect;
