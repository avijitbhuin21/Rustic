import React from 'react';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { useLayout } from '@/state/layout';
import { SettingsPanel } from './settings-panel';

export function SettingsModal() {
  const open = useLayout((s) => s.settingsOpen);
  const closeSettings = useLayout((s) => s.closeSettings);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && closeSettings()}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[80vw] max-w-[80vw] sm:max-w-[80vw] h-[80vh] max-h-[80vh] p-0 gap-0 overflow-hidden"
      >
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <SettingsPanel onClose={closeSettings} />
      </DialogContent>
    </Dialog>
  );
}
