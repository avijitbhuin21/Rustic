import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, Globe, ExternalLink } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { SettingsSection, SettingRow } from './setting-row';

const URL_KEY = 'rustic.remoteBackend.url';

/**
 * Remote backend (thin-client mode): point the app at a deployed
 * rustic-server. On connect the window navigates into the remote UI —
 * explorer, editor, terminals and agents all run in the cloud environment,
 * synced live. Restarting the app returns to the local workspace.
 */
export function RemoteBackendSettings() {
  const [url, setUrl] = useState(() => {
    try {
      return localStorage.getItem(URL_KEY) || '';
    } catch {
      return '';
    }
  });
  const [password, setPassword] = useState('');
  const [testing, setTesting] = useState(false);
  const [verified, setVerified] = useState(null); // normalized base URL after a passing test

  const persistUrl = (v) => {
    setUrl(v);
    setVerified(null);
    try {
      localStorage.setItem(URL_KEY, v);
    } catch {}
  };

  const testConnection = async () => {
    if (!url.trim()) return;
    setTesting(true);
    setVerified(null);
    try {
      const base = await invoke('remote_backend_test', { url: url.trim(), password });
      setVerified(base);
      toast.success('Connection verified');
      return base;
    } catch (e) {
      toast.error(String(e?.message || e));
      return null;
    } finally {
      setTesting(false);
    }
  };

  const connect = async () => {
    const base = verified || (await testConnection());
    if (!base) return;
    // Navigate the app window into the remote UI. The server's login screen
    // takes over auth (the session lives on that origin). Restarting the app
    // returns to the local workspace.
    window.location.href = base;
  };

  return (
    <SettingsSection title="Remote Backend">
      <SettingRow
        label="Server URL"
        description="A deployed rustic-server instance (e.g. https://rustic.example.com). Connecting turns this window into a thin client — everything runs in the cloud environment."
        htmlFor="remote-url"
      >
        <Input
          id="remote-url"
          type="url"
          placeholder="https://rustic.example.com"
          value={url}
          onChange={(e) => persistUrl(e.target.value)}
          className="h-7 w-64 text-xs"
        />
      </SettingRow>
      <SettingRow label="Password" description="The server's access password (used to verify the connection; you'll log in on the server itself)." htmlFor="remote-password">
        <Input
          id="remote-password"
          type="password"
          autoComplete="off"
          value={password}
          onChange={(e) => {
            setPassword(e.target.value);
            setVerified(null);
          }}
          className="h-7 w-64 text-xs"
        />
      </SettingRow>
      <SettingRow
        label="Connect"
        description={
          verified
            ? `Verified: ${verified}. Connect switches this window to the remote environment; restart Rustic to come back local.`
            : 'Test the connection, then connect.'
        }
      >
        <div className="flex items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            className="h-7 text-xs"
            disabled={testing || !url.trim()}
            onClick={testConnection}
          >
            {testing ? <Loader2 className="size-3 animate-spin" /> : <Globe className="size-3" />}
            Test
          </Button>
          <Button
            size="sm"
            className="h-7 text-xs"
            disabled={testing || !url.trim()}
            onClick={connect}
          >
            <ExternalLink className="size-3" />
            Connect
          </Button>
        </div>
      </SettingRow>
    </SettingsSection>
  );
}

export default RemoteBackendSettings;
