import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ArrowUp,
  Check,
  ChevronRight,
  ChevronDown,
  Loader2,
  Plus,
  Search,
  Eye,
  Pencil,
  X,
  Zap,
} from 'lucide-react';
import { toast } from 'sonner';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { useAgent } from '@/state/agent';
import { useCustomModels } from '@/state/custom-models';
import { useLiveModels } from '@/state/live-models';
import { tiersForModel } from '@/state/agent';
import { RegisterModelModal } from './register-model-modal';
import {
  extractImagesFromClipboard,
  readFileAsBase64,
  saveImageToUploads,
} from '@/lib/clipboard-image';
import { cn } from '@/lib/utils';

// Draft attachment chip with a clickable thumbnail that opens a full-screen
// lightbox. The X button removes the attachment; clicking the thumbnail
// opens the full image. stopPropagation on the remove button so a click on
// the X doesn't also fire the lightbox.
function AttachmentChip({ attachment: att, onRemove }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <div
        className="group relative inline-flex items-stretch overflow-hidden rounded-md border border-border/60 bg-muted/40"
        title={att.relativePath || att.name}
      >
        {/* The whole chip body — thumbnail + filename — is one click target
            that opens the lightbox. The X button is a sibling so clicking
            it doesn't also trigger the lightbox. */}
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-label={`Open ${att.name} full size`}
          className="flex cursor-zoom-in items-center gap-1.5 px-1 py-1 pr-2 text-left hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
        >
          <img
            src={att.url}
            alt={att.name}
            className="size-8 shrink-0 rounded object-cover"
          />
          <span className="max-w-[140px] truncate text-[11px] text-foreground/80">
            {att.name}
          </span>
        </button>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onRemove?.();
          }}
          aria-label={`Remove ${att.name}`}
          className="flex w-6 shrink-0 items-center justify-center border-l border-border/40 text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
        >
          <X className="size-3" />
        </button>
      </div>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          showCloseButton={false}
          className="w-screen max-w-[100vw] gap-0 border-none bg-transparent p-0 ring-0 shadow-none sm:max-w-[100vw]"
        >
          <DialogTitle className="sr-only">Image Preview</DialogTitle>
          <div
            className="flex h-screen w-screen cursor-zoom-out items-center justify-center p-6"
            onClick={() => setOpen(false)}
          >
            <img
              src={att.url}
              alt={att.name}
              onClick={(e) => e.stopPropagation()}
              className="max-h-[92vh] max-w-[92vw] cursor-default rounded-md object-contain shadow-2xl"
            />
          </div>
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-label="Close image"
            className="fixed right-4 top-4 z-[60] flex size-10 items-center justify-center rounded-full bg-background/70 text-foreground shadow-md backdrop-blur hover:bg-background"
          >
            <X className="size-5" />
          </button>
        </DialogContent>
      </Dialog>
    </>
  );
}

// Rustic-themed prompt box used in the agent chat dock.
//
// Layout:
//   ┌──────────────────────────────────────────────┐
//   │ Ask the agent…                                │
//   │                                               │
//   ├──────────────────────────────────────────────┤
//   │ [Tools] [Model ⌄] [Reasoning] [Project] [↑]   │
//   └──────────────────────────────────────────────┘
//
// Model picker lives in the action row between Tools and Reasoning so the
// user can see what they're talking to without looking away from where they
// type. Providers are collapsed by default inside the popover; expand to see
// model lists. Attachments were removed earlier at the user's request.

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

// Stable empty array for the live-ids selector. Without this, `s.byKey[key]
// || []` would allocate a fresh array on every Zustand snapshot read, and
// useSyncExternalStore's reference comparison would treat each as a state
// change — re-rendering forever and tripping React's max-depth guard.
const EMPTY_IDS = [];

// Merge built-in registry entries with live ids and any user-saved custom
// specs into a single sorted, optionally search-filtered list. Pulled out of
// ProviderGroup so the parent can see each group's match count up front and
// hide groups that don't match the active search query.
function mergeModelEntries({ builtinModels, liveIds, customMap, searchQuery, includeLive, providerType }) {
  const byId = new Map();
  for (const m of builtinModels || []) {
    const id = m.id || m.model_id;
    byId.set(id, {
      id,
      label: m.name || m.display_name || id,
      registered: true,
    });
  }
  // User-saved custom models are also "registered" — surface them inside their
  // provider's group WITHOUT needing to click "Browse more models" first. The
  // RegisterModelModal saves the spec with a `provider` field (older specs may
  // use `provider_type`), so match either against this group's providerType —
  // checking the wrong field here was the bug that left set-up models stuck
  // inside "Browse more models".
  for (const [id, spec] of Object.entries(customMap || {})) {
    const specProvider = spec?.provider || spec?.provider_type;
    if (!byId.has(id) && specProvider && (!providerType || specProvider === providerType)) {
      byId.set(id, {
        id,
        label: spec.name || id,
        registered: true,
      });
    }
  }
  // Only merge the live `/v1/models` payload when the caller asked for it
  // (after the user clicks "Browse more models"). Without this gate the
  // popover used to populate registered rows first, then noticeably reflow
  // when the live fetch resolved with unregistered ones.
  if (includeLive) {
    for (const id of liveIds || []) {
      if (!byId.has(id)) {
        const spec = customMap[id];
        byId.set(id, {
          id,
          label: spec?.name || id,
          registered: !!spec,
        });
      }
    }
  }
  const list = Array.from(byId.values()).sort((a, b) =>
    a.label.localeCompare(b.label),
  );
  const q = (searchQuery || '').trim().toLowerCase();
  if (!q) return list;
  return list.filter(
    (e) =>
      e.id.toLowerCase().includes(q) ||
      e.label.toLowerCase().includes(q),
  );
}

// Provider keys match the backend ProviderType enum strings used by both
// `get_ai_config` and `fetch_ai_models`. The canonical order is the order
// users see in the picker; anything not in this list still renders, just
// after the canonical entries.
const PROVIDER_ORDER = [
  'Claude',
  'OpenAi',
  'Gemini',
  'OpenRouter',
  'Compatible',
];
const PROVIDER_LABELS = {
  Claude: 'Anthropic',
  OpenAi: 'OpenAI',
  Gemini: 'Google',
  OpenRouter: 'OpenRouter',
  Compatible: 'OpenAI-Compatible',
};

function prettifyProvider(key) {
  if (PROVIDER_LABELS[key]) return PROVIDER_LABELS[key];
  if (!key) return 'Other';
  return key
    .split(/[-_]/g)
    .map((s) => s.charAt(0).toUpperCase() + s.slice(1))
    .join(' ');
}

// One row inside a provider group. Shows the model name, an active check, and
// a small "needs setup" badge if the model has no built-in spec and no
// user-saved spec — clicking it then opens the Register modal instead of
// selecting straight away.
function ModelRow({ providerKey, modelId, label, registered, active, onPick }) {
  return (
    <button
      type="button"
      onClick={() => onPick(providerKey, modelId, registered)}
      className={cn(
        'flex h-7 w-full items-center justify-between gap-2 rounded-md pl-7 pr-2 text-xs transition-colors',
        active
          ? 'bg-amber-500/15 text-amber-500'
          : 'text-foreground hover:bg-muted',
      )}
    >
      <span className="truncate">{label}</span>
      <div className="flex items-center gap-1.5">
        {!registered && (
          <span className="rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wide text-amber-500">
            Setup
          </span>
        )}
        {active && <Check className="size-3.5 shrink-0" />}
      </div>
    </button>
  );
}

// Three user-facing permission modes the picker exposes. Maps onto the
// backend's PermissionLevel enum: Chat (read-only), ManualEdit (asks before
// each write), FullAuto (bypass all permission prompts including shell + MCP
// + sub-agents). The backend also has an AutoEdit tier that's intentionally
// omitted — three modes is the cleaner mental model the user asked for.
const MODE_ITEMS = [
  {
    id: 'Chat',
    label: 'Chat',
    icon: Eye,
    description: 'Read-only — answers questions, never writes.',
  },
  {
    id: 'ManualEdit',
    label: 'Edit',
    icon: Pencil,
    description: 'Can edit files but asks before each write.',
  },
  {
    id: 'FullAuto',
    label: 'Auto',
    icon: Zap,
    description: 'No prompts — reads, writes, runs shell, spawns sub-agents.',
  },
];

const MODE_LABELS = Object.fromEntries(MODE_ITEMS.map((m) => [m.id, m.label]));

function ModePopover({ open, onOpenChange }) {
  const permissionLevel = useAgent((s) => s.permissionLevel);
  const setPermissionLevel = useAgent((s) => s.setPermissionLevel);
  const currentLabel = MODE_LABELS[permissionLevel] || 'Mode';

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Mode"
          className="flex h-7 items-center gap-1 rounded-md px-2 text-xs font-medium text-foreground/90 transition-colors hover:bg-muted hover:text-foreground"
        >
          <span>{currentLabel}</span>
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="start"
        sideOffset={8}
        className="w-64 gap-1 p-1"
      >
        <div className="px-2 pt-1 pb-0.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
          Mode
        </div>
        {MODE_ITEMS.map(({ id, label, icon: Icon, description }) => {
          const active = permissionLevel === id;
          return (
            <button
              key={id}
              type="button"
              onClick={() => {
                setPermissionLevel(id);
                onOpenChange(false);
              }}
              className={cn(
                'flex w-full items-start gap-2.5 rounded-md p-2 text-left transition-colors',
                active
                  ? 'bg-primary/10 text-primary'
                  : 'text-foreground hover:bg-muted',
              )}
            >
              <Icon className="mt-0.5 size-4 shrink-0" />
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="text-sm font-medium leading-tight">
                  {label}
                </span>
                <span className="mt-0.5 text-[11px] leading-snug text-muted-foreground">
                  {description}
                </span>
              </div>
              {active && (
                <Check className="mt-0.5 size-3.5 shrink-0 text-primary" />
              )}
            </button>
          );
        })}
      </PopoverContent>
    </Popover>
  );
}

function ProviderGroup({
  groupKey,
  providerType,
  baseUrl,
  label,
  entries,
  expanded,
  onToggle,
  liveLoaded,
  onLoadLive,
  selectedProvider,
  selectedModel,
  onPick,
  forceShowAll,
}) {
  const loading = useLiveModels((s) => !!s.loadingByKey[groupKey]);
  const error = useLiveModels((s) => s.errorByKey[groupKey]);
  const loadLive = useLiveModels((s) => s.load);

  // Defer mounting the row list until the user has actually expanded this
  // group at least once. Without this, every provider's hundreds of
  // ModelRow elements reconcile into the DOM the moment the popover opens
  // (just hidden under grid-rows:0fr), which is what made the model picker
  // feel laggy. Once mounted we keep them in the tree so the collapse
  // transition still looks smooth on subsequent toggles.
  const [hasMounted, setHasMounted] = useState(expanded);
  useEffect(() => {
    if (expanded && !hasMounted) setHasMounted(true);
  }, [expanded, hasMounted]);

  // Live `/v1/models` fetch now only fires when the user explicitly opts in
  // via the "Browse more models" row OR when a search forces the full list
  // open. Expanding alone no longer triggers it — that's what caused the
  // "registered models first, then everything else suddenly appears" reflow.
  useEffect(() => {
    if (liveLoaded || forceShowAll) {
      loadLive({ key: groupKey, providerType, baseUrl }).catch(() => {});
    }
  }, [liveLoaded, forceShowAll, groupKey, providerType, baseUrl, loadLive]);

  const showAll = liveLoaded || forceShowAll;

  return (
    <div className="flex flex-col">
      <button
        type="button"
        onClick={onToggle}
        className="flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-foreground transition-colors hover:bg-muted"
      >
        <ChevronRight
          className="size-3 shrink-0 transition-transform duration-200"
          style={{ transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
        />
        <span className="font-medium">{label}</span>
        <span className="ml-auto text-[10px] text-muted-foreground">
          {loading ? '…' : hasMounted ? entries.length || '' : ''}
        </span>
      </button>
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 240ms ease, opacity 200ms ease',
          opacity: expanded ? 1 : 0,
        }}
      >
        <div style={{ overflow: 'hidden' }}>
          {hasMounted && (
            <>
              {error && (
                <div className="px-6 py-1.5 text-[11px] italic text-destructive">
                  {error}
                </div>
              )}
              {!error && loading && entries.length === 0 && (
                <div className="px-6 py-1.5 text-[11px] italic text-muted-foreground">
                  Loading…
                </div>
              )}
              {!error && !loading && entries.length === 0 && !showAll && (
                <div className="px-6 py-1.5 text-[11px] italic text-muted-foreground">
                  No registered models — click below to browse.
                </div>
              )}
              {!error && !loading && entries.length === 0 && showAll && (
                <div className="px-6 py-1.5 text-[11px] italic text-muted-foreground">
                  No models available
                </div>
              )}
              {entries.map((e) => (
                <ModelRow
                  key={`${groupKey}::${e.id}`}
                  providerKey={providerType}
                  modelId={e.id}
                  label={e.label}
                  registered={e.registered}
                  active={
                    selectedProvider === providerType && selectedModel === e.id
                  }
                  onPick={onPick}
                />
              ))}
              {!showAll && (
                <button
                  type="button"
                  onClick={onLoadLive}
                  className="flex h-7 w-full items-center gap-1.5 rounded-md pl-7 pr-2 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                >
                  <Plus className="size-3 shrink-0" />
                  <span>Browse more models…</span>
                </button>
              )}
              {showAll && loading && entries.length > 0 && (
                <div className="flex h-7 items-center gap-1.5 pl-7 pr-2 text-[11px] italic text-muted-foreground">
                  <Loader2 className="size-3 shrink-0 animate-spin" />
                  Loading additional models…
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ModelPopover({ open, onOpenChange }) {
  const builtinModels = useAgent((s) => s.models);
  const providersConfig = useAgent((s) => s.providersConfig);
  const selectedModel = useAgent((s) => s.selectedModel);
  const selectedProvider = useAgent((s) => s.selectedProvider);
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const setSelectedModel = useAgent((s) => s.setSelectedModel);
  const refreshProvidersConfig = useAgent((s) => s.refreshProvidersConfig);
  const customMap = useCustomModels((s) => s.models);

  // Ensure we have a fresh provider config when the popover opens — the user
  // may have added/removed an API key in settings between opens, and we want
  // those changes reflected without a reload.
  useEffect(() => {
    if (open) refreshProvidersConfig();
  }, [open, refreshProvidersConfig]);

  // Build the list of groups to render. Each provider type contributes a
  // group; Compatible providers contribute one group per configured instance
  // so multiple base_urls don't get merged into a single fetch.
  const groups = useMemo(() => {
    const builtinByProvider = {};
    for (const m of builtinModels || []) {
      const p = m.provider || 'unknown';
      if (!builtinByProvider[p]) builtinByProvider[p] = [];
      builtinByProvider[p].push(m);
    }

    const out = [];
    const seen = new Set();
    // First pass: providers explicitly configured by the user, in canonical order.
    for (const provider of [
      ...PROVIDER_ORDER,
      ...providersConfig
        .map((p) => p.provider_type)
        .filter((t) => !PROVIDER_ORDER.includes(t)),
    ]) {
      const configured = providersConfig.filter(
        (p) => p.provider_type === provider && p.has_api_key,
      );
      if (configured.length === 0) continue;

      if (provider === 'Compatible') {
        // One group per configured Compatible instance.
        for (const inst of configured) {
          const suffix = inst.name ? ` — ${inst.name}` : '';
          const groupKey = `Compatible:${inst.name || 'default'}`;
          if (seen.has(groupKey)) continue;
          seen.add(groupKey);
          out.push({
            groupKey,
            providerType: 'Compatible',
            baseUrl: inst.base_url || null,
            label: `${PROVIDER_LABELS.Compatible}${suffix}`,
            builtinModels: builtinByProvider[provider] || [],
          });
        }
      } else {
        const groupKey = provider;
        if (seen.has(groupKey)) continue;
        seen.add(groupKey);
        out.push({
          groupKey,
          providerType: provider,
          baseUrl: null,
          label: prettifyProvider(provider),
          builtinModels: builtinByProvider[provider] || [],
        });
      }
    }
    return out;
  }, [providersConfig, builtinModels]);

  // Default everything collapsed; auto-expand the selected provider so the
  // current pick is visible on first open without an extra click.
  const [expanded, setExpanded] = useState({});
  // Per-group opt-in for the provider's full live `/v1/models` list. Until
  // the user clicks "Browse more models" we only show registered models
  // (built-in registry + custom specs), keeping the popover snappy and
  // avoiding the reflow when the live fetch resolves.
  const [liveLoaded, setLiveLoaded] = useState({});
  useEffect(() => {
    if (open && selectedProvider) {
      // Compatible's group key includes the instance name so we can't
      // auto-expand without that disambiguation — skip it.
      if (selectedProvider !== 'Compatible') {
        setExpanded((prev) =>
          prev[selectedProvider] ? prev : { ...prev, [selectedProvider]: true },
        );
      }
    }
  }, [open, selectedProvider]);
  // Reset the "browse more" opt-in each time the popover closes so the next
  // open starts in the curated state again. Cached live results stay in the
  // useLiveModels store so re-opting in is instant.
  useEffect(() => {
    if (!open) setLiveLoaded({});
  }, [open]);

  // Free-text filter applied across all groups. When set we also force every
  // non-flat group to render expanded so matches are visible without an extra
  // click — the auto-expand effect kicks in once the inner ProviderGroup mounts
  // and triggers its live-models fetch on demand.
  const [searchQuery, setSearchQuery] = useState('');
  useEffect(() => {
    if (!open) setSearchQuery('');
  }, [open]);

  // Subscribe to the full live-models map so we can compute each non-flat
  // group's merged entries up here. Zustand returns the same byKey object
  // reference unless one of its entries changes, so this stays cheap.
  const liveByKey = useLiveModels((s) => s.byKey);
  const loadingByKey = useLiveModels((s) => s.loadingByKey);

  // Per-group post-search entry lists. Driving the search-aware visibility
  // and the inner rendering from one source so the parent's "hide empty"
  // logic always matches what the child would have rendered.
  const groupEntries = useMemo(() => {
    const out = {};
    const hasSearch = !!searchQuery.trim();
    for (const g of groups) {
      out[g.groupKey] = mergeModelEntries({
        builtinModels: g.builtinModels,
        liveIds: liveByKey[g.groupKey] ?? EMPTY_IDS,
        customMap,
        providerType: g.providerType,
        searchQuery,
        // Include live results when the user opted in for this group, or
        // when a search is active (so search reaches every available model,
        // not just registered ones).
        includeLive: !!liveLoaded[g.groupKey] || hasSearch,
      });
    }
    return out;
  }, [groups, liveByKey, customMap, searchQuery, liveLoaded]);

  const visibleGroups = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return groups;
    return groups.filter((g) => {
      // Keep groups visible while loading (live results may still arrive),
      // or once they have any matching entry. Hide groups whose fetch
      // finished with zero matches so the popover isn't padded with empty
      // "No models available" rows.
      const matched = (groupEntries[g.groupKey] || []).length > 0;
      const live = liveByKey[g.groupKey];
      const stillLoading = !!loadingByKey[g.groupKey] && live == null;
      return matched || stillLoading;
    });
  }, [groups, groupEntries, searchQuery, liveByKey, loadingByKey]);

  // Modal handoff: when the user picks an unregistered model we stash it here
  // and render the Register dialog. After save the dialog calls onSaved with
  // the model id, which then runs the normal pick flow.
  const [pendingRegister, setPendingRegister] = useState(null);

  const pick = async (provider, modelId, registered) => {
    if (!registered) {
      setPendingRegister({ providerType: provider, modelId });
      return;
    }
    setSelectedModel(provider, modelId);
    onOpenChange(false);
    if (activeTaskId && isTauri()) {
      try {
        await invoke('switch_model', {
          taskId: activeTaskId,
          providerType: provider,
          model: modelId,
        });
      } catch (e) {}
    }
  };

  const currentLabel = useMemo(() => {
    if (!selectedModel) return 'Model';
    const builtin = (builtinModels || []).find(
      (mm) => (mm.id || mm.model_id) === selectedModel,
    );
    if (builtin) return builtin.name || builtin.display_name || selectedModel;
    const custom = customMap[selectedModel];
    if (custom?.name) return custom.name;
    return selectedModel;
  }, [builtinModels, selectedModel, customMap]);

  return (
    <>
      <Popover open={open} onOpenChange={onOpenChange}>
        <PopoverTrigger asChild>
          <button
            type="button"
            aria-label="Model"
            className="flex h-7 max-w-[200px] items-center gap-1 rounded-md px-2 text-xs font-medium text-foreground/90 transition-colors hover:bg-muted hover:text-foreground"
          >
            <span className="truncate">
              {selectedModel ? currentLabel : 'Model'}
            </span>
            <ChevronDown className="size-3 shrink-0 opacity-60" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          side="top"
          align="start"
          className="max-h-[60vh] w-80 overflow-y-auto gap-1.5"
        >
          <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
            Model
          </div>
          <div className="relative">
            <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search models…"
              className="h-7 w-full rounded-md border border-input bg-transparent pl-7 pr-2 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/50"
            />
          </div>
          {groups.length === 0 && (
            <div className="px-2 py-1.5 text-xs text-muted-foreground">
              No providers configured. Add one in Settings → Agent.
            </div>
          )}
          {groups.length > 0 && visibleGroups.length === 0 && (
            <div className="px-2 py-1.5 text-xs italic text-muted-foreground">
              No matches for "{searchQuery}"
            </div>
          )}
          <div className="flex flex-col gap-0.5">
            {visibleGroups.map((g) => (
              <ProviderGroup
                key={g.groupKey}
                groupKey={g.groupKey}
                providerType={g.providerType}
                baseUrl={g.baseUrl}
                label={g.label}
                entries={groupEntries[g.groupKey] || EMPTY_IDS}
                expanded={!!expanded[g.groupKey] || !!searchQuery.trim()}
                onToggle={() =>
                  setExpanded((prev) => ({
                    ...prev,
                    [g.groupKey]: !prev[g.groupKey],
                  }))
                }
                liveLoaded={!!liveLoaded[g.groupKey]}
                onLoadLive={() =>
                  setLiveLoaded((prev) => ({ ...prev, [g.groupKey]: true }))
                }
                forceShowAll={!!searchQuery.trim()}
                selectedProvider={selectedProvider}
                selectedModel={selectedModel}
                onPick={pick}
              />
            ))}
          </div>
        </PopoverContent>
      </Popover>

      <RegisterModelModal
        open={!!pendingRegister}
        onOpenChange={(o) => {
          if (!o) setPendingRegister(null);
        }}
        modelId={pendingRegister?.modelId}
        providerType={pendingRegister?.providerType}
        onSaved={async () => {
          const p = pendingRegister;
          setPendingRegister(null);
          if (!p) return;
          setSelectedModel(p.providerType, p.modelId);
          onOpenChange(false);
          if (activeTaskId && isTauri()) {
            try {
              await invoke('switch_model', {
                taskId: activeTaskId,
                providerType: p.providerType,
                model: p.modelId,
              });
            } catch (e) {}
          }
        }}
      />
    </>
  );
}

function ThinkPopover({ open, onOpenChange, tier, setTier, modelId }) {
  const tiers = useMemo(() => tiersForModel(modelId), [modelId]);
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Reasoning mode"
          className="flex h-7 items-center gap-1 rounded-md px-2 text-xs font-medium text-foreground/90 transition-colors hover:bg-muted hover:text-foreground"
        >
          <span className="capitalize">
            {tier !== 'off' ? tier : 'Think'}
          </span>
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="w-52 gap-1">
        <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          Reasoning mode
        </div>
        <div className="flex flex-col gap-0.5 pt-1">
          {tiers.map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => {
                setTier(t);
                onOpenChange(false);
              }}
              className={cn(
                'flex h-7 items-center justify-between rounded-md px-2 text-xs capitalize transition-colors',
                tier === t
                  ? 'bg-violet-500/15 text-violet-500'
                  : 'text-foreground hover:bg-muted',
              )}
            >
              <span>{t === 'off' ? 'Off' : t}</span>
              {tier === t && <Check className="size-3.5" />}
            </button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function PromptBox({
  onSubmit,
  onAbort,
  isStreaming = false,
  disabled = false,
  placeholder = 'Ask the agent…',
  variant = 'default',
  className,
  autoFocus = false,
  chatStarted = false,
  // When the agent tool dock sits flush on top of the prompt, we flatten the
  // prompt's top corners so the two read as one fused container. Callers
  // that render the prompt standalone (welcome hero) leave this false.
  flatTop = false,
}) {
  const [value, setValue] = useState('');
  const [thinkOpen, setThinkOpen] = useState(false);
  const [modelOpen, setModelOpen] = useState(false);
  const [modeOpen, setModeOpen] = useState(false);
  // Per-message image attachments built up by pasting screenshots into the
  // textarea. Each entry carries the data URL (for the in-prompt preview),
  // base64 + media_type (for the backend send_message payload), and the
  // on-disk path under `<project>/.rustic/uploaded/...` so the model can
  // reference the file by path in follow-up turns.
  const [attachments, setAttachments] = useState([]);
  const textareaRef = useRef(null);

  const thinkingTier = useAgent((s) => s.thinkingTier);
  const setThinkingTier = useAgent((s) => s.setThinkingTier);
  const selectedModel = useAgent((s) => s.selectedModel);
  const activeProjectRoot = useAgent((s) => s.activeProject?.root || '');
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const pendingDraft = useAgent((s) => s.pendingDraft);
  const clearPendingDraft = useAgent((s) => s.clearPendingDraft);

  // Seed the prompt with a pending draft (set by RevertButton after a
  // chat+files revert). We re-apply whenever pendingDraft references the
  // active task; clear it once applied so a second prompt mount can't
  // re-populate. PromptBox is rendered twice in chat-view (hero +
  // chat-dock variants); only the one matching the active task should
  // pick the draft up — and `clearPendingDraft` guarantees only one of
  // them actually wins the race.
  useEffect(() => {
    if (!pendingDraft) return;
    if (pendingDraft.taskId && pendingDraft.taskId !== activeTaskId) return;
    setValue(pendingDraft.text || '');
    setAttachments(Array.isArray(pendingDraft.attachments) ? pendingDraft.attachments : []);
    clearPendingDraft();
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [pendingDraft, activeTaskId, clearPendingDraft]);

  const autoGrow = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }, []);

  useEffect(() => {
    autoGrow();
  }, [value, autoGrow]);

  useEffect(() => {
    if (autoFocus) textareaRef.current?.focus();
  }, [autoFocus]);

  const handlePaste = async (e) => {
    const images = extractImagesFromClipboard(e.clipboardData);
    if (images.length === 0) return;
    // We're handling these images — don't let the default behaviour also dump
    // the raw `[object File]` placeholder into the textarea.
    e.preventDefault();
    if (!activeProjectRoot) {
      toast.error('Open a project before pasting an image.');
      return;
    }
    for (const { file, mediaType } of images) {
      try {
        const { base64, dataUrl } = await readFileAsBase64(file);
        const { absolutePath, relativePath, filename } = await saveImageToUploads({
          projectRoot: activeProjectRoot,
          base64,
        });
        setAttachments((prev) => [
          ...prev,
          {
            id: `att-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
            name: filename,
            url: dataUrl,
            mediaType,
            base64Data: base64,
            path: absolutePath,
            relativePath,
          },
        ]);
      } catch (err) {
        const msg = typeof err === 'string' ? err : err?.message || String(err);
        toast.error(`Couldn't attach image: ${msg}`);
      }
    }
  };

  const removeAttachment = (id) => {
    setAttachments((prev) => prev.filter((a) => a.id !== id));
  };

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0) return;
    onSubmit?.(trimmed, attachments);
    setValue('');
    setAttachments([]);
  };

  const onKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  const hasContent = value.trim() !== '' || attachments.length > 0;
  const isHero = variant === 'hero';

  return (
    <div
      className={cn(
        'rounded-3xl border border-border/70 bg-popover p-2 shadow-[0_8px_30px_rgba(0,0,0,0.24)] transition-all duration-300',
        // Flatten the top edge when the dock is sitting above this prompt,
        // so the two share a single rounded shell with no visible seam.
        flatTop && 'rounded-t-none border-t-0',
        isHero && 'w-full',
        className,
      )}
    >
      {attachments.length > 0 && (
        <div className="mb-1 flex flex-wrap gap-1.5 px-1 pt-1">
          {attachments.map((att) => (
            <AttachmentChip
              key={att.id}
              attachment={att}
              onRemove={() => removeAttachment(att.id)}
            />
          ))}
        </div>
      )}
      <textarea
        ref={textareaRef}
        rows={1}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
        onPaste={handlePaste}
        placeholder={placeholder}
        disabled={disabled}
        className={cn(
          'flex min-h-[44px] w-full resize-none rounded-md border-none bg-transparent px-3 py-2 text-xs leading-relaxed text-foreground placeholder:text-muted-foreground',
          'focus-visible:outline-none focus-visible:ring-0 disabled:cursor-not-allowed disabled:opacity-50',
        )}
      />

      <div className="flex items-center justify-between gap-2 p-0 pt-2">
        <div className="flex items-center gap-0.5">
          <ModePopover open={modeOpen} onOpenChange={setModeOpen} />
          <ModelPopover open={modelOpen} onOpenChange={setModelOpen} />
          <ThinkPopover
            open={thinkOpen}
            onOpenChange={setThinkOpen}
            tier={thinkingTier}
            setTier={setThinkingTier}
            modelId={selectedModel}
          />
        </div>

        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => {
                if (isStreaming) onAbort?.();
                else submit();
              }}
              disabled={!isStreaming && (disabled || !hasContent)}
              aria-label={isStreaming ? 'Stop' : 'Send'}
              className={cn(
                'flex size-7 items-center justify-center rounded-full transition-all duration-200',
                isStreaming
                  // Solid dark pill with a filled inner square — same affordance
                  // shape as the send button (so the user's eye doesn't have to
                  // jump), but unmistakably "stop" via the filled square. The
                  // subtle pulse makes the streaming state legible.
                  ? 'bg-foreground text-background hover:bg-foreground/85 animate-pulse'
                  : hasContent
                    ? 'bg-foreground text-background hover:bg-foreground/85'
                    : 'bg-transparent text-muted-foreground',
                (!isStreaming && (disabled || !hasContent)) && 'opacity-60',
              )}
            >
              {isStreaming ? (
                <span className="size-2.5 rounded-[2px] bg-background" />
              ) : (
                <ArrowUp className="size-4" />
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">
            {isStreaming ? 'Stop' : hasContent ? 'Send' : 'Type a message'}
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
}

export default PromptBox;
