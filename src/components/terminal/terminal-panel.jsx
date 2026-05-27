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
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';

export function TerminalPanel() {
  const allSessions = useTerminal((s) => s.sessions);
  const hiddenSessionIds = useTerminal((s) => s.hiddenSessionIds);
  const activeSessionId = useTerminal((s) => s.activeSessionId);
  const wireListeners = useTerminal((s) => s.wireListeners);
  const refreshSessions = useTerminal((s) => s.refreshSessions);
  const createTerminal = useTerminal((s) => s.createTerminal);
  const hideTerminal = useTerminal((s) => s.hideTerminal);
  const closeTerminal = useTerminal((s) => s.closeTerminal);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);
  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);

  useEffect(() => {
    wireListeners();
    refreshSessions();
  }, [wireListeners, refreshSessions]);

  // Filter out hidden terminals
  const sessions = allSessions.filter((s) => !hiddenSessionIds.has(s.id));
  const activeId = sessions.find((s) => s.id === activeSessionId)?.id ?? sessions[0]?.id ?? null;

  // Open a terminal in a specific project's root. When `project` is null, the
  // shell inherits the app's cwd (handy when no project is open).
  const openTerminalIn = (project) => {
    createTerminal({
      cwd: project?.root_path,
      label: project?.name ?? 'shell',
    });
  };

  return (
    <ResizablePanelGroup
      direction="horizontal"
      className="h-full w-full bg-background"
    >
      {/* Left-side vertical terminal tabs (VS Code style) */}
      <ResizablePanel id="terminal-sidebar" defaultSize="12%" minSize="6%" maxSize="30%">
        <div className="flex h-full flex-col border-r border-border/60">
          <div className="flex-1 overflow-y-auto">
            {sessions.map((s) => (
              <div
                key={s.id}
                className={cn(
                  'group flex items-center justify-between border-b border-border/60 px-2 py-1.5 cursor-pointer text-xs',
                  activeId === s.id
                    ? 'bg-muted text-foreground'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                )}
                onClick={() => setActiveSessionId(s.id)}
              >
                <span className="truncate flex-1">{s.label || `pty ${s.id}`}</span>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTerminal(s.id);
                  }}
                  className="ml-1 rounded p-px text-muted-foreground opacity-0 hover:bg-destructive/20 hover:text-destructive group-hover:opacity-100"
                  title="Terminate terminal"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
          </div>
          <div className="border-t border-border/60 p-1">
            <NewTerminalMenu
              projects={projects}
              activeProjectId={activeProjectId}
              onPick={openTerminalIn}
              trigger={
                <Button variant="ghost" size="sm" className="w-full justify-start" title="New terminal">
                  <Plus className="size-3 mr-1" />
                  New Terminal
                </Button>
              }
            />
          </div>
        </div>
      </ResizablePanel>

      <ResizableHandle />

      {/* Terminal content area */}
      <ResizablePanel id="terminal-content" defaultSize="88%" minSize="40%">
        <div className="relative h-full w-full overflow-hidden">
          {sessions.length === 0 ? (
            <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
              <NewTerminalMenu
                projects={projects}
                activeProjectId={activeProjectId}
                onPick={openTerminalIn}
                trigger={
                  <Button variant="outline" size="sm">
                    <Plus className="size-3 mr-1" />
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
      </ResizablePanel>
    </ResizablePanelGroup>
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
