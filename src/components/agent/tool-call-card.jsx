import React, { useMemo, useState } from 'react';
import {
  CheckCircle2,
  ChevronRight,
  Circle,
  CircleDotDashed,
  CircleX,
  Eye,
} from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';

function formatValue(v) {
  if (v === undefined || v === null) return '';
  if (typeof v === 'string') return v;
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}

function deriveStatus({ hasResult, isError }) {
  if (!hasResult) return 'in-progress';
  if (isError) return 'failed';
  return 'completed';
}

const STATUS_LABEL = {
  'in-progress': 'running',
  completed: 'done',
  failed: 'failed',
  pending: 'pending',
};

const STATUS_BADGE = {
  completed: 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300',
  'in-progress': 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300',
  failed: 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300',
  pending: 'bg-muted text-muted-foreground',
};

// Which input field each batch-capable tool uses for its sub-call array.
// Mirrors the tool schemas in crates/rustic-agent/src/tools/*.rs — keep in sync
// when new batch tools are added.
const BATCH_FIELDS = {
  read_file: 'reads',
  create_file: 'creates',
  edit_file: 'edits',
  grep_search: 'queries',
  glob: 'patterns',
  find_symbol: 'lookups',
  find_references: 'lookups',
  call_sites: 'lookups',
  spawn_subagent: 'agents',
  web_search: 'queries',
  web_fetch: 'fetches',
};

// Pick a short, human-readable label for a single batch entry. Falls back to a
// truncated JSON dump if the tool doesn't have a known title field.
function entryTitle(toolName, entry) {
  if (entry == null) return '';
  if (typeof entry !== 'object') return String(entry);
  switch (toolName) {
    case 'read_file':
    case 'create_file':
    case 'edit_file':
      return entry.path || JSON.stringify(entry);
    case 'grep_search':
    case 'web_search':
      return entry.query || JSON.stringify(entry);
    case 'glob':
      return entry.pattern || JSON.stringify(entry);
    case 'find_symbol':
    case 'find_references':
    case 'call_sites':
      return entry.name || JSON.stringify(entry);
    case 'spawn_subagent':
      return entry.name || entry.prompt || JSON.stringify(entry);
    case 'web_fetch':
      return entry.url || JSON.stringify(entry);
    default:
      return JSON.stringify(entry);
  }
}

// Split a combined batch output into per-entry segments by parsing the
// `=== <tool> entry N: ... ===` separator that read_file / grep_search / glob /
// web_search / web_fetch emit. Returns null when no separators are found
// (atomic batches like edit_file / spawn_subagent / code-intel) so the caller
// can fall back to showing one combined output.
function splitBatchOutput(output, count) {
  if (typeof output !== 'string' || !output) return null;
  const headerRe = /^=== \S+ entry (\d+):.*===\s*$/;
  const lines = output.split('\n');
  const segments = new Array(count).fill('');
  let currentIdx = -1;
  let buffer = [];
  const flush = () => {
    if (currentIdx >= 0 && currentIdx < count) {
      segments[currentIdx] = buffer.join('\n').trim();
    }
  };
  for (const line of lines) {
    const m = headerRe.exec(line);
    if (m) {
      flush();
      currentIdx = parseInt(m[1], 10) - 1;
      buffer = [];
    } else if (currentIdx >= 0) {
      buffer.push(line);
    }
  }
  flush();
  return currentIdx === -1 ? null : segments;
}

// Pull every `Sub-agent '<id>' spawned ...` line out of the spawn_subagent
// tool's output. Backend formats each spawn confirmation that way (see
// crates/rustic-agent/src/tools/subagent_tools.rs:1287). Returns [] when the
// output hasn't arrived yet, so the rows just don't render until then.
function parseSpawnedAgentIds(output) {
  if (typeof output !== 'string' || !output) return [];
  const ids = [];
  const re = /Sub-agent\s+['"]([^'"]+)['"]\s+spawned/g;
  let m;
  while ((m = re.exec(output)) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

// Status chip + click target for one spawned child. Reads its live transcript
// from the agent store and opens the read-only sub-agent chat sheet on click.
// We deliberately don't crash if the sub-agent record hasn't shown up yet —
// the SubagentSpawned event might race against the spawn_subagent tool result
// in either direction, so we render a row regardless and the sheet shows a
// "Waiting for stream…" placeholder if we get there first.
function SpawnedSubagentRow({ agentId }) {
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const sub = useAgent((s) =>
    activeTaskId ? s.subagentsByTask?.[activeTaskId]?.[agentId] : null,
  );
  const openSheet = useAgent((s) => s.openSubagentSheet);

  const status = sub?.status || 'running';
  const model = sub?.model || '';
  const statusCls =
    status === 'completed'
      ? 'text-green-600 dark:text-green-400'
      : status === 'failed'
        ? 'text-red-600 dark:text-red-400'
        : 'text-blue-600 dark:text-blue-400';

  return (
    <motion.button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        if (activeTaskId) openSheet(activeTaskId, agentId);
      }}
      className="group flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-[11px]"
      whileHover={{ backgroundColor: 'rgba(127,127,127,0.08)' }}
      transition={{ duration: 0.15 }}
    >
      <Eye className="size-3 shrink-0 text-muted-foreground group-hover:text-foreground" />
      <span className="shrink-0 font-mono text-foreground/90">
        {agentId.slice(0, 12)}
      </span>
      {model && (
        <span className="shrink-0 text-muted-foreground">· {model}</span>
      )}
      <span className={cn('ml-auto shrink-0 font-medium', statusCls)}>
        {status === 'running' && sub?.lastUpdate
          ? 'streaming…'
          : status}
      </span>
    </motion.button>
  );
}

function SpawnedSubagentList({ output }) {
  const ids = useMemo(() => parseSpawnedAgentIds(output), [output]);
  if (ids.length === 0) return null;
  return (
    <div>
      <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
        Sub-agents ({ids.length}) — click to watch
      </div>
      <div className="space-y-0.5 rounded border border-border/50 bg-muted/30 p-1">
        {ids.map((id) => (
          <SpawnedSubagentRow key={id} agentId={id} />
        ))}
      </div>
    </div>
  );
}

function StatusIcon({ status }) {
  return (
    <AnimatePresence mode="wait" initial={false}>
      <motion.div
        key={status}
        initial={{ opacity: 0, scale: 0.8, rotate: -10 }}
        animate={{ opacity: 1, scale: 1, rotate: 0 }}
        exit={{ opacity: 0, scale: 0.8, rotate: 10 }}
        transition={{ duration: 0.2, ease: [0.2, 0.65, 0.3, 0.9] }}
        className="flex"
      >
        {status === 'completed' ? (
          <CheckCircle2 className="size-4 text-green-500" />
        ) : status === 'in-progress' ? (
          <CircleDotDashed className="size-4 animate-spin text-blue-500 [animation-duration:3s]" />
        ) : status === 'failed' ? (
          <CircleX className="size-4 text-red-500" />
        ) : (
          <Circle className="size-4 text-muted-foreground" />
        )}
      </motion.div>
    </AnimatePresence>
  );
}

const badgeVariants = {
  initial: { scale: 1 },
  animate: {
    scale: [1, 1.08, 1],
    transition: { duration: 0.35, ease: [0.34, 1.56, 0.64, 1] },
  },
};

const panelVariants = {
  hidden: { opacity: 0, height: 0 },
  visible: {
    opacity: 1,
    height: 'auto',
    transition: { duration: 0.25, ease: [0.2, 0.65, 0.3, 0.9] },
  },
  exit: {
    opacity: 0,
    height: 0,
    transition: { duration: 0.2, ease: [0.2, 0.65, 0.3, 0.9] },
  },
};

// One sub-call inside a batch tool call. Click to toggle its own input/output.
// No status badge of its own — the parent batch carries the overall status,
// and per-entry errors aren't reliably separable from the combined output for
// every batch tool. Errors specific to an entry surface in its segment.
function BatchEntryRow({ index, title, input, output }) {
  const [open, setOpen] = useState(false);
  const hasOutput = output !== undefined && output !== null && output !== '';
  return (
    <div className="flex flex-col">
      <motion.button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group flex w-full items-center gap-2 rounded-md py-0.5 pr-2 text-left text-[11px]"
        whileHover={{ backgroundColor: 'rgba(127,127,127,0.06)' }}
        transition={{ duration: 0.15 }}
      >
        <motion.span
          animate={{ rotate: open ? 90 : 0 }}
          transition={{ duration: 0.18, ease: [0.2, 0.65, 0.3, 0.9] }}
          className="flex shrink-0 text-muted-foreground"
        >
          <ChevronRight className="size-3" />
        </motion.span>
        <span className="shrink-0 font-mono text-muted-foreground">
          {index + 1}.
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-foreground/90">
          {title}
        </span>
      </motion.button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="entry-panel"
            variants={panelVariants}
            initial="hidden"
            animate="visible"
            exit="exit"
            className="overflow-hidden"
          >
            <div className="ml-4 mt-1 mb-1 space-y-2 border-l border-dashed border-muted-foreground/30 pl-3">
              {input !== undefined && (
                <div>
                  <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                    Input
                  </div>
                  <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-1.5 font-mono text-[11px] text-foreground/90">
                    {formatValue(input)}
                  </pre>
                </div>
              )}
              {hasOutput && (
                <div>
                  <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                    Output
                  </div>
                  <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-1.5 font-mono text-[11px] text-foreground/90">
                    {formatValue(output)}
                  </pre>
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

// Renders a single tool call as a borderless row that visually sits on the
// turn's dashed connecting line. The icon's wrapper has a solid background so
// it punches through the line at its center, mimicking a "node on a wire".
// Click anywhere on the row to expand input + output inline below. When the
// call is a batch (input contains the tool's batch array field, e.g. `reads`
// for read_file), the expanded view renders one sub-row per entry instead of
// the raw input/output blob; each sub-row in turn expands to its own input +
// output segment.
export function ToolCallCard({ name, input, output, isError, defaultOpen = false }) {
  const [open, setOpen] = useState(defaultOpen);
  const hasResult = output !== undefined && output !== null;
  const status = deriveStatus({ hasResult, isError });
  const badgeClass = STATUS_BADGE[status];

  const batchField = BATCH_FIELDS[name];
  // Accept the batch field as either a real array OR a stringified JSON array
  // — mirrors the backend's `coerce_batch_array`. Without this, models that
  // serialize nested arrays as strings (Claude Haiku, GPT-4 under some
  // prompts) would never trigger the per-entry UI even though their batch
  // ran successfully on the server.
  const batchEntries = (() => {
    if (!batchField) return null;
    const raw = input?.[batchField];
    if (Array.isArray(raw)) return raw;
    if (typeof raw === 'string') {
      try {
        const parsed = JSON.parse(raw);
        if (Array.isArray(parsed)) return parsed;
      } catch {
        /* not parseable — fall through to non-batch view */
      }
    }
    return null;
  })();
  const isBatch = !!batchEntries && batchEntries.length > 0;

  const entryOutputs = useMemo(
    () => (isBatch ? splitBatchOutput(output, batchEntries.length) : null),
    [isBatch, output, batchEntries],
  );

  return (
    <div className="flex flex-col">
      <motion.button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group flex w-full items-center gap-2 rounded-md py-1 pr-2 text-left text-xs"
        whileHover={{ backgroundColor: 'rgba(127,127,127,0.06)' }}
        transition={{ duration: 0.15 }}
      >
        <span className="relative z-10 flex shrink-0 bg-sidebar">
          {/* Inner overlay carries the row's hover tint so the icon blends
              with the rest of the row on hover. Outer stays opaque so the
              dashed turn-rail behind it remains hidden. */}
          <span className="flex items-center justify-center px-0.5 group-hover:bg-[rgba(127,127,127,0.06)]">
            <StatusIcon status={status} />
          </span>
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-foreground">
          {name}
          {isBatch && (
            <span className="ml-1.5 text-muted-foreground">
              × {batchEntries.length}
            </span>
          )}
        </span>
        <motion.span
          key={status}
          variants={badgeVariants}
          initial="initial"
          animate="animate"
          className={cn(
            'shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium',
            badgeClass,
          )}
        >
          {STATUS_LABEL[status]}
        </motion.span>
      </motion.button>

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="panel"
            variants={panelVariants}
            initial="hidden"
            animate="visible"
            exit="exit"
            className="overflow-hidden"
          >
            <div className="ml-2 mt-1 mb-1 space-y-2 pl-5 text-xs">
              {/* For spawn_subagent, the live sub-agent chat is more useful
                  than the raw input/output, so we surface clickable rows for
                  each spawned child at the top of the panel. The standard
                  input/output blocks still render below as a fallback. */}
              {name === 'spawn_subagent' && (
                <SpawnedSubagentList output={output} />
              )}
              {isBatch ? (
                <>
                  <div className="space-y-0.5">
                    {batchEntries.map((entry, i) => (
                      <BatchEntryRow
                        key={i}
                        index={i}
                        title={entryTitle(name, entry)}
                        input={entry}
                        output={entryOutputs ? entryOutputs[i] : undefined}
                      />
                    ))}
                  </div>
                  {/* Atomic batches (edit_file, spawn_subagent, code-intel) don't
                      emit per-entry separators, so we couldn't split the output.
                      Show the combined output once below the entry list. */}
                  {hasResult && !entryOutputs && (
                    <div>
                      <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                        {isError ? 'Error' : 'Output'}
                      </div>
                      <pre
                        className={cn(
                          'overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-1.5 font-mono text-[11px]',
                          isError ? 'text-destructive' : 'text-foreground/90',
                        )}
                      >
                        {formatValue(output)}
                      </pre>
                    </div>
                  )}
                </>
              ) : (
                <>
                  {input !== undefined && (
                    <div>
                      <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                        Input
                      </div>
                      <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-1.5 font-mono text-[11px] text-foreground/90">
                        {formatValue(input)}
                      </pre>
                    </div>
                  )}
                  {hasResult && (
                    <div>
                      <div className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                        {isError ? 'Error' : 'Output'}
                      </div>
                      <pre
                        className={cn(
                          'overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-1.5 font-mono text-[11px]',
                          isError ? 'text-destructive' : 'text-foreground/90',
                        )}
                      >
                        {formatValue(output)}
                      </pre>
                    </div>
                  )}
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

export default ToolCallCard;
