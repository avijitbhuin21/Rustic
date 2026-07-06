import React, { useEffect, useState } from 'react';
import { Shield, ShieldAlert, TriangleAlert } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';

const DESTRUCTIVE_RE =
  /(\bdelete\b|\bremove\b|\brm\b|\brmdir\b|\bdel\b|\bdrop\b|--force|-f\b|\bforce\b|reset --hard|\bkill\b|\bterminate\b|\bdestroy\b|\bpurge\b|\buninstall\b|\bformat\b|\bwipe\b)/i;
const WRITE_RE =
  /(\bwrite\b|\bedit\b|\bcreate\b|\bmove\b|\brename\b|\bcopy\b|\bmkdir\b|\binstall\b|\bapply\b|\bpatch\b|\bsave\b|\bcommit\b|\bpush\b|\bchmod\b|\bappend\b|\breplace\b|\bupdate\b|\bexecute\b|\brun\b|\bbash\b|\bcommand\b)/i;

function classifyRisk(pending) {
  const hay = [
    pending?.operation,
    pending?.description,
    typeof pending?.preview === 'string' ? pending.preview : '',
  ]
    .filter(Boolean)
    .join(' ');
  if (DESTRUCTIVE_RE.test(hay)) return 'destructive';
  if (WRITE_RE.test(hay)) return 'write';
  return 'read';
}

const RISK_META = {
  destructive: {
    icon: TriangleAlert,
    iconCls: 'text-red-500',
    titleCls: 'text-red-600 dark:text-red-400',
    label: 'Potentially destructive',
  },
  write: {
    icon: ShieldAlert,
    iconCls: 'text-amber-500',
    titleCls: '',
    label: 'Modifies files or runs commands',
  },
  read: {
    icon: Shield,
    iconCls: 'text-muted-foreground',
    titleCls: '',
    label: 'Read-only',
  },
};

function Kbd({ children }) {
  return (
    <kbd className="rounded border border-border bg-muted px-1 py-0.5 font-mono text-[10px]">
      {children}
    </kbd>
  );
}

export function PermissionPrompt() {
  const pending = useAgent((s) => s.pendingPermission);
  const respond = useAgent((s) => s.respondPermission);

  const [alwaysAllow, setAlwaysAllow] = useState(false);
  useEffect(() => {
    setAlwaysAllow(false);
  }, [pending?.request_id]);

  const open = !!pending;
  const risk = classifyRisk(pending);
  const meta = RISK_META[risk];
  const RiskIcon = meta.icon;

  const approve = () => respond(true, { alwaysAllow });
  const deny = () => respond(false);

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        // Dismissing (Esc / outside click) counts as Deny so the backend
        // isn't left parked waiting for an answer that never comes.
        if (!o) deny();
      }}
    >
      <DialogContent
        className="overflow-hidden sm:max-w-md"
        onKeyDown={(e) => {
          if (e.key !== 'Enter') return;
          // Let focused buttons/checkboxes keep their native Enter behaviour.
          if (e.target?.closest?.('button, [role="checkbox"], a')) return;
          e.preventDefault();
          approve();
        }}
      >
        <DialogHeader className="min-w-0">
          <DialogTitle className={cn('flex items-center gap-2', meta.titleCls)}>
            <RiskIcon className={cn('size-4 shrink-0', meta.iconCls)} />
            Approve agent action
            <span className="ml-auto shrink-0 text-[10px] font-normal text-muted-foreground">
              {meta.label}
            </span>
          </DialogTitle>
          <DialogDescription>
            {pending?.description || 'The agent is requesting permission to run an operation.'}
          </DialogDescription>
        </DialogHeader>
        {pending?.operation && (
          <div className="min-w-0 text-xs">
            <div className="mb-1 text-muted-foreground">Operation</div>
            <div
              className={cn(
                'break-all rounded border px-2 py-1 font-mono',
                risk === 'destructive'
                  ? 'border-red-500/40 bg-red-500/10'
                  : risk === 'write'
                    ? 'border-amber-500/30 bg-amber-500/10'
                    : 'border-border bg-muted/40',
              )}
            >
              {pending.operation}
            </div>
          </div>
        )}
        {pending?.preview && (
          <div className="min-w-0 text-xs">
            <div className="mb-1 text-muted-foreground">Preview</div>
            {/* whitespace-pre-wrap so long single-line commands wrap; break-all
                handles the no-whitespace pathological case (e.g. base64).
                min-w-0 on the surrounding flex/grid items prevents the dialog
                from stretching to fit pre's intrinsic content width. */}
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-all rounded border border-border bg-muted/40 p-2 font-mono text-[11px] leading-relaxed">
              {typeof pending.preview === 'string'
                ? pending.preview
                : JSON.stringify(pending.preview, null, 2)}
            </pre>
          </div>
        )}
        {pending?.operation && (
          <label className="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground">
            <Checkbox
              checked={alwaysAllow}
              onCheckedChange={(c) => setAlwaysAllow(!!c)}
            />
            <span>
              Always allow <span className="font-mono text-foreground/80">{pending.operation}</span> for this task
            </span>
          </label>
        )}
        <DialogFooter className="items-center">
          <span className="mr-auto flex items-center gap-1.5 text-[10px] text-muted-foreground">
            <Kbd>Esc</Kbd> Deny
            <span className="px-0.5" />
            <Kbd>Enter</Kbd> Approve
          </span>
          <Button variant="outline" onClick={deny}>
            Deny
          </Button>
          <Button
            className={cn(
              risk === 'destructive' &&
                'bg-red-600 text-white hover:bg-red-600/90',
            )}
            onClick={approve}
          >
            Approve
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default PermissionPrompt;
