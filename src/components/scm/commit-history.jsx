import React, { useState } from 'react';
import { ChevronRight, ChevronDown, Loader2 } from 'lucide-react';
import { useGit, EMPTY_ARRAY } from '@/state/git';
import { cn } from '@/lib/utils';

function formatRelativeDate(input) {
  if (!input) return '';
  const ts =
    typeof input === 'number'
      ? input < 1e12
        ? input * 1000
        : input
      : Date.parse(input);
  if (!Number.isFinite(ts)) return '';
  const diff = (Date.now() - ts) / 1000;
  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  return new Date(ts).toLocaleDateString();
}

function shortHash(h) {
  return (h ?? '').toString().slice(0, 7);
}

// Branch label badge colors (cycles through a small palette)
const REF_COLORS = [
  'bg-blue-500/20 text-blue-400 border-blue-500/30',
  'bg-violet-500/20 text-violet-400 border-violet-500/30',
  'bg-emerald-500/20 text-emerald-400 border-emerald-500/30',
  'bg-amber-500/20 text-amber-400 border-amber-500/30',
];

function refColor(name, index) {
  if (name === 'HEAD' || name.includes('HEAD')) return 'bg-rose-500/20 text-rose-400 border-rose-500/30';
  if (name.startsWith('origin/')) return 'bg-orange-500/20 text-orange-400 border-orange-500/30';
  return REF_COLORS[index % REF_COLORS.length];
}

function CommitRow({ commit, projectId, onSelect, isLast }) {
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [files, setFiles] = useState(null);
  const loadCommitFiles = useGit((s) => s.loadCommitFiles);

  const oid = commit.oid ?? commit.hash ?? commit.id;
  const message = commit.message ?? commit.summary ?? commit.subject ?? '(no message)';
  const author = commit.author_name ?? commit.author ?? '';
  const when = commit.timestamp ?? commit.time ?? commit.date ?? commit.author_date;
  const isMerge = (commit.parent_count ?? 0) > 1;
  const refs = commit.refs ?? [];

  async function toggle() {
    if (expanded) {
      setExpanded(false);
      return;
    }
    setExpanded(true);
    if (files === null && oid) {
      setLoading(true);
      try {
        const result = await loadCommitFiles(oid, projectId);
        setFiles(result ?? []);
      } finally {
        setLoading(false);
      }
    }
  }

  return (
    <div className="flex min-w-0 overflow-hidden">
      {/* Graph column */}
      <div className="relative flex w-6 shrink-0 flex-col items-center">
        {/* Vertical connector line */}
        {!isLast && (
          <div className="absolute top-4 bottom-0 left-1/2 w-px -translate-x-1/2 bg-border/60" />
        )}
        {/* Commit dot */}
        <div
          className={cn(
            'relative mt-[9px] size-2.5 shrink-0 rounded-full border-2 bg-background',
            isMerge
              ? 'border-violet-400'
              : 'border-muted-foreground/50'
          )}
        />
      </div>

      {/* Content */}
      <div className="min-w-0 flex-1 pb-0.5">
        <button
          type="button"
          onClick={toggle}
          className="flex w-full items-start gap-1 px-1 py-1 text-left text-xs hover:bg-muted/60 rounded"
        >
          {expanded ? (
            <ChevronDown className="mt-0.5 size-3 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="mt-0.5 size-3 shrink-0 text-muted-foreground" />
          )}
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="truncate text-foreground leading-tight">{message}</span>
            {refs.length > 0 && (
              <div className="flex flex-wrap gap-0.5">
                {refs.map((ref, i) => (
                  <span
                    key={ref}
                    className={cn(
                      'rounded border px-1 py-px text-[9px] font-medium leading-tight',
                      refColor(ref, i)
                    )}
                  >
                    {ref}
                  </span>
                ))}
              </div>
            )}
            <span className="truncate text-[10px] text-muted-foreground">
              {shortHash(oid)} · {author} · {formatRelativeDate(when)}
            </span>
          </div>
        </button>

        {expanded && (
          <div className="pl-4 pb-1">
            {loading && (
              <div className="flex items-center gap-1.5 py-1 text-[10px] text-muted-foreground">
                <Loader2 className="size-3 animate-spin" />
                Loading…
              </div>
            )}
            {!loading && files?.length === 0 && (
              <div className="py-1 text-[10px] text-muted-foreground">
                No file changes.
              </div>
            )}
            {!loading &&
              files?.map((f, i) => {
                const path = f.path ?? f.file ?? '';
                const status = (f.status ?? 'M').toString().charAt(0).toUpperCase();
                const statusColors = {
                  A: 'text-emerald-500',
                  D: 'text-red-500',
                  R: 'text-blue-500',
                  M: 'text-yellow-500',
                };
                return (
                  <button
                    key={`${path}-${i}`}
                    type="button"
                    onClick={() => onSelect?.({ ...f, commitOid: oid })}
                    className="flex w-full items-center gap-1.5 rounded px-1 py-0.5 text-left text-[11px] hover:bg-muted/60"
                    title={path}
                  >
                    <span
                      className={cn(
                        'w-3 shrink-0 text-center font-mono text-[10px] font-semibold',
                        statusColors[status] ?? 'text-muted-foreground'
                      )}
                    >
                      {status}
                    </span>
                    <span className="truncate">{path}</span>
                  </button>
                );
              })}
          </div>
        )}
      </div>
    </div>
  );
}

export default function CommitHistory({ projectId, onSelectFile }) {
  const log = useGit((s) => s.projects[projectId]?.log ?? EMPTY_ARRAY);

  if (log.length === 0) {
    return (
      <div className="px-3 py-2 text-xs text-muted-foreground">
        No commits yet.
      </div>
    );
  }

  return (
    <div className="flex w-full min-w-0 flex-col overflow-hidden px-1">
      {log.map((c, i) => (
        <CommitRow
          key={(c.oid ?? c.hash ?? c.id ?? '') + i}
          commit={c}
          projectId={projectId}
          onSelect={onSelectFile}
          isLast={i === log.length - 1}
        />
      ))}
    </div>
  );
}
