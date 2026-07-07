import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import {
  ArrowUp,
  Check,
  ChevronRight,
  ChevronDown,
  Loader2,
  Mic,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Eye,
  Pencil,
  X,
  Zap,
  SquareTerminal,
  Sparkles,
  Workflow,
  FileText,
  Unlock,
  Target,
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
import { useTerminal } from '@/state/terminal';
import { useCustomModels } from '@/state/custom-models';
import { useLiveModels } from '@/state/live-models';
import { useOpenRouterSpecs } from '@/state/openrouter';
import { tiersForModel } from '@/state/agent';
import { RegisterModelModal } from './register-model-modal';
import { ContextUsageCapsule } from './context-usage-capsule';
import { GoalCapsule } from './goal-capsule';
import { SLASH_COMMANDS, parseSlashCommand } from '@/lib/slash-commands';
import {
  extractImagesFromClipboard,
  readImagesFromAsyncClipboard,
  imageMimeFromPath,
  readFileAsBase64,
  saveImageToUploads,
} from '@/lib/clipboard-image';
import { useClipboard } from '@/state/clipboard';
import { cn } from '@/lib/utils';

// ── Audio helpers ────────────────────────────────────────────────────────────
// The mic records webm/opus, but only OpenAI's Whisper endpoint tolerates that
// container. Gemini (inline_data) and OpenRouter (chat input_audio) require WAV.
// So we decode the recorded blob and re-encode it to 16 kHz mono 16-bit PCM WAV
// — a format every supported provider accepts, and small enough (~2 MB/min) to
// stay well under Gemini's 20 MB inline-request cap. Returns base64 WAV bytes.

// Encode an AudioBuffer (already at the target rate / mono) to a WAV ArrayBuffer.
function encodeWav(audioBuffer) {
  const numChannels = 1;
  const sampleRate = audioBuffer.sampleRate;
  const samples = audioBuffer.getChannelData(0);
  const bytesPerSample = 2; // 16-bit PCM
  const dataSize = samples.length * bytesPerSample;
  const buffer = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buffer);
  const writeStr = (offset, str) => {
    for (let i = 0; i < str.length; i++) view.setUint8(offset + i, str.charCodeAt(i));
  };
  writeStr(0, 'RIFF');
  view.setUint32(4, 36 + dataSize, true);
  writeStr(8, 'WAVE');
  writeStr(12, 'fmt ');
  view.setUint32(16, 16, true); // PCM fmt chunk size
  view.setUint16(20, 1, true); // PCM format
  view.setUint16(22, numChannels, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * numChannels * bytesPerSample, true); // byte rate
  view.setUint16(32, numChannels * bytesPerSample, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  writeStr(36, 'data');
  view.setUint32(40, dataSize, true);
  let offset = 44;
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    offset += 2;
  }
  return buffer;
}

// Decode a recorded audio Blob, downmix + resample to 16 kHz mono, and return
// base64-encoded WAV bytes. Throws if the platform can't decode the blob.
async function blobToWavBase64(blob) {
  const AudioCtx = window.AudioContext || window.webkitAudioContext;
  const OfflineCtx = window.OfflineAudioContext || window.webkitOfflineAudioContext;
  if (!AudioCtx || !OfflineCtx) throw new Error('Web Audio API unavailable');
  const arrayBuffer = await blob.arrayBuffer();
  const decodeCtx = new AudioCtx();
  let decoded;
  try {
    decoded = await decodeCtx.decodeAudioData(arrayBuffer.slice(0));
  } finally {
    decodeCtx.close?.();
  }
  // Resample to 16 kHz mono via an offline render.
  const targetRate = 16000;
  const frames = Math.max(1, Math.ceil(decoded.duration * targetRate));
  const offline = new OfflineCtx(1, frames, targetRate);
  const src = offline.createBufferSource();
  src.buffer = decoded;
  src.connect(offline.destination);
  src.start(0);
  const rendered = await offline.startRendering();
  const wav = encodeWav(rendered);
  const bytes = new Uint8Array(wav);
  let binary = '';
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

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
          className="flex w-6 shrink-0 items-center justify-center border-l border-border/40 text-muted-foreground transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
        >
          <X className="size-3.5" />
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

// A chip representing a terminal the user tagged onto the message. On send, the
// terminal's *current rendered screen* (resolved by the backend headless
// emulator) is captured and prepended to the message as context — so the model
// can answer about a TUI / colorized output the user is looking at.
function TerminalTagChip({ tag, onRemove }) {
  return (
    <div
      className="group inline-flex items-stretch overflow-hidden rounded-md border border-border/60 bg-muted/40"
      title={`Terminal screen will be attached: ${tag.label}`}
    >
      <span className="flex items-center gap-1.5 px-2 py-1 text-[11px] text-foreground/80">
        <SquareTerminal className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="max-w-[160px] truncate">{tag.label}</span>
      </span>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onRemove?.();
        }}
        aria-label={`Remove ${tag.label}`}
        className="flex w-6 shrink-0 items-center justify-center border-l border-border/40 text-muted-foreground transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

// Generic chip for a non-image context attachment (skill, workflow, or a
// referenced file). Mirrors TerminalTagChip's shape but takes its icon and
// label from the caller so all three attachment kinds read consistently.
function ContextChip({ icon: Icon, label, title, onRemove }) {
  return (
    <div
      className="group inline-flex items-stretch overflow-hidden rounded-md border border-border/60 bg-muted/40"
      title={title || label}
    >
      <span className="flex items-center gap-1.5 px-2 py-1 text-[11px] text-foreground/80">
        <Icon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="max-w-[180px] truncate">{label}</span>
      </span>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onRemove?.();
        }}
        aria-label={`Remove ${label}`}
        className="flex w-6 shrink-0 items-center justify-center border-l border-border/40 text-muted-foreground transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

// The inline autocomplete that drops above the textarea when the user types a
// `/` (skills + workflows) or `@` (project files) trigger token. Purely
// presentational + keyboard-driven; the parent owns trigger detection, the
// filtered item list, the active index, and what selecting an item does.
function MentionMenu({ kind, items, activeIndex, onHover, onSelect, query }) {
  if (items.length === 0) {
    const empty =
      kind === 'at'
        ? query
          ? `No files or terminals match "${query}"`
          : 'No files or terminals found.'
        : query
          ? `No skills or workflows match "${query}"`
          : 'No skills or workflows installed.';
    return (
      <div className="absolute bottom-full left-0 z-50 mb-2 w-full overflow-hidden rounded-lg border border-border/70 bg-popover p-2 text-xs text-muted-foreground shadow-[0_8px_30px_rgba(0,0,0,0.24)]">
        {empty}
      </div>
    );
  }
  return (
    <div className="absolute bottom-full left-0 z-50 mb-2 w-full overflow-hidden rounded-lg border border-border/70 bg-popover p-1 shadow-[0_8px_30px_rgba(0,0,0,0.24)]">
      <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
        {kind === 'at' ? 'Reference a file or terminal' : 'Commands, skills & workflows'}
      </div>
      <div className="max-h-64 overflow-y-auto">
        {items.map((item, i) => {
          const active = i === activeIndex;
          const Icon =
            item.kind === 'command'
              ? Target
              : item.kind === 'skill'
                ? Sparkles
                : item.kind === 'workflow'
                  ? Workflow
                  : item.kind === 'terminal'
                    ? SquareTerminal
                    : FileText;
          return (
            <button
              key={`${item.kind}:${item.value}`}
              type="button"
              // Use onMouseDown (not onClick) so the textarea doesn't lose
              // focus + fire its blur/selection handlers before the pick lands.
              onMouseDown={(e) => {
                e.preventDefault();
                onSelect(item);
              }}
              onMouseEnter={() => onHover(i)}
              className={cn(
                'flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs',
                active ? 'bg-muted text-foreground' : 'text-foreground/90 hover:bg-muted/60',
              )}
            >
              <Icon className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{item.label}</span>
              {item.tag && (
                <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
                  {item.tag}
                </span>
              )}
            </button>
          );
        })}
      </div>
    </div>
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


// Stable empty array for the live-ids selector. Without this, `s.byKey[key]
// || []` would allocate a fresh array on every Zustand snapshot read, and
// useSyncExternalStore's reference comparison would treat each as a state
// change — re-rendering forever and tripping React's max-depth guard.
const EMPTY_IDS = [];

// Merge built-in registry entries with live ids and any user-saved custom
// specs into a single sorted, optionally search-filtered list. Pulled out of
// ProviderGroup so the parent can see each group's match count up front and
// hide groups that don't match the active search query.
// Normalise an OpenAI-compatible endpoint URL so two spellings of the same
// base ("https://api.x.ai/v1" vs "https://api.x.ai/v1/") compare equal when we
// scope registered models to their endpoint.
function normBaseUrl(u) {
  return (u || '').trim().replace(/\/+$/, '').toLowerCase();
}

function mergeModelEntries({ builtinModels, liveIds, customMap, searchQuery, includeLive, providerType, baseUrl, orSpecs }) {
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
    if (byId.has(id) || !specProvider) continue;
    if (providerType && specProvider !== providerType) continue;
    // Compatible providers can have several configured endpoints (e.g. a
    // Bifrost router and Grok) that ALL report providerType 'Compatible'. Scope
    // a saved spec to the endpoint it was registered against so a model set up
    // on one endpoint doesn't leak into another's group. Legacy specs saved
    // before per-endpoint scoping carry no baseUrl — keep showing them in every
    // Compatible group so they don't vanish; they get scoped the next time the
    // user registers or edits them.
    if (
      providerType === 'Compatible' &&
      spec.baseUrl &&
      normBaseUrl(spec.baseUrl) !== normBaseUrl(baseUrl)
    ) {
      continue;
    }
    byId.set(id, {
      id,
      label: spec.name || id,
      registered: true,
    });
  }
  // Only merge the live `/v1/models` payload when the caller asked for it
  // (after the user clicks "Browse more models"). Without this gate the
  // popover used to populate registered rows first, then noticeably reflow
  // when the live fetch resolved with unregistered ones.
  if (includeLive) {
    for (const id of liveIds || []) {
      if (!byId.has(id)) {
        const spec = customMap[id];
        // OpenRouter models are auto-registered: we can pull their full spec
        // (cost/context/capabilities) from the catalogue on pick, so they never
        // need the manual Register modal. Prefer the OpenRouter display name.
        const orSpec = orSpecs ? orSpecs[id] : null;
        byId.set(id, {
          id,
          label: spec?.name || orSpec?.name || id,
          registered: !!spec || !!orSpec || providerType === 'OpenRouter',
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
// selecting straight away. The Settings gear opens the edit-model modal, where
// OpenRouter sub-provider selection now lives (the old inline "compare
// providers" panel was removed).
function ModelRow({ providerKey, baseUrl, modelId, label, registered, active, onPick, onEdit }) {
  const handleEditClick = (e) => {
    e.stopPropagation();
    onEdit?.(providerKey, modelId, baseUrl);
  };

  return (
    <button
      type="button"
      onClick={() => onPick(providerKey, modelId, registered, baseUrl)}
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
        {registered && (
          <div
            onClick={handleEditClick}
            className="flex cursor-pointer items-center justify-center rounded p-0.5 opacity-60 transition-opacity hover:bg-muted hover:opacity-100"
            title="Configure model"
            role="button"
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                handleEditClick(e);
              }
            }}
          >
            <Settings className="size-3 shrink-0" />
          </div>
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

// Per-thinking-tier presentation. The tier the user picks tints the model
// pill ("the color of the thing") and drives the node rail's fill colour, so a
// glance at the pill tells you the reasoning budget. Colours go cool→warm as
// the budget climbs. Class strings are literal so Tailwind keeps them.
const TIER_META = {
  off: { label: 'Off', text: 'text-muted-foreground', bg: 'bg-muted-foreground', ring: 'ring-muted-foreground/40' },
  low: { label: 'Low', text: 'text-sky-500', bg: 'bg-sky-500', ring: 'ring-sky-500/40' },
  medium: { label: 'Medium', text: 'text-emerald-500', bg: 'bg-emerald-500', ring: 'ring-emerald-500/40' },
  high: { label: 'High', text: 'text-amber-500', bg: 'bg-amber-500', ring: 'ring-amber-500/40' },
  max: { label: 'Max', text: 'text-rose-500', bg: 'bg-rose-500', ring: 'ring-rose-500/40' },
};
function tierMeta(tier) {
  return TIER_META[tier] || TIER_META.off;
}

// Horizontal "reasoning level" selector: one node per available tier, joined
// by line segments, filling up to the selected tier in that tier's colour.
// Lives in the model popover's footer strip and replaces the old standalone
// reasoning dropdown. Click a node (or its label) to set the tier.
function ThinkingNodeRail({ tiers, value, onChange }) {
  const selectedIndex = Math.max(0, tiers.indexOf(value));
  const sel = tierMeta(value);
  return (
    <div className="flex items-start">
      {tiers.map((t, i) => {
        const meta = tierMeta(t);
        const active = i === selectedIndex;
        const reached = i <= selectedIndex;
        return (
          <React.Fragment key={t}>
            {i > 0 && (
              // Connector segment sits at the node circles' vertical centre
              // (size-5 → 10px from top). Filled in the selected tier's colour
              // up to the active node.
              <div className="mt-[9px] h-0.5 flex-1 rounded-full bg-border">
                <div
                  className={cn('h-full rounded-full transition-all', i <= selectedIndex ? sel.bg : 'bg-transparent')}
                />
              </div>
            )}
            <button
              type="button"
              onClick={() => onChange(t)}
              aria-label={`Thinking: ${meta.label}`}
              aria-pressed={active}
              className="flex shrink-0 flex-col items-center gap-1 px-0.5 focus-visible:outline-none"
            >
              <span
                className={cn(
                  'flex size-5 items-center justify-center rounded-full border-2 transition-all',
                  active
                    ? cn(sel.bg, 'border-transparent ring-2 ring-offset-2 ring-offset-popover', sel.ring)
                    : reached
                      ? cn(sel.bg, 'border-transparent opacity-50')
                      : 'border-border bg-transparent',
                )}
              >
                <span
                  className={cn(
                    'size-1.5 rounded-full transition-colors',
                    reached ? 'bg-white' : 'bg-muted-foreground/40',
                  )}
                />
              </span>
              <span
                className={cn(
                  'text-[9px] font-medium uppercase tracking-wide transition-colors',
                  active ? sel.text : 'text-muted-foreground',
                )}
              >
                {meta.label}
              </span>
            </button>
          </React.Fragment>
        );
      })}
    </div>
  );
}

function ModePopover({ open, onOpenChange, compact = false }) {
  const permissionLevel = useAgent((s) => s.permissionLevel);
  const setPermissionLevel = useAgent((s) => s.setPermissionLevel);
  const sensitiveAccess = useAgent((s) => s.sensitiveAccess);
  const setSensitiveAccess = useAgent((s) => s.setSensitiveAccess);
  const current = MODE_ITEMS.find((m) => m.id === permissionLevel) || MODE_ITEMS[0];
  const currentLabel = current.label;
  const CurrentIcon = current.icon;

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`Mode: ${currentLabel}`}
          title={`Mode: ${currentLabel}`}
          className={cn(
            'flex h-7 items-center text-foreground/90 transition-colors hover:bg-muted hover:text-foreground',
            // Compact = the leading segment of the fused model control: just the
            // mode icon (eye / pencil / bolt). Non-compact keeps the old labelled
            // pill for any standalone use.
            compact
              ? 'justify-center px-2'
              : 'gap-1 rounded-md px-2 text-xs font-medium',
          )}
        >
          {compact ? (
            <CurrentIcon className="size-4 shrink-0" />
          ) : (
            <>
              <span>{currentLabel}</span>
              <ChevronDown className="size-3 shrink-0 opacity-60" />
            </>
          )}
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
                <span className="mt-0.5 text-[11px] italic leading-snug text-muted-foreground">
                  {description}
                </span>
              </div>
              {active && (
                <Check className="mt-0.5 size-3.5 shrink-0 text-primary" />
              )}
            </button>
          );
        })}
        <div className="mt-1 border-t border-border/60 pt-1">
          <button
            type="button"
            onClick={() => setSensitiveAccess(!sensitiveAccess)}
            className="flex w-full items-start gap-2.5 rounded-md p-2 text-left transition-colors hover:bg-muted"
          >
            <Unlock className="mt-0.5 size-4 shrink-0" />
            <div className="flex min-w-0 flex-1 flex-col">
              <span className="text-sm font-medium leading-tight">Grant access to all files</span>
              <span className="mt-0.5 text-[11px] italic leading-snug text-muted-foreground">
                Lets the agent read every file, including .env and other gitignored or sensitive files.
              </span>
            </div>
            <span
              className={cn(
                'mt-0.5 inline-flex h-4 w-7 shrink-0 items-center rounded-full transition-colors',
                sensitiveAccess ? 'bg-primary' : 'bg-muted-foreground/30',
              )}
            >
              <span
                className={cn(
                  'size-3 rounded-full bg-background shadow transition-transform',
                  sensitiveAccess ? 'translate-x-3.5' : 'translate-x-0.5',
                )}
              />
            </span>
          </button>
        </div>
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
  onEdit,
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
                  baseUrl={baseUrl}
                  modelId={e.id}
                  label={e.label}
                  registered={e.registered}
                  active={
                    selectedProvider === providerType && selectedModel === e.id
                  }
                  onPick={onPick}
                  onEdit={onEdit}
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
  const saveCustomModel = useCustomModels((s) => s.save);
  const orSpecs = useOpenRouterSpecs((s) => s.byId);
  const loadOrSpecs = useOpenRouterSpecs((s) => s.load);
  // Reasoning state lives here now: the footer node rail sets it and the
  // trigger pill is tinted by it ("the color of the thing").
  const thinkingTier = useAgent((s) => s.thinkingTier);
  const setThinkingTier = useAgent((s) => s.setThinkingTier);
  const tiers = useMemo(() => tiersForModel(selectedModel), [selectedModel]);
  const tint = tierMeta(thinkingTier);

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

  // When an OpenRouter provider is configured, warm the catalogue once the
  // popover opens. This both populates the auto-register specs and primes the
  // backend cost cache so OpenRouter cost estimates are accurate at send time.
  useEffect(() => {
    if (open && groups.some((g) => g.providerType === 'OpenRouter')) {
      loadOrSpecs().catch(() => {});
    }
  }, [open, groups, loadOrSpecs]);

  // Persist an OpenRouter model's spec (from the catalogue) to the custom-model
  // store and push its inferred capabilities to the backend. Runs on pick/edit
  // so the user never has to hand-fill the Register modal for OpenRouter.
  const autoRegisterOpenRouter = useCallback(
    async (modelId) => {
      if (!modelId || customMap[modelId]) return;
      let spec = orSpecs[modelId];
      if (!spec) {
        const map = await loadOrSpecs();
        spec = map?.[modelId];
      }
      if (!spec) return; // catalogue unavailable — selection still proceeds
      saveCustomModel(modelId, {
        name: spec.name || modelId,
        provider: 'OpenRouter',
        contextWindow: spec.context_window,
        maxOutputTokens: spec.max_output_tokens,
        inputCost: spec.input_cost_per_m,
        outputCost: spec.output_cost_per_m,
        cachedInputCost: spec.cache_read_cost_per_m,
        cachedOutputCost: spec.cache_write_cost_per_m,
      });
      if (isTauri()) {
        try {
          await invoke('set_model_capabilities', {
            modelId,
            supportsTemperature: !!spec.supports_temperature,
            supportsReasoningEffort: !!spec.supports_reasoning_effort,
            supportsAdaptiveThinking: false,
          });
        } catch (e) {}
      }
    },
    [customMap, orSpecs, loadOrSpecs, saveCustomModel],
  );

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
  const loadLive = useLiveModels((s) => s.load);
  const resetAllLive = useLiveModels((s) => s.resetAll);

  // Manual refresh: drop every cached `/v1/models` snapshot and force a fresh
  // pull for the groups whose live list is currently on screen. This is the
  // escape hatch for "I added a model to my router (e.g. Bifrost) but chat
  // still shows the old set" — previously the only fix was to remove and
  // re-add the provider, because both the frontend store and the backend's
  // 5-minute cache held the stale list.
  const [refreshing, setRefreshing] = useState(false);
  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    try {
      await refreshProvidersConfig();
      resetAllLive();
      await Promise.all(
        groups
          .filter((g) => liveLoaded[g.groupKey] || !!searchQuery.trim())
          .map((g) =>
            loadLive({
              key: g.groupKey,
              providerType: g.providerType,
              baseUrl: g.baseUrl,
              force: true,
            }).catch(() => {}),
          ),
      );
    } finally {
      setRefreshing(false);
    }
  }, [
    groups,
    liveLoaded,
    searchQuery,
    refreshProvidersConfig,
    resetAllLive,
    loadLive,
  ]);

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
        baseUrl: g.baseUrl,
        orSpecs: g.providerType === 'OpenRouter' ? orSpecs : null,
        searchQuery,
        // Include live results when the user opted in for this group, or
        // when a search is active (so search reaches every available model,
        // not just registered ones).
        includeLive: !!liveLoaded[g.groupKey] || hasSearch,
      });
    }
    return out;
  }, [groups, liveByKey, customMap, searchQuery, liveLoaded, orSpecs]);

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

  const pick = async (provider, modelId, registered, baseUrl) => {
    if (!registered) {
      setPendingRegister({ providerType: provider, modelId, baseUrl });
      return;
    }
    // OpenRouter models are auto-registered (no manual modal): silently persist
    // their spec + capabilities so cost/context display and the backend cost
    // cache stay accurate.
    if (provider === 'OpenRouter') {
      await autoRegisterOpenRouter(modelId);
    }
    setSelectedModel(provider, modelId);
    // Intentionally do NOT close the popover here — the footer hosts the
    // thinking-tier rail, so the user can pick a model and a reasoning level in
    // one visit. Closing is left to an outside click (Radix default).
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
            className={cn(
              'flex h-7 max-w-[200px] items-center gap-1 px-2 text-xs font-medium transition-colors hover:bg-muted',
              // "the color of the thing": tint the model name by the active
              // reasoning tier. Off stays neutral so an idle pill isn't loud.
              thinkingTier !== 'off' ? tint.text : 'text-foreground/90 hover:text-foreground',
            )}
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
          className="flex max-h-[60vh] w-80 flex-col overflow-hidden p-0"
        >
          <div className="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto p-3">
            <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
              Model
            </div>
            <div className="flex items-center gap-1.5">
              <div className="relative flex-1">
                <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search models…"
                  className="h-7 w-full rounded-md border border-input bg-transparent pl-7 pr-2 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/50"
                />
              </div>
              <button
                type="button"
                onClick={handleRefresh}
                disabled={refreshing}
                title="Refresh model lists"
                aria-label="Refresh model lists"
                className="flex size-7 shrink-0 items-center justify-center rounded-md border border-input text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
              >
                <RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} />
              </button>
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
                onEdit={async (provider, modelId, baseUrl) => {
                  // Seed the OpenRouter spec first so the Register modal opens
                  // pre-filled instead of blank.
                  if (provider === 'OpenRouter') {
                    await autoRegisterOpenRouter(modelId);
                  }
                  setPendingRegister({ providerType: provider, modelId, baseUrl });
                }}
              />
            ))}
            </div>
          </div>
          {/* Footer strip — mirrors the edit-model modal's footer. Holds the
              reasoning-level node rail, pinned below the scrollable model list. */}
          <div className="shrink-0 border-t border-border/60 px-3 py-2.5">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                Thinking
              </span>
              <span className={cn('text-[10px] font-medium uppercase tracking-wide', tint.text)}>
                {tint.label}
              </span>
            </div>
            <ThinkingNodeRail tiers={tiers} value={thinkingTier} onChange={setThinkingTier} />
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
        baseUrl={pendingRegister?.baseUrl}
        onSaved={async () => {
          const p = pendingRegister;
          setPendingRegister(null);
          if (!p) return;
          setSelectedModel(p.providerType, p.modelId);
          // Keep the model popover open after registering too (the Register
          // modal closes itself) so the thinking rail stays reachable.
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

function RunningStatusInline({ startedAt, toolName, toolCount }) {
  /** Compact working indicator shown beside the model selector while the agent runs. */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const secs = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : null;
  const elapsed =
    secs != null
      ? `${String(Math.floor(secs / 60)).padStart(2, '0')}:${String(secs % 60).padStart(2, '0')}`
      : null;
  return (
    <div className="flex min-w-0 items-center gap-1.5 px-1.5 text-[11px] text-muted-foreground">
      <span className="relative flex size-2 shrink-0">
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary/50" />
        <span className="relative inline-flex size-2 rounded-full bg-primary/80" />
      </span>
      <span className="shrink-0 font-medium">Working</span>
      {elapsed && <span className="shrink-0 tabular-nums">{elapsed}</span>}
      {toolName && <span className="min-w-0 truncate font-mono">· {toolName}</span>}
      {toolCount > 0 && (
        <span className="shrink-0">
          · {toolCount} tool{toolCount > 1 ? 's' : ''}
        </span>
      )}
    </div>
  );
}

export function PromptBox({
  onSubmit,
  onAbort,
  isStreaming = false,
  runInfo = null,
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
  const [modelOpen, setModelOpen] = useState(false);
  const [modeOpen, setModeOpen] = useState(false);
  // Per-message image attachments built up by pasting screenshots into the
  // textarea. Each entry carries the data URL (for the in-prompt preview),
  // base64 + media_type (for the backend send_message payload), and the
  // on-disk path under `<project>/.rustic/uploaded/...` so the model can
  // reference the file by path in follow-up turns.
  const [attachments, setAttachments] = useState([]);
  // Terminals the user tagged onto this message: [{ id, label }]. Their
  // rendered screen is captured fresh at send time (see submit).
  const [terminalTags, setTerminalTags] = useState([]);
  const readTerminalScreen = useTerminal((s) => s.readTerminalScreen);
  // Live terminals, surfaced inside the "@" menu alongside files so they can be
  // attached the same way (type "@", then a name/pid, pick one).
  const terminalSessions = useTerminal((s) => s.sessions);
  const hiddenSessionIds = useTerminal((s) => s.hiddenSessionIds);
  const textareaRef = useRef(null);

  // ── Audio input (speech-to-text) ─────────────────────────────────────────
  // The mic only appears once an "audio input agent" is configured in Settings.
  // We read that flag from get_ai_config on mount and refresh it on window
  // focus (so toggling it in Settings reflects without an app restart).
  const [audioEnabled, setAudioEnabled] = useState(false);
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const mediaRecorderRef = useRef(null);
  const mediaStreamRef = useRef(null);
  const audioChunksRef = useRef([]);
  // PromptBox is mounted twice (hero + chat-dock), so the global
  // `audio-transcript-delta` event reaches both listeners. Only the instance
  // that actually started a transcription should consume the deltas — this ref
  // gates that (a state read inside the once-bound listener would be stale).
  const transcribingRef = useRef(false);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const refreshAudioFlag = async () => {
      try {
        const cfg = await invoke('get_ai_config');
        const ai = cfg?.audio_input;
        if (!cancelled) setAudioEnabled(!!(ai && ai.provider_key && ai.model));
      } catch {
        if (!cancelled) setAudioEnabled(false);
      }
    };
    refreshAudioFlag();
    // Window `focus` covers switching back from another app. Settings is an
    // in-app panel (no focus change), so it also fires `audio-input-changed`
    // on save/clear — listen for both so the mic appears/disappears without a
    // manual page refresh.
    window.addEventListener('focus', refreshAudioFlag);
    window.addEventListener('audio-input-changed', refreshAudioFlag);
    return () => {
      cancelled = true;
      window.removeEventListener('focus', refreshAudioFlag);
      window.removeEventListener('audio-input-changed', refreshAudioFlag);
    };
  }, []);

  // Stream transcript deltas straight into the textarea as they arrive.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten;
    // `listen()` is async but the cleanup is sync. Under StrictMode (and any
    // fast remount) the first effect's cleanup runs while this promise is still
    // pending — `unlisten` is undefined, so it no-ops and the listener leaks.
    // The leaked + live listeners both append every delta → transcript doubled
    // ("hey how are you hey how are you"). This flag makes the stale effect tear
    // its own listener down the instant the promise resolves.
    let cancelled = false;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('audio-transcript-delta', (e) => {
        if (!transcribingRef.current) return; // not the recording instance
        const t = e?.payload?.text;
        if (t) {
          setValue((v) => v + t);
          requestAnimationFrame(() => textareaRef.current?.focus());
        }
      }))
      .then((fn) => {
        if (cancelled) { try { fn(); } catch {} return; }
        unlisten = fn;
      });
    return () => { cancelled = true; try { unlisten?.(); } catch {} };
  }, []);

  const stopRecording = useCallback(() => {
    try { mediaRecorderRef.current?.stop(); } catch {}
    mediaRecorderRef.current = null;
  }, []);

  // Starter-prompt chips on the welcome screen insert their text here. Only
  // the hero instance listens — the chips are only rendered next to it, and
  // gating on variant keeps a docked instance from also consuming the event.
  useEffect(() => {
    if (variant !== 'hero') return undefined;
    const onInsert = (e) => {
      const text = e?.detail?.text;
      if (typeof text !== 'string' || !text) return;
      setValue((v) => (v ? `${v} ${text}` : text));
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (el) {
          el.focus();
          el.setSelectionRange(el.value.length, el.value.length);
        }
      });
    };
    window.addEventListener('prompt-insert', onInsert);
    return () => window.removeEventListener('prompt-insert', onInsert);
  }, [variant]);

  const startRecording = useCallback(async () => {
    if (recording) return;
    if (!navigator.mediaDevices?.getUserMedia) {
      toast.error('Microphone is not available in this environment.');
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      mediaStreamRef.current = stream;
      const mr = new MediaRecorder(stream);
      audioChunksRef.current = [];
      mr.ondataavailable = (e) => { if (e.data && e.data.size > 0) audioChunksRef.current.push(e.data); };
      mr.onstop = async () => {
        const mime = mr.mimeType || 'audio/webm';
        const blob = new Blob(audioChunksRef.current, { type: mime });
        audioChunksRef.current = [];
        mediaStreamRef.current?.getTracks().forEach((t) => t.stop());
        mediaStreamRef.current = null;
        setRecording(false);
        if (blob.size === 0) return;
        transcribingRef.current = true;
        setTranscribing(true);
        try {
          // Re-encode to WAV so every provider (Gemini/OpenRouter need it,
          // OpenAI accepts it) can transcribe the clip. If decode fails, fall
          // back to shipping the raw recording (OpenAI's Whisper tolerates webm).
          let audioBase64;
          let outMime = 'audio/wav';
          try {
            audioBase64 = await blobToWavBase64(blob);
          } catch (encErr) {
            console.warn('[audio] WAV re-encode failed, sending raw clip:', encErr);
            const bytes = new Uint8Array(await blob.arrayBuffer());
            let binary = '';
            const CHUNK = 0x8000;
            for (let i = 0; i < bytes.length; i += CHUNK) {
              binary += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
            }
            audioBase64 = btoa(binary);
            outMime = mime;
          }
          await invoke('transcribe_audio', { audioBase64, mime: outMime });
        } catch (err) {
          toast.error(`Transcription failed: ${err}`);
        } finally {
          transcribingRef.current = false;
          setTranscribing(false);
          requestAnimationFrame(() => textareaRef.current?.focus());
        }
      };
      mr.start();
      mediaRecorderRef.current = mr;
      setRecording(true);
    } catch (err) {
      toast.error(`Microphone unavailable: ${err}`);
    }
  }, [recording]);

  // ── `/` and `@` mention attachments ──────────────────────────────────────
  // Skills/workflows picked from the "/" menu — their full body is injected
  // into this turn's system prompt by the backend (passed by name via extras).
  const [skillTags, setSkillTags] = useState([]); // [{ name, description }]
  const [workflowTags, setWorkflowTags] = useState([]); // [{ name, description }]
  // Slash command picked from the "/" menu, shown as a chip; the typed text
  // becomes the command's argument (e.g. the /goal completion condition).
  const [commandTag, setCommandTag] = useState(null); // { name }
  // Files picked from the "@" menu — passed by REFERENCE (path) only so the
  // model reads them itself with read_file rather than us dumping contents.
  const [fileTags, setFileTags] = useState([]); // [{ id, relativePath }]

  // Data sources for the menus. Skills/workflows are cheap, fetched once on
  // mount. The project file list can be large, so it's fetched lazily the
  // first time the "@" menu opens and cached for the rest of the session.
  const [skills, setSkills] = useState([]);
  const [workflows, setWorkflows] = useState([]);
  const [projectFiles, setProjectFiles] = useState(null); // null = not yet loaded
  const filesLoadingRef = useRef(false);

  // Active mention menu: { type: 'slash' | 'at', triggerIndex, query } or null.
  const [menu, setMenu] = useState(null);
  const [menuIndex, setMenuIndex] = useState(0);

  const activeProjectRoot = useAgent((s) => s.activeProject?.root || '');
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const pendingDraft = useAgent((s) => s.pendingDraft);
  const clearPendingDraft = useAgent((s) => s.clearPendingDraft);

  const activeProjectId = useAgent((s) => s.activeProject?.id ?? 'none');
  const setPromptDraft = useAgent((s) => s.setPromptDraft);
  const draftKey = activeTaskId ? `task:${activeTaskId}` : `new:${activeProjectId}`;
  const skipDraftPersistRef = useRef(false);

  useEffect(() => {
    // A pendingDraft for this task outranks the stored draft: reverting the
    // FIRST message empties the chat, which swaps in the hero PromptBox — a
    // fresh mount. The pendingDraft effect above has already seeded the text;
    // loading the (empty) stored draft here would wipe it.
    const pd = useAgent.getState().pendingDraft;
    if (pd && (!pd.taskId || pd.taskId === activeTaskId)) return;
    const draft = useAgent.getState().promptDrafts[draftKey];
    skipDraftPersistRef.current = true;
    setValue(draft?.value || '');
    setAttachments(Array.isArray(draft?.attachments) ? draft.attachments : []);
    setTerminalTags(Array.isArray(draft?.terminalTags) ? draft.terminalTags : []);
    setSkillTags(Array.isArray(draft?.skillTags) ? draft.skillTags : []);
    setWorkflowTags(Array.isArray(draft?.workflowTags) ? draft.workflowTags : []);
    setFileTags(Array.isArray(draft?.fileTags) ? draft.fileTags : []);
    setCommandTag(draft?.commandTag || null);
  }, [draftKey]);

  // Seed the prompt with a pending draft (set by RevertButton after a
  // chat+files revert). We re-apply whenever pendingDraft references the
  // active task; clear it once applied so a second prompt mount can't
  // re-populate. PromptBox is rendered twice in chat-view (hero +
  // chat-dock variants); only the one matching the active task should
  // pick the draft up — and `clearPendingDraft` guarantees only one of
  // them actually wins the race. MUST stay BELOW the stored-draft load
  // effect: on a fresh mount (first-message revert swaps in the hero box)
  // both run, and the pending text has to be applied after — not wiped by
  // — the stored-draft load.
  useEffect(() => {
    if (!pendingDraft) return;
    if (pendingDraft.taskId && pendingDraft.taskId !== activeTaskId) return;
    setValue(pendingDraft.text || '');
    setAttachments(Array.isArray(pendingDraft.attachments) ? pendingDraft.attachments : []);
    clearPendingDraft();
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [pendingDraft, activeTaskId, clearPendingDraft]);

  useEffect(() => {
    if (skipDraftPersistRef.current) {
      skipDraftPersistRef.current = false;
      return;
    }
    setPromptDraft(draftKey, {
      value,
      attachments,
      terminalTags,
      skillTags,
      workflowTags,
      fileTags,
      commandTag,
    });
  }, [draftKey, value, attachments, terminalTags, skillTags, workflowTags, fileTags, commandTag, setPromptDraft]);

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

  // Load the skill + workflow catalogues once. Both are small (names +
  // descriptions); the full body is resolved on the backend at send time.
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    (async () => {
      try {
        const [sk, wf] = await Promise.all([
          invoke('list_skills').catch(() => []),
          invoke('list_workflows').catch(() => []),
        ]);
        if (!alive) return;
        setSkills(Array.isArray(sk) ? sk : []);
        setWorkflows(Array.isArray(wf) ? wf : []);
      } catch (e) {
        /* non-fatal — the "/" menu just shows "none installed" */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // Lazily fetch the project file list the first time the "@" menu is needed.
  const ensureProjectFiles = useCallback(async () => {
    if (projectFiles !== null || filesLoadingRef.current) return;
    if (!isTauri() || !activeProjectRoot) {
      setProjectFiles([]);
      return;
    }
    filesLoadingRef.current = true;
    try {
      const files = await invoke('list_project_files', {
        rootPath: activeProjectRoot,
        maxFiles: 5000,
      });
      setProjectFiles(Array.isArray(files) ? files : []);
    } catch (e) {
      setProjectFiles([]);
    } finally {
      filesLoadingRef.current = false;
    }
  }, [projectFiles, activeProjectRoot]);

  // Inspect the text immediately before the caret for an active `/` or `@`
  // trigger token. A token is live when its trigger char sits at the start of
  // the input or just after whitespace, and nothing but non-whitespace follows
  // it up to the caret. Returns the menu descriptor or null.
  const detectMention = useCallback((text, caret) => {
    const upto = text.slice(0, caret);
    // Walk back from the caret to the trigger or a whitespace boundary.
    let i = caret - 1;
    while (i >= 0) {
      const ch = upto[i];
      if (ch === '@' || ch === '/') break;
      if (ch === ' ' || ch === '\n' || ch === '\t') return null;
      i -= 1;
    }
    if (i < 0) return null;
    const trigger = upto[i];
    const prev = i > 0 ? upto[i - 1] : '';
    const atBoundary = i === 0 || prev === ' ' || prev === '\n' || prev === '\t';
    if (!atBoundary) return null;
    return {
      type: trigger === '@' ? 'at' : 'slash',
      triggerIndex: i,
      query: upto.slice(i + 1),
    };
  }, []);

  // Recompute the active menu from the textarea's current value + caret.
  const refreshMenu = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    const next = detectMention(el.value, el.selectionStart ?? el.value.length);
    setMenu(next);
    if (next?.type === 'at') ensureProjectFiles();
  }, [detectMention, ensureProjectFiles]);

  // Reset the highlighted row whenever the trigger token (position or kind)
  // changes, so a fresh "/" or "@" always starts at the top of its list.
  useEffect(() => {
    setMenuIndex(0);
  }, [menu?.triggerIndex, menu?.type]);

  // The filtered, capped item list backing the open menu. Each item carries a
  // `kind` (skill/workflow/file), the `value` used to build the chip, a
  // display `label`, and an optional `tag` badge.
  const menuItems = useMemo(() => {
    if (!menu) return [];
    const q = menu.query.trim().toLowerCase();
    if (menu.type === 'slash') {
      const commandItems = SLASH_COMMANDS.filter((c) => commandTag?.name !== c.name).map((c) => ({
        kind: 'command',
        value: c.name,
        label: `/${c.name}`,
        description: c.description || '',
        tag: 'command',
      }));
      const skillItems = skills
        .filter((s) => !skillTags.some((t) => t.name === s.name))
        .map((s) => ({
          kind: 'skill',
          value: s.name,
          label: s.name,
          description: s.description || '',
          tag: 'skill',
        }));
      const workflowItems = workflows
        .filter((w) => !workflowTags.some((t) => t.name === w.name))
        .map((w) => ({
          kind: 'workflow',
          value: w.name,
          label: w.name,
          description: w.description || '',
          tag: 'workflow',
        }));
      const all = [...commandItems, ...skillItems, ...workflowItems];
      const filtered = q
        ? all.filter(
            (it) =>
              it.label.toLowerCase().includes(q) ||
              it.description.toLowerCase().includes(q),
          )
        : all;
      return filtered.slice(0, 50);
    }
    // `@` → terminals first, then files. Terminals are matchable by the word
    // "terminal", their label, and their pid; files by path. Listing terminals
    // first keeps them reachable even when the project has thousands of files.
    const terminalItems = terminalSessions
      .filter((s) => !hiddenSessionIds.has(s.id))
      .filter((s) => !terminalTags.some((t) => t.id === s.id))
      .map((s) => {
        const label = s.label || `pty ${s.id}`;
        const pid = s.pid != null ? String(s.pid) : '';
        return {
          kind: 'terminal',
          value: String(s.id),
          sessionId: s.id,
          label,
          // Searchable haystack: typing "terminal", a name, or a pid all hit.
          description: `terminal ${label} ${pid}`,
          tag: pid ? `pid ${pid}` : null,
        };
      });
    const terminalsFiltered = q
      ? terminalItems.filter((it) => it.description.toLowerCase().includes(q))
      : terminalItems;

    const taken = new Set(fileTags.map((f) => f.relativePath));
    const list = (projectFiles || []).filter((p) => !taken.has(p));
    const filesFiltered = (q ? list.filter((p) => p.toLowerCase().includes(q)) : list)
      .slice(0, 50)
      .map((p) => {
        const slash = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
        return {
          kind: 'file',
          value: p,
          label: slash >= 0 ? p.slice(slash + 1) : p,
          description: p,
          tag: null,
        };
      });
    return [...terminalsFiltered, ...filesFiltered];
  }, [menu, skills, workflows, projectFiles, skillTags, workflowTags, fileTags, terminalSessions, hiddenSessionIds, terminalTags, commandTag]);

  const closeMenu = useCallback(() => {
    setMenu(null);
    setMenuIndex(0);
  }, []);

  // Apply a picked menu item: strip the trigger token from the textarea and
  // add the corresponding attachment chip.
  const acceptMention = useCallback(
    (item) => {
      if (!menu) return;
      const el = textareaRef.current;
      const caret = el ? el.selectionStart ?? value.length : value.length;
      const before = value.slice(0, menu.triggerIndex);
      const after = value.slice(caret);
      const nextValue = `${before}${after}`;
      setValue(nextValue);

      if (item.kind === 'skill') {
        setSkillTags((prev) =>
          prev.some((t) => t.name === item.value)
            ? prev
            : [...prev, { name: item.value, description: item.description }],
        );
      } else if (item.kind === 'workflow') {
        setWorkflowTags((prev) =>
          prev.some((t) => t.name === item.value)
            ? prev
            : [...prev, { name: item.value, description: item.description }],
        );
      } else if (item.kind === 'command') {
        setCommandTag({ name: item.value });
      } else if (item.kind === 'file') {
        setFileTags((prev) =>
          prev.some((t) => t.relativePath === item.value)
            ? prev
            : [...prev, { id: `file-${item.value}`, relativePath: item.value }],
        );
      } else if (item.kind === 'terminal') {
        addTerminalTag({ id: item.sessionId, label: item.label });
      }

      closeMenu();
      // Restore focus + caret to where the token used to start.
      requestAnimationFrame(() => {
        const node = textareaRef.current;
        if (!node) return;
        node.focus();
        const pos = before.length;
        node.setSelectionRange(pos, pos);
      });
    },
    [menu, value, closeMenu],
  );

  const removeSkillTag = (name) =>
    setSkillTags((prev) => prev.filter((t) => t.name !== name));
  const removeWorkflowTag = (name) =>
    setWorkflowTags((prev) => prev.filter((t) => t.name !== name));
  const removeFileTag = (rel) =>
    setFileTags((prev) => prev.filter((t) => t.relativePath !== rel));

  // Attach image files that were "Copy"-ed inside the app's own explorer.
  // That copy never reaches the OS clipboard in the web build, so a paste in
  // the chat would otherwise silently do nothing (the main iPad complaint).
  const attachFromInAppClipboard = async () => {
    const { paths } = useClipboard.getState();
    const imagePaths = (paths || []).filter((p) => imageMimeFromPath(p));
    if (imagePaths.length === 0) return false;
    for (const p of imagePaths) {
      try {
        const res = await invoke('read_file_base64', { path: p });
        const mediaType = imageMimeFromPath(p) || 'image/png';
        const filename = p.split(/[\\/]/).pop() || 'image';
        let relativePath = p;
        if (activeProjectRoot) {
          const normRoot = activeProjectRoot.replace(/[\\/]+$/, '');
          if (p.startsWith(normRoot)) {
            relativePath = p.slice(normRoot.length).replace(/^[\\/]+/, '').replace(/\\/g, '/');
          }
        }
        setAttachments((prev) => [
          ...prev,
          {
            id: `att-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
            name: filename,
            url: `data:${mediaType};base64,${res.data}`,
            mediaType,
            base64Data: res.data,
            path: p,
            relativePath,
          },
        ]);
      } catch (err) {
        const msg = typeof err === 'string' ? err : err?.message || String(err);
        toast.error(`Couldn't attach ${p.split(/[\\/]/).pop()}: ${msg}`);
      }
    }
    return true;
  };

  const handlePaste = async (e) => {
    let images = extractImagesFromClipboard(e.clipboardData);
    if (images.length === 0) {
      // Nothing usable in the synchronous event. If there's text, let the
      // default paste run. Otherwise this is likely Safari/iPadOS, which only
      // exposes copied images through the async clipboard API — or an in-app
      // explorer "Copy" that never touched the OS clipboard at all.
      const types = Array.from(e.clipboardData?.types || []);
      if (types.includes('text/plain') || types.includes('text/html')) return;
      e.preventDefault();
      images = await readImagesFromAsyncClipboard();
      if (images.length === 0) {
        await attachFromInAppClipboard();
        return;
      }
    } else {
      // We're handling these images — don't let the default behaviour also dump
      // the raw `[object File]` placeholder into the textarea.
      e.preventDefault();
    }
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

  const addTerminalTag = (tag) => {
    setTerminalTags((prev) =>
      prev.some((t) => t.id === tag.id) ? prev : [...prev, tag],
    );
  };
  const removeTerminalTag = (id) => {
    setTerminalTags((prev) => prev.filter((t) => t.id !== id));
  };

  // Build the context block prepended to a message for each tagged terminal.
  // Captures the rendered screen fresh so it reflects the terminal's state at
  // the moment of sending, not when it was tagged.
  const captureTerminalContext = async () => {
    if (terminalTags.length === 0) return '';
    const blocks = [];
    for (const tag of terminalTags) {
      let screen;
      try {
        screen = await readTerminalScreen(tag.id);
      } catch (err) {
        screen = `(could not read terminal — it may have closed: ${
          typeof err === 'string' ? err : err?.message || String(err)
        })`;
      }
      const body = (screen || '').trim() || '(screen is empty)';
      blocks.push(
        `Current screen of terminal "${tag.label}":\n\`\`\`\n${body}\n\`\`\``,
      );
    }
    return blocks.join('\n\n');
  };

  const submit = async () => {
    const trimmed = value.trim();
    // Registered slash commands are intercepted here — they act on the app
    // instead of being sent to the model as a prompt. Two entry points: the
    // /goal chip picked from the menu (args = the typed text), or the literal
    // "/goal …" typed out by hand.
    const slash = commandTag
      ? { command: commandTag.name, args: trimmed }
      : parseSlashCommand(trimmed);
    if (slash?.command === 'goal') {
      setValue('');
      setCommandTag(null);
      closeMenu();
      const agent = useAgent.getState();
      if (!slash.args) {
        toast.info('Usage: /goal <condition> — or /goal clear to cancel');
      } else if (/^(clear|stop|off|cancel|none)$/i.test(slash.args)) {
        agent.clearGoal();
      } else {
        agent.setGoal(slash.args);
      }
      return;
    }
    const hasAny =
      trimmed ||
      attachments.length > 0 ||
      terminalTags.length > 0 ||
      skillTags.length > 0 ||
      workflowTags.length > 0 ||
      fileTags.length > 0;
    if (!hasAny) return;

    const context = await captureTerminalContext();
    const finalText = context
      ? (trimmed ? `${context}\n\n${trimmed}` : context)
      : trimmed;

    onSubmit?.(finalText, attachments, {
      skills: skillTags.map((t) => t.name),
      workflows: workflowTags.map((t) => t.name),
      fileTags: fileTags.map((t) => ({ relativePath: t.relativePath })),
    });
    setValue('');
    setAttachments([]);
    setTerminalTags([]);
    setSkillTags([]);
    setWorkflowTags([]);
    setFileTags([]);
    closeMenu();
  };

  const onKeyDown = (e) => {
    // When a mention menu is open, capture the navigation keys so they drive
    // the menu instead of the textarea / submit.
    if (menu && menuItems.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setMenuIndex((i) => (i + 1) % menuItems.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setMenuIndex((i) => (i - 1 + menuItems.length) % menuItems.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        acceptMention(menuItems[Math.min(menuIndex, menuItems.length - 1)]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closeMenu();
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  const hasContent =
    value.trim() !== '' ||
    attachments.length > 0 ||
    terminalTags.length > 0 ||
    skillTags.length > 0 ||
    workflowTags.length > 0 ||
    fileTags.length > 0 ||
    !!commandTag;
  const isHero = variant === 'hero';

  // The send button doubles as a mic when the composer is empty and an audio
  // input model is configured. Recording/transcribing take over the same
  // button so the user's eye doesn't have to move.
  const showMic = !isStreaming && !recording && !transcribing && !hasContent && audioEnabled;

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
      {(attachments.length > 0 ||
        terminalTags.length > 0 ||
        skillTags.length > 0 ||
        workflowTags.length > 0 ||
        fileTags.length > 0 ||
        commandTag) && (
        <div className="mb-1 flex flex-wrap gap-1.5 px-1 pt-1">
          {commandTag && (
            <ContextChip
              icon={Target}
              label={`/${commandTag.name}`}
              title="Goal command — the message text becomes the completion condition"
              onRemove={() => setCommandTag(null)}
            />
          )}
          {attachments.map((att) => (
            <AttachmentChip
              key={att.id}
              attachment={att}
              onRemove={() => removeAttachment(att.id)}
            />
          ))}
          {terminalTags.map((tag) => (
            <TerminalTagChip
              key={tag.id}
              tag={tag}
              onRemove={() => removeTerminalTag(tag.id)}
            />
          ))}
          {skillTags.map((tag) => (
            <ContextChip
              key={`skill:${tag.name}`}
              icon={Sparkles}
              label={tag.name}
              title={`Skill injected into the system prompt: ${tag.name}`}
              onRemove={() => removeSkillTag(tag.name)}
            />
          ))}
          {workflowTags.map((tag) => (
            <ContextChip
              key={`workflow:${tag.name}`}
              icon={Workflow}
              label={tag.name}
              title={`Workflow injected into the system prompt: ${tag.name}`}
              onRemove={() => removeWorkflowTag(tag.name)}
            />
          ))}
          {fileTags.map((tag) => (
            <ContextChip
              key={tag.id}
              icon={FileText}
              label={tag.relativePath}
              title={`File referenced (the agent reads it itself): ${tag.relativePath}`}
              onRemove={() => removeFileTag(tag.relativePath)}
            />
          ))}
        </div>
      )}
      <div className="relative">
        {menu && (
          <MentionMenu
            kind={menu.type === 'at' ? 'at' : 'slash'}
            items={menuItems}
            activeIndex={menuIndex}
            query={menu.query}
            onHover={setMenuIndex}
            onSelect={acceptMention}
          />
        )}
        <textarea
          ref={textareaRef}
          rows={1}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            // Defer so selectionStart reflects the post-change caret.
            requestAnimationFrame(refreshMenu);
          }}
          onKeyUp={(e) => {
            // Caret moves (arrows/home/end) don't fire onChange — re-detect the
            // active token, but let the open-menu navigation keys pass through.
            if (
              menu &&
              ['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(e.key)
            )
              return;
            refreshMenu();
          }}
          onClick={refreshMenu}
          onBlur={() => {
            // Close on blur, but a tick later so a menu item's onMouseDown pick
            // resolves first.
            setTimeout(() => closeMenu(), 120);
          }}
          onKeyDown={onKeyDown}
          onPaste={handlePaste}
          placeholder={commandTag?.name === 'goal'
            ? 'Describe the completion condition… (or type "clear")'
            : placeholder}
          disabled={disabled}
          className={cn(
            'flex min-h-[44px] w-full resize-none rounded-md border-none bg-transparent px-3 py-2 text-xs leading-relaxed text-foreground placeholder:text-muted-foreground',
            'focus-visible:outline-none focus-visible:ring-0 disabled:cursor-not-allowed disabled:opacity-50',
          )}
        />
      </div>

      <div className="flex items-center justify-between gap-2 p-0 pt-2">
        <div className="flex min-w-0 items-center gap-0.5">
          {/* Fused control: [mode icon] | [model name]. The mode icon opens the
              mode menu; the model name opens the model popover (which now hosts
              the reasoning-level node rail in its footer). The model name is
              tinted by the active thinking tier. */}
          <div className="inline-flex items-center overflow-hidden rounded-md border border-border/60 bg-muted/20">
            <ModePopover open={modeOpen} onOpenChange={setModeOpen} compact />
            <div className="h-4 w-px bg-border/60" />
            <ModelPopover open={modelOpen} onOpenChange={setModelOpen} />
          </div>
          {isStreaming && runInfo && (
            <RunningStatusInline
              startedAt={runInfo.startedAt}
              toolName={runInfo.runningTool}
              toolCount={runInfo.toolCount}
            />
          )}
        </div>

        <div className="flex items-center gap-1.5">
          <GoalCapsule />
          <ContextUsageCapsule />
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => {
                  if (isStreaming) { onAbort?.(); return; }
                  if (recording) { stopRecording(); return; }
                  if (hasContent) { submit(); return; }
                  if (audioEnabled && !transcribing) { startRecording(); return; }
                }}
              disabled={
                !isStreaming && !recording &&
                (transcribing || (hasContent ? disabled : !audioEnabled))
              }
              aria-label={
                isStreaming ? 'Stop generating'
                  : recording ? 'Stop recording'
                  : transcribing ? 'Transcribing'
                  : hasContent ? 'Send'
                  : showMic ? 'Record audio'
                  : 'Send'
              }
              className={cn(
                'flex size-7 items-center justify-center rounded-full transition-all duration-200',
                isStreaming
                  // Destructive-tinted stop pill — same slot as the send
                  // button so the eye doesn't have to move, but reads as
                  // "stop" via colour + filled square. Ring keeps it visually
                  // distinct from the recording state's solid red.
                  ? 'bg-destructive text-white hover:bg-destructive/85 ring-2 ring-destructive/25 animate-pulse'
                  : recording
                    // Recording: red, pulsing, click to stop.
                    ? 'bg-red-500 text-white hover:bg-red-500/85 animate-pulse'
                    : hasContent
                      ? 'bg-foreground text-background hover:bg-foreground/85'
                      : 'bg-transparent text-muted-foreground',
                (!isStreaming && !recording && transcribing) && 'opacity-60',
                (!isStreaming && !recording && !transcribing && !hasContent && !audioEnabled) && 'opacity-60',
              )}
            >
              {isStreaming ? (
                <span className="size-2.5 rounded-[2px] bg-white" />
              ) : recording ? (
                <span className="size-2.5 rounded-[2px] bg-white" />
              ) : transcribing ? (
                <Loader2 className="size-4 animate-spin" />
              ) : showMic ? (
                <Mic className="size-4" />
              ) : (
                <ArrowUp className="size-4" />
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">
            {isStreaming ? 'Stop generating'
              : recording ? 'Stop recording'
              : transcribing ? 'Transcribing…'
              : hasContent ? 'Send'
              : showMic ? 'Record audio'
              : 'Type a message'}
          </TooltipContent>
          </Tooltip>
        </div>
      </div>
    </div>
  );
}

export default PromptBox;
