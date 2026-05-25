import React, { useEffect } from 'react';
import { Plus, X, FolderOpen } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';
import { useTerminal } from '@/state/terminal';
import { useExplorer } from '@/state/explorer';
import { TerminalPane } from './terminal-pane';

export function TerminalPanel({ location = 'bottom' } = {}) {
  const allSessions = useTerminal((s) => s.sessions);
  const sessionLocations = useTerminal((s) => s.sessionLocations);
  const activeSessionId = useTerminal((s) => s.activeSessionId);
  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);
  const createTerminal = useTerminal((s) => s.createTerminal);
  const closeTerminal = useTerminal((s) => s.closeTerminal);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);
  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);

  useEffect(() => {
    wireListeners();
    refreshSessions();
  }, [wireListeners, refreshSessions]);

  // Only show sessions that belong here. Untagged sessions (e.g. restored from
  // backend) default to 'tab' so they don't accidentally appear in the bottom
  // panel.
  const sessions = allSessions.filter((s) => (sessionLocations[s.id] ?? 'tab') === location);
  const activeId = sessions.find((s) => s.id === activeSessionId)?.id ?? sessions[0]?.id ?? null;

  // Open a terminal in a specific project's root. When `project` is null, the
  // shell inherits the app's cwd (handy when no project is open).
  const openTerminalIn = (project) => {
    createTerminal({
      cwd: project?.root_path,
      label: project?.name ?? 'shell',
      location,
    });
  };

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <div className="flex h-7 shrink-0 items-center border-b border-border/60 pl-1 text-xs">
        <div className="flex flex-1 items-center gap-px overflow-x-auto">
          {sessions.map((s) => (
            <div
              key={s.id}
              className={cn(
                'group flex h-7 items-center gap-1 border-r border-border/60 px-2 cursor-pointer',
                activeId === s.id
                  ? 'bg-muted text-foreground'
                  : 'text-muted-foreground hover:text-foreground'
              )}
              onClick={() => setActiveSessionId(s.id)}
            >
              <span className="truncate text-[11px]">{s.label || `pty ${s.id}`}</span>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  closeTerminal(s.id);
                }}
                className="ml-1 rounded p-px text-muted-foreground opacity-0 hover:bg-muted-foreground/20 group-hover:opacity-100"
              >
                <X className="size-3" />
              </button>
            </div>
          ))}
        </div>
        <NewTerminalMenu
          projects={projects}
          activeProjectId={activeProjectId}
          onPick={openTerminalIn}
          trigger={
            <Button variant="ghost" size="icon-xs" title="New terminal">
              <Plus className="size-3" />
            </Button>
          }
        />
      </div>
      <div className="relative flex-1 overflow-hidden">
        {sessions.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            <NewTerminalMenu
              projects={projects}
              activeProjectId={activeProjectId}
              onPick={openTerminalIn}
              trigger={
                <Button variant="outline" size="sm">
                  <Plus className="size-3" />
                  New Terminal
                </Button>
              }
            />
          </div>
        ) : (
          sessions.map((s) => (
            <div
              key={s.id}
              className={cn('absolute inset-0', activeId === s.id ? 'block' : 'hidden')}
            >
              <TerminalPane sessionId={s.id} active={activeId === s.id} />
            </div>
          ))
        )}
      </div>
    </div>
  );
}

// Dropdown for the "+" button: lets the user pick which project's root the
// new terminal opens in. Falls back to a single direct "New shell" item when
// no projects are open (keeps the click-to-create flow snappy in that case).
function NewTerminalMenu({ projects, activeProjectId, onPick, trigger }) {
  if (!projects || projects.length === 0) {
    return React.cloneElement(trigger, { onClick: () => onPick(null) });
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuLabel className="text-[11px] text-muted-foreground">
          New terminal in project
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {projects.map((p) => (
          <DropdownMenuItem
            key={p.id}
            onSelect={() => onPick(p)}
            className="gap-2 text-xs"
          >
            <FolderOpen className="size-3.5 text-muted-foreground" />
            <span className="flex-1 truncate">{p.name}</span>
            {p.id === activeProjectId && (
              <span className="text-[10px] text-muted-foreground">active</span>
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
