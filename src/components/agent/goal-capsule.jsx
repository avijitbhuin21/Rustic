import React from 'react';
import { Target, X, Loader2 } from 'lucide-react';
import { useAgent } from '@/state/agent';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

// GoalCapsule — a pill in the prompt-box action row shown while a /goal is
// active on the current task. Displays the loop state + turn count and hosts
// the one-click cancel (equivalent to typing "/goal clear").

const STATUS_LABEL = {
  active: 'goal active',
  continuing: 'looping',
  evaluating: 'verifying…',
  unmet: 'looping',
  blocked: 'blocked',
  error: 'stopped',
};

/** Renders the active-goal pill for the current task, or nothing. */
export function GoalCapsule({ className }) {
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const goal = useAgent((s) => (s.activeTaskId ? s.goalByTask[s.activeTaskId] : null));
  const clearGoal = useAgent((s) => s.clearGoal);

  if (!activeTaskId || !goal) return null;

  const busy = goal.status === 'evaluating';
  const failed = goal.status === 'error';
  const blocked = goal.status === 'blocked';
  const label = STATUS_LABEL[goal.status] || 'goal active';

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          className={cn(
            'flex h-[22px] max-w-[220px] select-none items-center gap-1 rounded-full border px-2 text-[10px]',
            failed && 'border-destructive/50 text-destructive',
            blocked && 'border-warning/50 text-warning',
            !failed && !blocked && 'border-border text-muted-foreground',
            className,
          )}
        >
          {busy ? (
            <Loader2 className="size-3 shrink-0 animate-spin" />
          ) : (
            <Target className="size-3 shrink-0" />
          )}
          <span className="min-w-0 truncate">
            {label}
            {goal.turns > 0 ? ` · ${goal.turns}` : ''}
          </span>
          <button
            type="button"
            onClick={() => clearGoal(activeTaskId)}
            aria-label="Clear goal"
            className="ml-0.5 shrink-0 text-muted-foreground transition-colors hover:text-foreground"
          >
            <X className="size-3" />
          </button>
        </div>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-[320px]">
        <div className="font-medium">Goal: {goal.condition}</div>
        {goal.reason && (
          <div className="mt-1 text-muted-foreground">
            {failed ? 'Stopped: ' : blocked ? 'Blocked: ' : 'Last evaluation: '}
            {goal.reason}
          </div>
        )}
        <div className="mt-1 italic text-muted-foreground">
          {blocked
            ? 'The agent stopped the loop because it cannot finish this on its own. Send /goal again to restart it; any other message clears the goal.'
            : 'The agent keeps working until an evaluator model confirms this condition. Sending a message, clicking × or typing /goal clear ends it.'}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

export default GoalCapsule;
