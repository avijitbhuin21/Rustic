import React, { useCallback, useEffect, useState } from 'react';
import { ArrowLeft, Loader2, Play, Square, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { IS_WEB } from '@/lib/platform';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { confirm } from '@/components/confirm-dialog';
import { AgentLogo } from '@/components/icons/agent-logos';
import { TerminalPane } from '@/components/terminal/lazy-terminal-pane';
import { useExternalAgents } from '@/state/external-agents';
import { agentLabel } from './external-agent-launchers';
import { useCliBusy } from './cli-activity';
import { CliActivityList } from './cli-activity-list';
import { CliInsertPicker } from './cli-insert-picker';

const RUSTIC_FILE_MIME = 'application/x-rustic-file';

/** The persistent terminal instance for a PTY (its module is a lazy chunk). */
async function terminalInstance(ptySessionId) {
  const mod = await import('@/components/terminal/terminal-instance');
  return mod.acquireTerminalInstance(ptySessionId);
}

/** `path` relative to `root` with forward slashes when it sits inside it; otherwise unchanged. */
function relativeTo(root, path) {
  const norm = (p) => String(p).replace(/\\/g, '/').replace(/\/+$/, '');
  const base = root ? norm(root) : '';
  const full = norm(path);
  return base && full.startsWith(`${base}/`) ? full.slice(base.length + 1) : full;
}

/**
 * A CLI agent conversation (Claude Code / Codex / agy) rendered as a chat inside
 * the agent panel. The body is the agent's own PTY — the user types straight
 * into the tool, and stopping it is that tool's own Ctrl+C or the stop button.
 * Around it: an activity list fed by the CLI's hooks, and an insert picker /
 * file drop that paste text at the CLI's cursor but never submit it.
 *
 * The terminal instance is owned by terminal-instance.js, so switching to a
 * Rustic chat and back keeps the whole session on screen.
 */
export function CliChatView({ row, onBack }) {
  const ptySessionId = useExternalAgents((s) => s.ptyBySession[row.id]);
  const busy = useCliBusy(ptySessionId);
  const live = ptySessionId != null;
  const resumable = !!row.external_session_id;
  const [pickerOpen, setPickerOpen] = useState(false);
  const [dropping, setDropping] = useState(false);

  // Paste text at the CLI's cursor. Never submits (see insertText).
  const insert = useCallback(
    async (text) => {
      if (ptySessionId == null) return;
      try {
        (await terminalInstance(ptySessionId)).insertText(text);
      } catch (err) {
        toast.error(String(err));
      }
    },
    [ptySessionId],
  );

  // The CLI terminal sits on the chat surface, and its shortcut opens the picker.
  useEffect(() => {
    if (ptySessionId == null) return undefined;
    let inst = null;
    let gone = false;
    terminalInstance(ptySessionId)
      .then((i) => {
        if (gone) return;
        inst = i;
        inst.setSurface('--background');
        inst.setOnOpenPicker(() => setPickerOpen(true));
      })
      .catch(() => {});
    return () => {
      gone = true;
      inst?.setOnOpenPicker(null);
      inst?.setSurface(null);
    };
  }, [ptySessionId]);

  const draggedPath = (e) =>
    e.dataTransfer.types.includes(RUSTIC_FILE_MIME) ? e.dataTransfer.getData(RUSTIC_FILE_MIME) : '';

  const onDragOver = (e) => {
    if (!live || !e.dataTransfer.types.includes(RUSTIC_FILE_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    setDropping(true);
  };

  const onDrop = (e) => {
    setDropping(false);
    const path = draggedPath(e);
    if (!live || !path) return;
    e.preventDefault();
    insert(`@${relativeTo(row.cwd, path)} `);
  };

  const resume = async () => {
    try {
      await useExternalAgents.getState().openSession(row);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const stop = async () => {
    try {
      await useExternalAgents.getState().stop(row);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const remove = async () => {
    const shown = row.title || `${agentLabel(row.agent)} session`;
    const ok = await confirm({
      title: 'Delete CLI session?',
      description: `"${shown}"\nThe process is stopped and the CLI's own record of the conversation is deleted too. This can't be undone.`,
      confirmLabel: 'Delete',
      destructive: true,
      rememberKey: 'cli-session-delete',
    });
    if (!ok) return;
    try {
      await useExternalAgents.getState().forget(row);
      onBack();
    } catch (err) {
      toast.error(String(err));
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div
        className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2"
        style={{ paddingRight: IS_WEB ? undefined : 138 }}
      >
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="ghost" size="icon-xs" onClick={onBack}>
              <ArrowLeft className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
            Back to chat
          </TooltipContent>
        </Tooltip>
        {busy ? (
          <Loader2 className="size-3.5 shrink-0 animate-spin text-primary" />
        ) : (
          <AgentLogo agent={row.agent} className="size-3.5 shrink-0" />
        )}
        <span className="min-w-0 flex-1 truncate text-xs text-foreground/90">
          {row.title || `${agentLabel(row.agent)} session`}
        </span>
        <div className="ml-auto flex items-center gap-1">
          {live && (
            <CliInsertPicker
              projectRoot={row.cwd}
              open={pickerOpen}
              onOpenChange={setPickerOpen}
              onInsert={insert}
            />
          )}
          {live ? (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon-xs" onClick={stop}>
                  <Square className="size-3" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
                Stop {agentLabel(row.agent)} (resumable later)
              </TooltipContent>
            </Tooltip>
          ) : (
            resumable && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon-xs" onClick={resume}>
                    <Play className="size-3" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
                  Resume this session
                </TooltipContent>
              </Tooltip>
            )
          )}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={remove}
                className="hover:bg-destructive/20 hover:text-destructive"
              >
                <Trash2 className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
              Delete this session
            </TooltipContent>
          </Tooltip>
        </div>
      </div>
      <CliActivityList row={row} live={live} />
      <div
        className={`relative min-h-0 flex-1 bg-background px-3 pb-2 pt-2 ${dropping ? 'ring-1 ring-inset ring-info' : ''}`}
        onDragOverCapture={onDragOver}
        onDragLeave={() => setDropping(false)}
        onDropCapture={onDrop}
      >
        {live ? (
          <TerminalPane sessionId={ptySessionId} active />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
            <AgentLogo agent={row.agent} className="size-8 opacity-70" />
            <p className="text-xs text-muted-foreground">
              {resumable
                ? 'This session is stopped. Resuming starts the CLI again with its own history.'
                : 'This session never reported an id, so the CLI can\u2019t resume it.'}
            </p>
            {resumable && (
              <Button variant="secondary" size="sm" onClick={resume}>
                <Play className="mr-1.5 size-3" />
                Resume
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
