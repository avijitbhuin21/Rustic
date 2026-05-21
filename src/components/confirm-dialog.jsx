import React from 'react';
import { create } from 'zustand';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

const useConfirmStore = create((set) => ({
  open: false,
  title: '',
  description: '',
  confirmLabel: 'Confirm',
  cancelLabel: 'Cancel',
  destructive: false,
  resolver: null,

  request: (opts) =>
    new Promise((resolve) => {
      set({
        open: true,
        title: opts.title ?? 'Are you sure?',
        description: opts.description ?? '',
        confirmLabel: opts.confirmLabel ?? 'Confirm',
        cancelLabel: opts.cancelLabel ?? 'Cancel',
        destructive: !!opts.destructive,
        resolver: resolve,
      });
    }),

  resolve: (value) =>
    set((state) => {
      state.resolver?.(value);
      return { open: false, resolver: null };
    }),
}));

export function confirm(opts) {
  return useConfirmStore.getState().request(opts);
}

export function ConfirmDialogHost() {
  const open = useConfirmStore((s) => s.open);
  const title = useConfirmStore((s) => s.title);
  const description = useConfirmStore((s) => s.description);
  const confirmLabel = useConfirmStore((s) => s.confirmLabel);
  const cancelLabel = useConfirmStore((s) => s.cancelLabel);
  const destructive = useConfirmStore((s) => s.destructive);
  const resolve = useConfirmStore((s) => s.resolve);

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) resolve(false); }}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description && (
            <DialogDescription className="whitespace-pre-line">{description}</DialogDescription>
          )}
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => resolve(false)}>
            {cancelLabel}
          </Button>
          <Button
            variant={destructive ? 'destructive' : 'default'}
            size="sm"
            onClick={() => resolve(true)}
            autoFocus
          >
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
