// The mock Rustic window shown by the theme preview. Controls are Rustic's real
// components; the panels copy the markup and classes of the real chrome
// (file-node, tab-bar, breadcrumb, bottom-panel-host, status-bar), which can't
// be mounted here because they need the app's stores and backend.
import React from 'react';
import {
  ChevronRight, Folder, FolderOpen, FilePlus, FolderPlus, RefreshCw, X, Plus, Maximize2, GitBranch, Columns2,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { terminalTheme } from '@/lib/terminal-theme';
import FILE_ICONS from './file-icons.json';
import { MOCK_TREE, MOCK_TABS, MOCK_CODE, MOCK_ACTIVE_LINE, MOCK_TERMINAL } from './mock-data';
import { MockChat, MockDialog } from './mock-chat';

const EDITOR_FONT = "'Victor Mono', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace";
const GIT_TINT = { M: 'text-warning', '?': 'text-success' };
const TOKEN_STYLE = {
  k: { color: 'var(--syntax-keyword)' },
  s: { color: 'var(--syntax-string)' },
  t: { color: 'var(--syntax-type)' },
};
const TOKEN_CLASS = { c: 'italic text-muted-foreground', n: 'text-[#098658] dark:text-[#b5cea8]' };

function FileIcon({ name }) {
  return (
    <span
      aria-hidden
      className="inline-flex size-3.5 shrink-0 items-center justify-center"
      dangerouslySetInnerHTML={{ __html: FILE_ICONS[name] || '' }}
    />
  );
}

function Explorer({ showTooltip }) {
  return (
    <div data-ink-frame="panel" className="h-full w-full overflow-hidden bg-sidebar">
      <div className="flex h-full flex-col">
        <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
          <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Explorer</span>
          <div className="flex items-center gap-1">
            <Tooltip open={showTooltip}>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon-xs"><FilePlus className="size-3.5" /></Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">New file</TooltipContent>
            </Tooltip>
            <Button variant="ghost" size="icon-xs"><FolderPlus className="size-3.5" /></Button>
            <Button variant="ghost" size="icon-xs"><RefreshCw className="size-3.5" /></Button>
          </div>
        </div>
        <div className="flex h-7 items-center gap-1 border-b border-border/60 bg-muted/60 px-2 text-[11px] font-semibold uppercase tracking-wide">
          <ChevronRight className="size-3 rotate-90 text-muted-foreground" />
          moonlight
        </div>
        <div className="py-1">
          {MOCK_TREE.map(([depth, name, kind, git, selected]) => (
            <div
              key={`${depth}-${name}`}
              style={{ paddingLeft: 4 + depth * 12 }}
              className={cn(
                'flex h-6 cursor-pointer items-center gap-1 px-1 text-xs hover:bg-muted/50',
                selected ? 'bg-muted text-foreground' : 'text-foreground/80',
              )}
            >
              <span className="flex w-4 items-center justify-center">
                {kind !== 'file' && (
                  <ChevronRight className={cn('size-3 text-muted-foreground', kind === 'open' && 'rotate-90')} />
                )}
              </span>
              {kind === 'file' ? <FileIcon name={name} />
                : kind === 'open' ? <FolderOpen className="size-3.5 shrink-0 text-primary/70" />
                : <Folder className="size-3.5 shrink-0 text-primary/70" />}
              <span className={cn('truncate', GIT_TINT[git])}>{name}</span>
              {git && (
                <span className={cn('ml-auto w-4 shrink-0 pr-0.5 text-center font-mono text-[10px] font-semibold', GIT_TINT[git])}>
                  {git === '?' ? 'U' : git}
                </span>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function Editor() {
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div data-ink-frame="rule-bottom" className="flex h-8 shrink-0 items-stretch border-b border-border bg-muted/20">
        <div className="flex flex-1 items-center overflow-hidden">
          {MOCK_TABS.map((name, i) => (
            <div
              key={name}
              className={cn(
                'relative flex h-8 shrink-0 cursor-pointer items-center gap-1.5 border-r border-border px-3 text-xs select-none',
                i === 0 ? 'bg-background text-foreground' : 'bg-muted/40 text-muted-foreground hover:bg-muted/60 hover:text-foreground',
              )}
            >
              {i === 0 && <span className="absolute left-0 right-0 top-0 h-px bg-primary" />}
              <FileIcon name={name} />
              <span className="max-w-[200px] truncate">{name}</span>
              <X className="ml-1 size-3 opacity-60" />
            </div>
          ))}
        </div>
        <div className="flex items-center px-1.5">
          <Button variant="ghost" size="icon-xs"><Columns2 className="size-3.5" /></Button>
        </div>
      </div>
      <div className="flex h-[22px] shrink-0 items-center overflow-hidden border-b border-border/40 bg-background px-3 text-[11px] text-muted-foreground">
        moonlight<span className="mx-1 select-none text-[10px] text-muted-foreground/60">›</span>
        web<span className="mx-1 select-none text-[10px] text-muted-foreground/60">›</span>
        <span className="text-foreground">app.tsx</span>
      </div>
      <div className="min-h-0 flex-1 overflow-hidden py-1 text-[13px] leading-[19px]" style={{ fontFamily: EDITOR_FONT }}>
        {MOCK_CODE.map((line, i) => {
          const active = i + 1 === MOCK_ACTIVE_LINE;
          return (
            <div key={i} className={cn('flex whitespace-pre', active && 'bg-foreground/[0.08]')}>
              <span className={cn('w-12 shrink-0 pr-4 text-right select-none', active ? 'text-foreground' : 'text-muted-foreground')}>
                {i + 1}
              </span>
              <span className="text-foreground">
                {line.map(([kind, text], j) => (
                  <span key={j} style={TOKEN_STYLE[kind]} className={TOKEN_CLASS[kind]}>{text}</span>
                ))}
                {active && <span className="ml-px inline-block h-[15px] w-[2px] translate-y-[3px] bg-foreground" />}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function TerminalPanel({ paints }) {
  // Re-read after every repaint: these colors come from the theme's variables.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const colors = React.useMemo(() => terminalTheme(), [paints]);
  return (
    <div data-ink-frame="panel" className="flex h-full w-full flex-col bg-background">
      <div className="flex h-7 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <div className="flex items-center gap-0.5 text-[11px]">
          <span className="rounded-md bg-muted px-2 py-0.5 text-foreground">User terminals</span>
          <span className="rounded-md px-2 py-0.5 text-muted-foreground">Agent terminals</span>
        </div>
        <div className="flex items-center gap-1">
          <Button variant="ghost" size="icon-xs"><Plus className="size-3.5" /></Button>
          <Button variant="ghost" size="icon-xs"><Maximize2 className="size-3.5" /></Button>
          <Button variant="ghost" size="icon-xs"><X className="size-3.5" /></Button>
        </div>
      </div>
      <div
        className="flex-1 overflow-hidden px-3 py-2 text-[13px] leading-[17px] whitespace-pre"
        style={{ background: colors.background, color: colors.foreground, fontFamily: EDITOR_FONT }}
      >
        {MOCK_TERMINAL.map((line, i) => (
          <div key={i}>
            {line.map(([key, text, bold], j) => (
              <span key={j} style={{ color: colors[key], fontWeight: bold ? 700 : 400 }}>{text}</span>
            ))}
            {i === MOCK_TERMINAL.length - 1 && (
              <span className="inline-block h-[15px] w-[8px] translate-y-[3px]" style={{ background: colors.cursor }} />
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function StatusBar() {
  return (
    <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-background px-2 text-[11px] text-muted-foreground select-none">
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1"><GitBranch className="size-3" /> main</span>
        <span className="text-warning">1 warning</span>
        <span className="text-danger">1 error</span>
      </div>
      <div className="flex items-center gap-3">
        <span>Ln {MOCK_ACTIVE_LINE}, Col 58</span>
        <span>TypeScript JSX</span>
        <span>Rustic</span>
      </div>
    </div>
  );
}

/** The whole mock window. `scene` is "workbench", "dialog" or "menu"; `paints` counts theme repaints. */
export function MockWindow({ scene, paints }) {
  return (
    <TooltipProvider>
      <div className="flex h-full w-full flex-col bg-background text-foreground">
        <div className="flex min-h-0 flex-1">
          <div className="w-[232px] shrink-0 border-r border-border">
            <Explorer showTooltip={scene === 'menu'} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <Editor />
            <div className="h-[212px] shrink-0 border-t border-border">
              <TerminalPanel paints={paints} />
            </div>
          </div>
          <div className="w-[372px] shrink-0 border-l border-border">
            <MockChat menuOpen={scene === 'menu'} />
          </div>
        </div>
        <StatusBar />
      </div>
      <MockDialog open={scene === 'dialog'} />
    </TooltipProvider>
  );
}