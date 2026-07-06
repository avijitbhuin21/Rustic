import React, { useEffect, useMemo, useRef, useState } from 'react';
import { X, ChevronDown, ChevronRight, Regex, CaseSensitive, WholeWord, Check, ReplaceAll, Loader2, History } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Toggle } from '@/components/ui/toggle';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { confirm } from '@/components/confirm-dialog';
import { useSearch } from '@/state/search';
import { useExplorer } from '@/state/explorer';
import { AddProjectButton } from '@/components/shell/add-project-button';
import { SearchResults } from './search-results';

export function SearchPanel({ onOpenFile }) {
  const query        = useSearch((s) => s.query);
  const replace      = useSearch((s) => s.replace);
  const regex        = useSearch((s) => s.regex);
  const caseSensitive = useSearch((s) => s.caseSensitive);
  const wholeWord    = useSearch((s) => s.wholeWord);
  const running      = useSearch((s) => s.running);
  const totalMatches = useSearch((s) => s.totalMatches);
  const filesMatched = useSearch((s) => s.filesMatched);
  const setField     = useSearch((s) => s.setField);
  const start        = useSearch((s) => s.start);
  const cancel       = useSearch((s) => s.cancel);
  const scopeIds     = useSearch((s) => s.scopeIds);
  const setScopeIds  = useSearch((s) => s.setScopeIds);
  const results        = useSearch((s) => s.results);
  const excludedFiles  = useSearch((s) => s.excludedFiles);
  const excludedMatches = useSearch((s) => s.excludedMatches);
  const replaceAll     = useSearch((s) => s.replaceAll);
  const includeGlobs   = useSearch((s) => s.includeGlobs);
  const excludeGlobs   = useSearch((s) => s.excludeGlobs);
  const history        = useSearch((s) => s.history);

  const projects      = useExplorer((s) => s.projects);

  const [showReplace, setShowReplace] = useState(false);
  const [showFilters, setShowFilters] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [replacing, setReplacing] = useState(false);

  // How many files/matches Replace All would touch (results minus excluded
  // files and matches). Drives the button state + the confirm copy.
  const affected = useMemo(() => {
    let files = 0;
    let matches = 0;
    for (const [file, ms] of results.entries()) {
      if (excludedFiles.has(file)) continue;
      const exCount = excludedMatches.get(file)?.size ?? 0;
      if (exCount >= ms.length) continue;
      files += 1;
      matches += ms.length - exCount;
    }
    return { files, matches };
  }, [results, excludedFiles, excludedMatches]);

  const onReplaceAll = async () => {
    if (replacing) return;
    const ok = await confirm({
      title: 'Replace All',
      description: `Replace ${affected.matches} match${affected.matches === 1 ? '' : 'es'} in ${affected.files} file${affected.files === 1 ? '' : 's'}? This cannot be undone.`,
      confirmLabel: 'Replace All',
      destructive: true,
    });
    if (!ok) return;
    setReplacing(true);
    try {
      await replaceAll();
    } catch (err) {
      console.error('replaceAll failed:', err);
    } finally {
      setReplacing(false);
    }
  };

  // Default to first project; clean up removed projects from selection.
  useEffect(() => {
    if (projects.length === 0) return;
    const valid = scopeIds.filter((id) => projects.some((p) => p.id === id));
    if (valid.length === 0) {
      setScopeIds([projects[0].id]);
    } else if (valid.length < scopeIds.length) {
      setScopeIds(valid);
    }
  }, [projects, scopeIds, setScopeIds]);

  const toggleProject = (id) => {
    if (scopeIds.includes(id)) {
      if (scopeIds.length > 1) setScopeIds(scopeIds.filter((s) => s !== id));
    } else {
      setScopeIds([...scopeIds, id]);
    }
  };

  const scopeLabel =
    scopeIds.length === 0   ? 'No project'
    : scopeIds.length === 1 ? (projects.find((p) => p.id === scopeIds[0])?.name ?? '1 project')
    : `${scopeIds.length} projects`;

  // Debounced auto-search.
  const debounceRef = useRef(null);
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (!query.trim() || projects.length === 0 || scopeIds.length === 0) {
      cancel();
      return;
    }
    debounceRef.current = setTimeout(() => {
      start();
    }, 200);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [query, regex, caseSensitive, wholeWord, includeGlobs, excludeGlobs, scopeIds, projects.length, start, cancel]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border/60 px-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Search
        </span>
        <AddProjectButton />

        {/* Multi-project selector */}
        <Popover>
          <PopoverTrigger asChild>
            <button className="ml-auto flex h-5 max-w-[120px] items-center gap-1 rounded border border-border/60 px-1.5 text-[10px] text-muted-foreground hover:bg-muted/50 hover:text-foreground">
              <span className="truncate">{scopeLabel}</span>
              <ChevronDown className="size-2.5 shrink-0" />
            </button>
          </PopoverTrigger>
          <PopoverContent className="w-44 p-1" align="end" sideOffset={4}>
            {projects.length === 0 ? (
              <p className="px-2 py-1 text-[11px] text-muted-foreground">No projects open</p>
            ) : (
              projects.map((p) => {
                const selected = scopeIds.includes(p.id);
                return (
                  <button
                    key={p.id}
                    onClick={() => toggleProject(p.id)}
                    className="flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs hover:bg-muted/50"
                  >
                    <span className="flex size-3.5 shrink-0 items-center justify-center rounded-sm border border-border/60">
                      {selected && <Check className="size-2.5 text-foreground" />}
                    </span>
                    <span className="truncate text-foreground">{p.name}</span>
                  </button>
                );
              })
            )}
          </PopoverContent>
        </Popover>

        {running && (
          <Button variant="ghost" size="icon-xs" onClick={cancel}>
            <X className="size-3" />
          </Button>
        )}
      </div>

      <div className="flex flex-col gap-1.5 border-b border-border/60 p-2">
        <div className="flex items-start gap-1">
          <button
            type="button"
            onClick={() => setShowReplace((v) => !v)}
            aria-label={showReplace ? 'Hide replace' : 'Show replace'}
            className="mt-1 flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-muted/50 hover:text-foreground"
          >
            {showReplace ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
          </button>

          <div className="flex flex-1 flex-col gap-1.5">
            <div className="relative flex items-center">
              <Input
                value={query}
                onChange={(e) => setField('query', e.target.value)}
                onFocus={() => setHistoryOpen(true)}
                onBlur={() => setHistoryOpen(false)}
                placeholder="Search"
                className="h-7 pr-20 text-xs"
                autoFocus
              />
              {historyOpen && !query && history.length > 0 && (
                <div className="absolute inset-x-0 top-full z-10 mt-1 rounded-md border border-border/60 bg-popover py-1 shadow-md">
                  {history.map((h) => (
                    <button
                      key={h}
                      onMouseDown={(e) => { e.preventDefault(); setField('query', h); }}
                      className="flex w-full items-center gap-1.5 px-2 py-1 text-left text-[11px] text-muted-foreground hover:bg-muted/50 hover:text-foreground"
                    >
                      <History className="size-3 shrink-0" />
                      <span className="truncate font-mono">{h}</span>
                    </button>
                  ))}
                </div>
              )}
              <div className="absolute right-1 top-1/2 flex -translate-y-1/2 gap-0.5">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Toggle
                      pressed={caseSensitive}
                      onPressedChange={(v) => setField('caseSensitive', v)}
                      size="sm"
                      className="size-5 p-0"
                      aria-label="Match Case"
                    >
                      <CaseSensitive className="size-3" />
                    </Toggle>
                  </TooltipTrigger>
                  <TooltipContent>Match Case</TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Toggle
                      pressed={wholeWord}
                      onPressedChange={(v) => setField('wholeWord', v)}
                      size="sm"
                      className="size-5 p-0"
                      aria-label="Whole Word"
                    >
                      <WholeWord className="size-3" />
                    </Toggle>
                  </TooltipTrigger>
                  <TooltipContent>Whole Word</TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Toggle
                      pressed={regex}
                      onPressedChange={(v) => setField('regex', v)}
                      size="sm"
                      className="size-5 p-0"
                      aria-label="Regex"
                    >
                      <Regex className="size-3" />
                    </Toggle>
                  </TooltipTrigger>
                  <TooltipContent>Regex</TooltipContent>
                </Tooltip>
              </div>
            </div>

            {showReplace && (
              <div className="flex items-center gap-1">
                <Input
                  value={replace}
                  onChange={(e) => setField('replace', e.target.value)}
                  placeholder="Replace"
                  className="h-7 flex-1 text-xs"
                />
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      size="sm"
                      variant="secondary"
                      className="size-7 shrink-0 p-0"
                      disabled={replacing || running || affected.files === 0 || !query.trim()}
                      onClick={onReplaceAll}
                      aria-label="Replace all matches across every non-excluded file"
                    >
                      <ReplaceAll className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>
                    {`Replace All${affected.files > 0 ? ` (${affected.files})` : ''}`}
                  </TooltipContent>
                </Tooltip>
              </div>
            )}

            <button
              type="button"
              onClick={() => setShowFilters((v) => !v)}
              className="flex items-center gap-1 self-start text-[10px] text-muted-foreground hover:text-foreground"
            >
              {showFilters ? <ChevronDown className="size-2.5" /> : <ChevronRight className="size-2.5" />}
              <span>Filters{!showFilters && (includeGlobs.trim() || excludeGlobs.trim()) ? ' · active' : ''}</span>
            </button>
            {showFilters && (
              <div className="flex flex-col gap-1">
                <Input
                  value={includeGlobs}
                  onChange={(e) => setField('includeGlobs', e.target.value)}
                  placeholder="files to include (e.g. src/**/*.js)"
                  className="h-6 text-[11px]"
                />
                <Input
                  value={excludeGlobs}
                  onChange={(e) => setField('excludeGlobs', e.target.value)}
                  placeholder="files to exclude"
                  className="h-6 text-[11px]"
                />
              </div>
            )}
          </div>
        </div>

        {(running || totalMatches > 0 || query) && (
          <div className="flex items-center gap-1.5 pl-6 text-[11px] text-muted-foreground">
            {running && <Loader2 className="size-3 shrink-0 animate-spin" />}
            <span>
              {running
                ? `Searching…${totalMatches > 0 ? ` ${totalMatches} matches in ${filesMatched} files` : ''}`
                : totalMatches > 0
                  ? `${totalMatches} matches in ${filesMatched} files`
                  : 'No matches'}
            </span>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1">
        <SearchResults onOpenFile={onOpenFile} />
      </div>
    </div>
  );
}
