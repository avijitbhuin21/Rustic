import React, { useState } from 'react';
import { ChevronDown, ChevronRight, FileText, X, Undo2 } from 'lucide-react';
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
  const excludedFiles = useSearch((s) => s.excludedFiles);
  const excludedMatches = useSearch((s) => s.excludedMatches);
  const toggleFileExcluded = useSearch((s) => s.toggleFileExcluded);

  const fileExcluded = excludedFiles.has(file);
  const matchSet = excludedMatches.get(file);
  const name = file.replace(/^.*[\\/]/, '');

  return (
    <div className={cn(fileExcluded && 'opacity-40')}>
      <div className="group/file flex h-6 w-full items-center gap-1 px-2 hover:bg-muted/40">
        <button
          onClick={() => setOpen((o) => !o)}
          className="flex min-w-0 flex-1 items-center gap-1 text-xs"
          title={file}
        >
          {open ? <ChevronDown className="size-3 text-muted-foreground" /> : <ChevronRight className="size-3 text-muted-foreground" />}
          <FileText className="size-3 text-muted-foreground" />
          <span className={cn('truncate text-foreground', fileExcluded && 'line-through')}>{name}</span>
        </button>
        <span className="rounded bg-muted px-1 text-[10px] tabular-nums">{matches.length}</span>
        <button
          onClick={(e) => { e.stopPropagation(); toggleFileExcluded(file); }}
          className="flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground group-hover/file:opacity-100"
          title={fileExcluded ? 'Include this file in Replace All' : 'Exclude this file from Replace All'}
          aria-label={fileExcluded ? 'Include file' : 'Exclude file'}
        >
          {fileExcluded ? <Undo2 className="size-3" /> : <X className="size-3" />}
        </button>
      </div>
      {open && matches.map((m, i) => (
        <MatchRow
          key={i}
          file={file}
          match={m}
          ordinal={i}
          excluded={fileExcluded || !!matchSet?.has(i)}
          onOpenFile={onOpenFile}
        />
      ))}
    </div>
  );
}

function MatchRow({ file, match, ordinal, excluded, onOpenFile }) {
  const toggleMatchExcluded = useSearch((s) => s.toggleMatchExcluded);
  const before = match.line_text?.slice(0, match.start) ?? '';
  const hit = match.line_text?.slice(match.start, match.end) ?? '';
  const after = match.line_text?.slice(match.end) ?? '';
  return (
    <div className="group/match flex h-5 w-full items-center gap-2 px-2 pl-8 text-[11px] hover:bg-muted/40">
      <button
        onClick={() => onOpenFile?.(file, { line: match.line, matchStart: match.start, matchEnd: match.end })}
        className="min-w-0 flex-1 text-left text-muted-foreground"
      >
        <span data-search-match className={cn('truncate font-mono', excluded && 'line-through opacity-50')}>
          {before}
          <mark className="rounded bg-yellow-500/30 px-0.5 text-foreground">{hit}</mark>
          {after}
        </span>
      </button>
      <button
        onClick={(e) => { e.stopPropagation(); toggleMatchExcluded(file, ordinal); }}
        className="flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground group-hover/match:opacity-100"
        title={excluded ? 'Include this match in Replace All' : 'Exclude this match from Replace All'}
        aria-label={excluded ? 'Include match' : 'Exclude match'}
      >
        {excluded ? <Undo2 className="size-3" /> : <X className="size-3" />}
      </button>
    </div>
  );
}
