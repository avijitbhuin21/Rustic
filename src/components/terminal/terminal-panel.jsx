import React, { useEffect } from 'react';
import { Plus, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useTerminal } from '@/state/terminal';
import { useExplorer } from '@/state/explorer';
import { TerminalPane } from './terminal-pane';

export function TerminalPanel() {
  const sessions = useTerminal((s) => s.sessions);
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

  const handleNew = () => {
    const cwd = projects.find((p) => p.id === activeProjectId)?.root_path;
    createTerminal({ cwd, label: 'shell' });
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
                activeSessionId === s.id
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
        <Button variant="ghost" size="icon-xs" onClick={handleNew} title="New terminal">
          <Plus className="size-3" />
        </Button>
      </div>
      <div className="relative flex-1 overflow-hidden">
        {sessions.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            <Button variant="outline" size="sm" onClick={handleNew}>
              <Plus className="size-3" />
              New Terminal
            </Button>
          </div>
        ) : (
          sessions.map((s) => (
            <div
              key={s.id}
              className={cn('absolute inset-0', activeSessionId === s.id ? 'block' : 'hidden')}
            >
              <TerminalPane sessionId={s.id} active={activeSessionId === s.id} />
            </div>
          ))
        )}
      </div>
    </div>
  );
}
