import React from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { useAgent } from '@/state/agent';

export function PermissionPrompt() {
  const pending = useAgent((s) => s.pendingPermission);
  const respond = useAgent((s) => s.respondPermission);
  const close = useAgent((s) => s.closePermission);

  const open = !!pending;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) close();
      }}
    >
      <DialogContent className="overflow-hidden sm:max-w-md">
        <DialogHeader className="min-w-0">
          <DialogTitle>Approve agent action</DialogTitle>
          <DialogDescription>
            {pending?.description || 'The agent is requesting permission to run an operation.'}
          </DialogDescription>
        </DialogHeader>
        {pending?.operation && (
          <div className="min-w-0 text-xs">
            <div className="mb-1 text-muted-foreground">Operation</div>
            <div className="break-all rounded border border-border bg-muted/40 px-2 py-1 font-mono">
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
        <DialogFooter>
          <Button variant="outline" onClick={() => respond(false)}>
            Deny
          </Button>
          <Button onClick={() => respond(true)}>Approve</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default PermissionPrompt;
