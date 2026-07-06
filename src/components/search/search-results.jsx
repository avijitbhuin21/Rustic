import React, { useCallback, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { ChevronDown, ChevronRight, FileText, X, Undo2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useSearch } from '@/state/search';
import { useExplorer } from '@/state/explorer';

function relativeDir(file, projects) {
  const norm = file.replace(/\\/g, '/');
  const idx = norm.lastIndexOf('/');
  let dir = idx === -1 ? '' : norm.slice(0, idx);
  let best = '';
  for (const p of projects) {
    const root = (p.root_path ?? '').replace(/\\/g, '/');
    if (root && (dir === root || dir.startsWith(root + '/')) && root.length > best.length) best = root;
  }
  return best ? dir.slice(best.length).replace(/^\//, '') : dir;
}

export function SearchResults({ onOpenFile }) {
  const results = useSearch((s) => s.results);
  const query = useSearch((s) => s.query);
  const projectCount = useExplorer((s) => s.projects.length);
  const [collapsed, setCollapsed] = useState(() => new Set());
  const scrollRef = useRef(null);

  const toggleOpen = useCallback((file) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(file)) next.delete(file);
      else next.add(file);
      return next;
    });
  }, []);

  // Flat (file header + match) row list — the shape the virtualizer windows
  // over. Collapsed files contribute only their header row.
  const rows = useMemo(() => {
    const out = [];
    for (const [file, matches] of results.entries()) {
      out.push({ type: 'file', key: file, file, matches });
      if (!collapsed.has(file)) {
        for (let i = 0; i < matches.length; i++) {
          out.push({ type: 'match', key: `${file}\n${i}`, file, match: matches[i], ordinal: i });
        }
      }
    }
    return out;
  }, [results, collapsed]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].type === 'file' ? 24 : 22),
    getItemKey: (i) => rows[i].key,
    overscan: 12,
  });

  if (rows.length === 0) {
    if (query.trim()) return null;
    return (
      <div className="px-4 py-6 text-center text-[11px] text-muted-foreground/70">
        {`Search across ${projectCount} project${projectCount === 1 ? '' : 's'} · regex supported`}
      </div>
    );
  }
  return (
    <div ref={scrollRef} className="explorer-scroll h-full overflow-y-auto">
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() + 8 }}>
        {virtualizer.getVirtualItems().map((vi) => {
          const row = rows[vi.index];
          return (
            <div
              key={vi.key}
              className="absolute left-0 w-full"
              style={{ top: vi.start + 4, height: vi.size }}
            >
              {row.type === 'file' ? (
                <FileRow
                  file={row.file}
                  count={row.matches.length}
                  open={!collapsed.has(row.file)}
                  onToggle={toggleOpen}
                />
              ) : (
                <MatchRow
                  file={row.file}
                  match={row.match}
                  ordinal={row.ordinal}
                  onOpenFile={onOpenFile}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Virtualized file-header row: collapse toggle, name/dir, match count, Replace-All exclusion. */
const FileRow = React.memo(function FileRow({ file, count, open, onToggle }) {
  const fileExcluded = useSearch((s) => s.excludedFiles.has(file));
  const toggleFileExcluded = useSearch((s) => s.toggleFileExcluded);
  const projects = useExplorer((s) => s.projects);
  const name = file.replace(/^.*[\\/]/, '');
  const dir = useMemo(() => relativeDir(file, projects), [file, projects]);

  return (
    <div className={cn('group/file flex h-6 w-full items-center gap-1 px-2 hover:bg-muted/40', fileExcluded && 'opacity-40')}>
      <button
        onClick={() => onToggle(file)}
        className="flex min-w-0 flex-1 items-center gap-1 text-xs"
        title={file}
      >
        {open ? <ChevronDown className="size-3 text-muted-foreground" /> : <ChevronRight className="size-3 text-muted-foreground" />}
        <FileText className="size-3 text-muted-foreground" />
        <span className={cn('truncate text-foreground', fileExcluded && 'line-through')}>{name}</span>
        {dir && <span className="truncate text-[10px] text-muted-foreground">{dir}</span>}
      </button>
      <span className="rounded bg-muted px-1 text-[10px] tabular-nums">{count}</span>
      <button
        onClick={(e) => { e.stopPropagation(); toggleFileExcluded(file); }}
        className="flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover/file:opacity-100"
        title={fileExcluded ? 'Include this file in Replace All' : 'Exclude this file from Replace All'}
        aria-label={fileExcluded ? 'Include file' : 'Exclude file'}
      >
        {fileExcluded ? <Undo2 className="size-3" /> : <X className="size-3" />}
      </button>
    </div>
  );
});

/** Virtualized match row: subscribes to its own exclusion state so toggles re-render only affected rows. */
const MatchRow = React.memo(function MatchRow({ file, match, ordinal, onOpenFile }) {
  const excluded = useSearch(
    (s) => s.excludedFiles.has(file) || !!s.excludedMatches.get(file)?.has(ordinal),
  );
  const toggleMatchExcluded = useSearch((s) => s.toggleMatchExcluded);
  const replaceText = useSearch((s) => s.replace);
  const query = useSearch((s) => s.query);
  const regex = useSearch((s) => s.regex);
  const caseSensitive = useSearch((s) => s.caseSensitive);
  const before = match.line_text?.slice(0, match.start) ?? '';
  const hit = match.line_text?.slice(match.start, match.end) ?? '';
  const after = match.line_text?.slice(match.end) ?? '';
  // Best-effort before→after preview; the JS regex engine may diverge from the
  // backend's on exotic patterns, so any failure just hides the preview.
  const preview = useMemo(() => {
    if (excluded || !replaceText) return null;
    if (!regex) return replaceText;
    try {
      return hit.replace(new RegExp(query, caseSensitive ? '' : 'i'), replaceText);
    } catch {
      return null;
    }
  }, [excluded, replaceText, regex, query, caseSensitive, hit]);
  return (
    <div className="group/match flex h-[22px] w-full items-center gap-2 px-2 pl-8 text-[11px] hover:bg-muted/40">
      <button
        onClick={() => onOpenFile?.(file, { line: match.line, matchStart: match.start, matchEnd: match.end })}
        className="min-w-0 flex-1 text-left text-muted-foreground"
      >
        <span data-search-match className={cn('truncate font-mono', excluded && 'line-through opacity-50')}>
          {before}
          <mark className={cn('search-match-highlight px-0.5 text-foreground', preview != null && 'line-through opacity-60')}>{hit}</mark>
          {preview != null && <span className="rounded bg-green-500/20 px-0.5 text-foreground">{preview}</span>}
          {after}
        </span>
      </button>
      <button
        onClick={(e) => { e.stopPropagation(); toggleMatchExcluded(file, ordinal); }}
        className="flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover/match:opacity-100"
        title={excluded ? 'Include this match in Replace All' : 'Exclude this match from Replace All'}
        aria-label={excluded ? 'Include match' : 'Exclude match'}
      >
        {excluded ? <Undo2 className="size-3" /> : <X className="size-3" />}
      </button>
    </div>
  );
});
