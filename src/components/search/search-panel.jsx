import React, { useEffect, useRef, useState } from 'react';
import { X, ChevronDown, ChevronRight, Regex, CaseSensitive, WholeWord, Check } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Toggle } from '@/components/ui/toggle';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
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

  const projects      = useExplorer((s) => s.projects);

  const [showReplace, setShowReplace] = useState(false);

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
  }, [query, regex, caseSensitive, wholeWord, scopeIds, projects.length, start, cancel]);

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
                placeholder="Search"
                className="h-7 pr-20 text-xs"
                autoFocus
              />
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
              <Input
                value={replace}
                onChange={(e) => setField('replace', e.target.value)}
                placeholder="Replace"
                className="h-7 text-xs"
              />
            )}
          </div>
        </div>

        {(totalMatches > 0 || (!running && query)) && (
          <div className="pl-6 text-[11px] text-muted-foreground">
            {totalMatches > 0
              ? `${totalMatches} matches in ${filesMatched} files`
              : 'No matches'}
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto explorer-scroll">
        <SearchResults onOpenFile={onOpenFile} />
      </div>
    </div>
  );
}
