import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useBrowser } from '@/state/browser';
import { SettingsSection, SettingRow } from './setting-row';

const MODES = [
  { id: 'path', label: 'Path', hint: 'Zero-config. Works anywhere; SPAs with absolute asset paths may need a dev-server base.' },
  { id: 'subdomain', label: 'Subdomain', hint: 'Your wildcard domain (3000.preview.example.com). Apps work unmodified, stays behind your login.' },
];

/** Web-only Settings section to configure how a VM dev server is opened in the user's own browser. */
export function TunnelSettings() {
  const [mode, setMode] = useState('path');
  const [previewDomain, setPreviewDomain] = useState('');
  const [cookieDomain, setCookieDomain] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState(null); // { ok: bool, msg: string }

  useEffect(() => {
    invoke('get_tunnel_config')
      .then((cfg) => {
        setMode(cfg?.mode || 'path');
        setPreviewDomain(cfg?.previewDomain || '');
        setCookieDomain(cfg?.cookieDomain || '');
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  const save = async () => {
    setSaving(true);
    setStatus(null);
    try {
      const res = await invoke('set_tunnel_config', { mode, previewDomain, cookieDomain });
      useBrowser.setState({
        previewDomain: res?.mode === 'subdomain' ? res?.previewDomain || null : null,
      });
      setStatus({ ok: true, msg: 'Saved.' });
    } catch (e) {
      setStatus({ ok: false, msg: String(e?.message || e) });
    } finally {
      setSaving(false);
    }
  };

  if (loading) return null;

  return (
    <SettingsSection title="Preview Tunnel">
      <SettingRow
        label="Open-in-browser mode"
        description="How the per-tab 'Open in my browser' button reaches a dev server running in the VM."
      >
        <div className="flex gap-1.5">
          {MODES.map((m) => (
            <button
              key={m.id}
              onClick={() => setMode(m.id)}
              className={cn(
                'rounded-md px-2.5 py-1 text-[12px] transition-colors',
                mode === m.id
                  ? 'bg-accent text-accent-foreground font-medium'
                  : 'text-muted-foreground hover:bg-accent/40 hover:text-foreground',
              )}
            >
              {m.label}
            </button>
          ))}
        </div>
      </SettingRow>

      <SettingRow label="" description={MODES.find((m) => m.id === mode)?.hint}>
        <span />
      </SettingRow>

      {mode === 'subdomain' && (
        <>
          <SettingRow
            label="Preview domain"
            description="Wildcard host pointed at this server, e.g. preview.example.com (used as 3000.preview.example.com)."
            htmlFor="tunnel-preview-domain"
          >
            <Input
              id="tunnel-preview-domain"
              value={previewDomain}
              onChange={(e) => setPreviewDomain(e.target.value)}
              placeholder="preview.example.com"
              className="h-7 w-56 text-xs"
            />
          </SettingRow>
          <SettingRow
            label="Cookie domain"
            description="Parent domain so your login cookie reaches the preview subdomains, e.g. .example.com."
            htmlFor="tunnel-cookie-domain"
          >
            <Input
              id="tunnel-cookie-domain"
              value={cookieDomain}
              onChange={(e) => setCookieDomain(e.target.value)}
              placeholder=".example.com"
              className="h-7 w-56 text-xs"
            />
          </SettingRow>
        </>
      )}

      <SettingRow
        label="Apply"
        description={
          status
            ? status.msg
            : 'Changes take effect immediately (no restart). Subdomain mode also requires the main app to be served on the same parent domain.'
        }
      >
        <Button size="sm" onClick={save} disabled={saving} className="h-7 text-xs">
          {saving ? 'Saving…' : 'Save'}
        </Button>
      </SettingRow>
    </SettingsSection>
  );
}
