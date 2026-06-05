import React from 'react';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { useBreakpoint } from '@/lib/use-breakpoint';
import { useLayout } from '@/state/layout';
import { SettingsPanel } from './settings-panel';

export function SettingsModal() {
  const open = useLayout((s) => s.settingsOpen);
  const closeSettings = useLayout((s) => s.closeSettings);
  const { isPhone } = useBreakpoint();

  return (
    <Dialog open={open} onOpenChange={(v) => !v && closeSettings()}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className={cn(
          'p-0 gap-0 overflow-hidden',
          isPhone
            ? 'w-screen max-w-none h-[100dvh] max-h-[100dvh] rounded-none border-0'
            : 'w-[80vw] max-w-[80vw] sm:max-w-[80vw] h-[80vh] max-h-[80vh]',
        )}
      >
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <SettingsPanel onClose={closeSettings} />
      </DialogContent>
    </Dialog>
  );
}
