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
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Approve agent action</DialogTitle>
          <DialogDescription>
            {pending?.description || 'The agent is requesting permission to run an operation.'}
          </DialogDescription>
        </DialogHeader>
        {pending?.operation && (
          <div className="text-xs">
            <div className="mb-1 text-muted-foreground">Operation</div>
            <div className="rounded border border-border bg-muted/40 px-2 py-1 font-mono">
              {pending.operation}
            </div>
          </div>
        )}
        {pending?.preview && (
          <div className="text-xs">
            <div className="mb-1 text-muted-foreground">Preview</div>
            <pre className="max-h-48 overflow-auto rounded border border-border bg-muted/40 p-2 font-mono text-[11px]">
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
