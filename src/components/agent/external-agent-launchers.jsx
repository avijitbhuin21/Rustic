import React, { useEffect, useMemo } from 'react';
import { Loader2, Play, Square, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { confirm } from '@/components/confirm-dialog';
import { useExternalAgents, launchKey } from '@/state/external-agents';
import { useRelativeTime } from '@/lib/relative-time';
import { AgentLogo } from '@/components/icons/agent-logos';
import { useCliBusy } from './cli-activity';
import { cn } from '@/lib/utils';

const EMPTY_ROWS = [];

/** Display name for an agent kind, used before the CLI reports a title. */
export function agentLabel(agent) {
  if (agent === 'claude') return 'Claude Code';
  if (agent === 'codex') return 'Codex';
  if (agent === 'antigravity') return 'Antigravity';
  return agent;
}

// Row of launch icons — one per CLI agent installed on this machine.
export function ExternalAgentButtons({ project, className }) {
  const installed = useExternalAgents((s) => s.installed);
  const launching = useExternalAgents((s) => s.launching);
  const detect = useExternalAgents((s) => s.detect);

  useEffect(() => {
    detect().catch(() => {});
  }, [detect]);

  if (!installed.length) return null;

  const launch = async (e, agent) => {
    e.stopPropagation();
    try {
      await useExternalAgents.getState().spawn(agent.agent, project.id);
    } catch (err) {
      toast.error(String(err));
    }
  };

  return (
    <>
      {installed.map((agent) => {
        const busy = launching === launchKey(project.id, agent.agent);
        const updatable = !!agent.latestVersion;
        const duplicates = agent.shadowed?.length ?? 0;
        return (
          <Tooltip key={agent.agent}>
            <TooltipTrigger asChild>
              <button
                onClick={(e) => launch(e, agent)}
                disabled={busy}
                className={cn(
                  'relative flex size-5 items-center justify-center rounded transition-opacity hover:bg-foreground/10 disabled:opacity-50',
                  className,
                )}
              >
                {busy ? (
                  <Loader2 className="size-3 animate-spin" />
                ) : (
                  <AgentLogo agent={agent.agent} className="size-3.5" />
                )}
                {updatable && !busy && (
                  <span className="absolute right-0 top-0 size-1.5 rounded-full bg-primary ring-1 ring-background" />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="max-w-64 space-y-1">
              <div>New {agent.label} session here</div>
              {agent.version && (
                <div className="text-muted-foreground">
                  v{agent.version}
                  {updatable && ` — v${agent.latestVersion} available`}
                </div>
              )}
              {updatable && (
                <div className="text-muted-foreground">
                  Update from inside a session; the next launch uses the new version.
                </div>
              )}
              {duplicates > 0 && (
                <div className="text-muted-foreground">
                  {duplicates} other cop{duplicates === 1 ? 'y' : 'ies'} on PATH — Rustic runs the
                  newest.
                </div>
              )}
            </TooltipContent>
          </Tooltip>
        );
      })}
    </>
  );
}

// One CLI agent conversation, rendered as a chat row among the project's normal
// Rustic chats. Live sessions spin while the tool works; stopped ones resume on
// click. "Stop" keeps the conversation resumable, the bin deletes it everywhere.
export function CliSessionRow({ row, active }) {
  const timestampMs = React.useMemo(() => {
    const t = new Date(row.last_active_at ?? row.created_at ?? 0).getTime();
    return Number.isFinite(t) && t > 0 ? t : null;
  }, [row.last_active_at, row.created_at]);
  const relative = useRelativeTime(timestampMs);
  const ptySessionId = useExternalAgents((s) => s.ptyBySession[row.id]);
  const busy = useCliBusy(ptySessionId);
  const live = ptySessionId != null;
  const resumable = !!row.external_session_id;

  const open = async () => {
    try {
      await useExternalAgents.getState().openSession(row);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const stop = async (e) => {
    e.stopPropagation();
    try {
      await useExternalAgents.getState().stop(row);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const forget = async (e) => {
    e.stopPropagation();
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
    } catch (err) {
      toast.error(String(err));
    }
  };

  return (
    <div
      role="button"
      onClick={open}
      className={cn(
        'group/session flex h-7 cursor-pointer items-center gap-1.5 pl-6 pr-2 text-xs',
        'hover:bg-foreground/[0.06]',
        active && 'bg-primary/15 text-foreground',
        !live && !resumable && 'cursor-default opacity-60',
      )}
    >
      <span className="flex size-3 shrink-0 items-center justify-center">
        {busy ? (
          <Loader2 className="size-3 animate-spin text-primary" />
        ) : (
          <AgentLogo agent={row.agent} className="size-3.5" />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate text-foreground/90">
        {row.title || `${agentLabel(row.agent)} session`}
      </span>
      {relative && (
        <span className="shrink-0 select-none text-[10px] tabular-nums text-muted-foreground/70 group-hover/session:hidden">
          {relative}
        </span>
      )}
      <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/session:opacity-100">
        {live ? (
          <button
            onClick={stop}
            title="Stop the CLI (the conversation stays resumable)"
            className="flex size-4 items-center justify-center rounded hover:bg-foreground/10"
          >
            <Square className="size-2.5" />
          </button>
        ) : (
          resumable && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                open();
              }}
              title="Resume this session"
              className="flex size-4 items-center justify-center rounded hover:bg-foreground/10"
            >
              <Play className="size-2.5" />
            </button>
          )
        )}
        <button
          onClick={forget}
          title="Delete this session (stops it and removes its transcript)"
          className="flex size-4 items-center justify-center rounded hover:bg-destructive/20 hover:text-destructive"
        >
          <Trash2 className="size-2.5" />
        </button>
      </div>
    </div>
  );
}

/**
 * Loads (once per project) and returns the project's CLI agent sessions so the
 * chat tree can interleave them with normal Rustic chats. The current
 * placeholder session is left out: like a new Rustic chat, it only earns a row
 * once a first message has been sent.
 */
export function useProjectCliSessions(projectId, enabled = true) {
  const rows = useExternalAgents((s) => s.sessionsByProject[projectId]);
  const draftRowId = useExternalAgents((s) => s.draftRowId);
  const loadSessions = useExternalAgents((s) => s.loadSessions);

  useEffect(() => {
    if (enabled && projectId) loadSessions(projectId).catch(() => {});
  }, [enabled, projectId, loadSessions]);

  return useMemo(() => {
    if (!rows?.length) return EMPTY_ROWS;
    if (!draftRowId) return rows;
    return rows.filter((r) => r.id !== draftRowId);
  }, [rows, draftRowId]);
}
