import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Folder, Check } from 'lucide-react';
import { getIcon } from 'material-file-icons';
import {
  DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem,
} from '@/components/ui/dropdown-menu';
import { readDir, useExplorer } from '@/state/explorer';
import { useEditor } from '@/state/editor';
import { cn } from '@/lib/utils';

const MAX_MENU_ENTRIES = 300;

function ancestorPath(path, up) {
  let p = path;
  for (let i = 0; i < up; i++) p = p.replace(/[\\/][^\\/]+$/, '');
  return p;
}

const normPath = (p) => (p ?? '').replace(/\\/g, '/');

function FileIcon({ name }) {
  // Memoized so React 19 doesn't re-set innerHTML on every render.
  const html = useMemo(() => ({ __html: getIcon(name).svg }), [name]);
  return (
    <span
      aria-hidden
      className="inline-flex size-3.5 shrink-0 items-center justify-center"
      dangerouslySetInnerHTML={html}
    />
  );
}

function FolderSegment({ name, path, childPath }) {
  const [entries, setEntries] = useState(null);
  const [failed, setFailed] = useState(false);
  const openFile = useEditor((s) => s.openFile);

  const load = (dir) => {
    setEntries(null);
    setFailed(false);
    readDir(dir)
      .then((list) =>
        setEntries(
          [...list].sort((a, b) => (b.is_dir - a.is_dir) || a.name.localeCompare(b.name)),
        ),
      )
      .catch(() => setFailed(true));
  };

  return (
    <DropdownMenu onOpenChange={(open) => { if (open) load(path); }}>
      <DropdownMenuTrigger asChild>
        <button
          className="truncate rounded-sm px-0.5 text-[11px] text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-accent/50 data-[state=open]:text-foreground"
        >
          {name}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-72 w-60 overflow-y-auto">
        {failed && (
          <div className="px-2 py-1.5 text-[11px] text-muted-foreground">Could not read folder</div>
        )}
        {!failed && entries === null && (
          <div className="px-2 py-1.5 text-[11px] text-muted-foreground">Loading…</div>
        )}
        {!failed && entries?.length === 0 && (
          <div className="px-2 py-1.5 text-[11px] text-muted-foreground">Empty folder</div>
        )}
        {(entries ?? []).slice(0, MAX_MENU_ENTRIES).map((entry) => (
          <DropdownMenuItem
            key={entry.path}
            onSelect={(e) => {
              if (entry.is_dir) {
                // Drill into the subfolder in place instead of closing.
                e.preventDefault();
                load(entry.path);
              } else {
                openFile(entry.path);
              }
            }}
            className={cn(
              'gap-1.5 text-xs',
              normPath(entry.path) === normPath(childPath) && 'bg-accent/40',
            )}
          >
            {entry.is_dir ? (
              <Folder className="size-3.5 shrink-0 text-primary/70" />
            ) : (
              <FileIcon name={entry.name} />
            )}
            <span className="truncate">{entry.name}</span>
          </DropdownMenuItem>
        ))}
        {(entries?.length ?? 0) > MAX_MENU_ENTRIES && (
          <div className="px-2 py-1 text-[10px] text-muted-foreground">
            +{entries.length - MAX_MENU_ENTRIES} more
          </div>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function FileSegment({ name, fullPath }) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef(null);
  useEffect(() => () => clearTimeout(timerRef.current), []);
  return (
    <button
      onClick={() => {
        navigator.clipboard
          .writeText(fullPath)
          .then(() => {
            setCopied(true);
            clearTimeout(timerRef.current);
            timerRef.current = setTimeout(() => setCopied(false), 1200);
          })
          .catch(() => {});
      }}
      title={copied ? 'Copied' : 'Click to copy path'}
      className="flex min-w-0 items-center gap-1 rounded-sm px-0.5 text-[11px] text-foreground/85 transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    >
      <span className="truncate">{name}</span>
      {copied && <Check className="size-3 shrink-0 text-emerald-500" />}
    </button>
  );
}

export function Breadcrumb({ tab }) {
  const projects = useExplorer((s) => s.projects);

  const segments = useMemo(() => {
    if (!tab?.path) return [];
    const norm = tab.path.replace(/\\/g, '/');
    // Try to make the path relative to a known project root so the breadcrumb
    // shows "src › lib.rs" instead of the full absolute path.
    for (const p of projects) {
      const root = p.root_path.replace(/\\/g, '/').replace(/\/$/, '');
      if (norm.startsWith(root + '/')) {
        return norm.slice(root.length + 1).split('/').filter(Boolean);
      }
    }
    // Fallback: all path segments of the absolute path.
    return norm.split('/').filter(Boolean);
  }, [tab?.path, projects]);

  if (segments.length === 0) return null;

  return (
    <div
      className="flex h-[22px] shrink-0 items-center overflow-hidden border-b border-border/40 bg-background px-3"
      title={tab.path}
    >
      {segments.map((seg, i) => {
        const isLast = i === segments.length - 1;
        return (
          <React.Fragment key={i}>
            {i > 0 && (
              <span className="mx-1 select-none text-[10px] text-muted-foreground/60">›</span>
            )}
            {isLast ? (
              <FileSegment name={seg} fullPath={tab.path} />
            ) : (
              <FolderSegment
                name={seg}
                path={ancestorPath(tab.path, segments.length - 1 - i)}
                childPath={ancestorPath(tab.path, segments.length - 2 - i)}
              />
            )}
          </React.Fragment>
        );
      })}
    </div>
  );
}
