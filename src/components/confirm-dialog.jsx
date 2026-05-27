import React from 'react';
import { create } from 'zustand';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

const useConfirmStore = create((set) => ({
  open: false,
  title: '',
  description: '',
  // Optional rich preview block rendered below the description. ReactNode,
  // not a string — callers can pass JSX (e.g. a list of files that will
  // be touched by a revert). Falsy = no preview block, just the
  // title/description.
  details: null,
  confirmLabel: 'Confirm',
  cancelLabel: 'Cancel',
  // Optional secondary confirm action. When set, a second button appears
  // between Cancel and the primary confirm. The dialog resolves to
  // `secondaryConfirmValue` (defaults to 'secondary') when the secondary
  // button is clicked, vs `true` for primary confirm and `false` for cancel.
  // Used by Revert to offer "Files only" alongside "Chat + files".
  secondaryConfirmLabel: null,
  secondaryConfirmValue: 'secondary',
  destructive: false,
  resolver: null,

  request: (opts) =>
    new Promise((resolve) => {
      set({
        open: true,
        title: opts.title ?? 'Are you sure?',
        description: opts.description ?? '',
        details: opts.details ?? null,
        confirmLabel: opts.confirmLabel ?? 'Confirm',
        cancelLabel: opts.cancelLabel ?? 'Cancel',
        secondaryConfirmLabel: opts.secondaryConfirmLabel ?? null,
        secondaryConfirmValue: opts.secondaryConfirmValue ?? 'secondary',
        destructive: !!opts.destructive,
        resolver: resolve,
      });
    }),

  resolve: (value) =>
    set((state) => {
      state.resolver?.(value);
      return {
        open: false,
        resolver: null,
        details: null,
        secondaryConfirmLabel: null,
      };
    }),
}));

export function confirm(opts) {
  return useConfirmStore.getState().request(opts);
}

export function ConfirmDialogHost() {
  const open = useConfirmStore((s) => s.open);
  const title = useConfirmStore((s) => s.title);
  const description = useConfirmStore((s) => s.description);
  const details = useConfirmStore((s) => s.details);
  const confirmLabel = useConfirmStore((s) => s.confirmLabel);
  const cancelLabel = useConfirmStore((s) => s.cancelLabel);
  const secondaryConfirmLabel = useConfirmStore((s) => s.secondaryConfirmLabel);
  const secondaryConfirmValue = useConfirmStore((s) => s.secondaryConfirmValue);
  const destructive = useConfirmStore((s) => s.destructive);
  const resolve = useConfirmStore((s) => s.resolve);

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) resolve(false); }}>
      <DialogContent
        showCloseButton={false}
        className="gap-3 p-5 sm:max-w-[460px]"
      >
        <DialogHeader className="gap-1.5">
          <DialogTitle className="text-sm font-medium">{title}</DialogTitle>
          {description && (
            <DialogDescription className="whitespace-pre-line text-[13px] leading-snug">
              {description}
            </DialogDescription>
          )}
        </DialogHeader>
        {details && (
          <div className="text-[12px] leading-snug text-foreground/85">
            {details}
          </div>
        )}
        <div className="flex justify-end gap-2">
          {/* For destructive confirms we autoFocus Cancel so that pressing
              Enter on dialog open dismisses rather than commits — without
              this, the user could accidentally confirm a revert/delete by
              hitting Enter or Space immediately after the dialog appears. */}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => resolve(false)}
            autoFocus={destructive}
          >
            {cancelLabel}
          </Button>
          {secondaryConfirmLabel && (
            <Button
              variant={destructive ? 'outline' : 'secondary'}
              size="sm"
              onClick={() => resolve(secondaryConfirmValue)}
            >
              {secondaryConfirmLabel}
            </Button>
          )}
          <Button
            variant={destructive ? 'destructive' : 'default'}
            size="sm"
            onClick={() => resolve(true)}
            autoFocus={!destructive}
          >
            {confirmLabel}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
