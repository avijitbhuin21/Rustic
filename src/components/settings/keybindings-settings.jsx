import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { useSettings } from '@/state/settings';
import { SettingsSection, SettingRow } from './setting-row';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function KeybindingsSettings() {
  const detect = useSettings((s) => s.detectVscodeKeybindings);
  const [importPath, setImportPath] = useState(null);
  const [busy, setBusy] = useState(false);

  const handleDetect = async () => {
    setBusy(true);
    try {
      const path = await detect();
      setImportPath(path);
    } finally {
      setBusy(false);
    }
  };

  const handleImportFile = async () => {
    if (!isTauri()) return;
    setBusy(true);
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (!path || typeof path !== 'string') return;
      await invoke('import_keybindings', { path });
      toast.success('Keybindings imported');
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleImportDetected = async () => {
    if (!isTauri() || !importPath) return;
    setBusy(true);
    try {
      await invoke('import_keybindings', { path: importPath });
      toast.success('Keybindings imported');
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <SettingsSection title="Import">
        <SettingRow
          label="Import VS Code keybindings"
          description="Detect and import your VS Code keybindings.json"
        >
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={handleDetect} disabled={busy}>
              {busy ? 'Working…' : 'Detect'}
            </Button>
            {importPath && (
              <Button size="sm" onClick={handleImportDetected} disabled={busy}>
                Import detected
              </Button>
            )}
          </div>
        </SettingRow>
        {importPath && (
          <div className="py-2 text-[11px] text-muted-foreground">
            Found: <span className="font-mono">{importPath}</span>
          </div>
        )}
        <SettingRow
          label="Import from file…"
          description="Choose a keybindings.json file on disk"
        >
          <Button variant="outline" size="sm" onClick={handleImportFile} disabled={busy}>
            Import file…
          </Button>
        </SettingRow>
      </SettingsSection>
    </>
  );
}
