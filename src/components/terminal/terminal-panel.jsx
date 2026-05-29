import React, { useEffect } from 'react';
import { Plus, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useTerminal } from '@/state/terminal';
import { TERMINAL_PICKER_EVENT } from '@/components/terminal-project-picker';
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
  const closeTerminal = useTerminal((s) => s.closeTerminal);
  const setActiveSessionId = useTerminal((s) => s.setActiveSessionId);

  useEffect(() => {
    wireListeners();
    refreshSessions();
  }, [wireListeners, refreshSessions]);

  // Filter out hidden terminals.
  const sessions = allSessions.filter((s) => !hiddenSessionIds.has(s.id));
  // The active terminal: keep the current one if still alive, otherwise fall
  // back to the first *user* terminal (never auto-select an agent terminal —
  // those only surface here when explicitly opened from the chat dock).
  const activeId =
    sessions.find((s) => s.id === activeSessionId)?.id ??
    sessions.find((s) => !s.is_agent)?.id ??
    null;
  // The sidebar lists user terminals only. Agent terminals are tracked
  // separately in the chat dock; they appear here solely as the active pane
  // when the user explicitly opens one — so include the active session even
  // if it is an agent terminal.
  const listed = sessions.filter((s) => !s.is_agent || s.id === activeId);

  // One-click new terminal → project picker (same dialog the title-bar "+"
  // opens) so the user chooses which project's root to open in.
  const openTerminalPicker = () =>
    window.dispatchEvent(new Event(TERMINAL_PICKER_EVENT));

  return (
    <ResizablePanelGroup
      direction="horizontal"
      className="h-full w-full bg-background"
    >
      {/* Terminal content area (left) */}
      <ResizablePanel id="terminal-content" defaultSize="88%" minSize="40%">
        <div className="relative h-full w-full overflow-hidden">
          {listed.length === 0 ? (
            <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
              <Button variant="outline" size="sm" onClick={openTerminalPicker}>
                <Plus className="size-3 mr-1" />
                New Terminal
              </Button>
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

      <ResizableHandle />

      {/* Right-side vertical terminal tabs */}
      <ResizablePanel id="terminal-sidebar" defaultSize="12%" minSize="6%" maxSize="30%">
        <div className="flex h-full flex-col border-l border-border/60">
          <div className="flex-1 overflow-y-auto">
            {listed.map((s) => (
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
        </div>
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
