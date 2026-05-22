import React, { useState } from 'react';
import { ChevronDown, ChevronRight, FileText } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useSearch } from '@/state/search';

export function SearchResults({ onOpenFile }) {
  const results = useSearch((s) => s.results);
  const entries = Array.from(results.entries());
  if (entries.length === 0) return null;
  return (
    <div className="flex flex-col py-1">
      {entries.map(([file, matches]) => (
        <FileGroup key={file} file={file} matches={matches} onOpenFile={onOpenFile} />
      ))}
    </div>
  );
}

function FileGroup({ file, matches, onOpenFile }) {
  const [open, setOpen] = useState(true);
  const name = file.replace(/^.*[\\/]/, '');
  const dir = file.slice(0, file.length - name.length).replace(/[\\/]+$/, '');
  return (
    <div>
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex h-6 w-full items-center gap-1 px-2 text-xs hover:bg-muted/40"
        title={file}
      >
        {open ? <ChevronDown className="size-3 text-muted-foreground" /> : <ChevronRight className="size-3 text-muted-foreground" />}
        <FileText className="size-3 text-muted-foreground" />
        <span className="truncate text-foreground">{name}</span>
        <span className="ml-auto rounded bg-muted px-1 text-[10px] tabular-nums">{matches.length}</span>
      </button>
      {open && matches.map((m, i) => (
        <MatchRow key={i} file={file} match={m} onOpenFile={onOpenFile} />
      ))}
    </div>
  );
}

function MatchRow({ file, match, onOpenFile }) {
  const before = match.line_text?.slice(0, match.start) ?? '';
  const hit = match.line_text?.slice(match.start, match.end) ?? '';
  const after = match.line_text?.slice(match.end) ?? '';
  return (
    <button
      onClick={() => onOpenFile?.(file, { line: match.line, matchStart: match.start, matchEnd: match.end })}
      className={cn(
        'flex h-5 w-full items-center gap-2 px-2 pl-8 text-[11px] hover:bg-muted/40',
        'text-muted-foreground'
      )}
    >
      <span data-search-match className="truncate text-left font-mono">
        {before}
        <mark className="rounded bg-yellow-500/30 px-0.5 text-foreground">{hit}</mark>
        {after}
      </span>
    </button>
  );
}
