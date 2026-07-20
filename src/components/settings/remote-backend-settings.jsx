import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, Globe, ExternalLink, CloudUpload, CloudDownload } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
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
  const [syncing, setSyncing] = useState(null); // 'push' | 'pull' | null
  const [confirming, setConfirming] = useState(null); // 'push' | 'pull' | null

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
    // The app window is frameless (decorations: false) and the custom titlebar
    // is part of the LOCAL UI — the remote server's web build has none. Turn on
    // native OS decorations before navigating so min/max/close survive the
    // switch; restarting the app returns to the frameless local shell.
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().setDecorations(true);
    } catch {}
    // Navigate the app window into the remote UI. The server's login screen
    // takes over auth (the session lives on that origin). Restarting the app
    // returns to the local workspace.
    window.location.href = base;
  };

  const runSync = async (direction) => {
    setConfirming(null);
    setSyncing(direction);
    const label = direction === 'push' ? 'Pushing to cloud…' : 'Pulling from cloud…';
    const toastId = toast.loading(label, { duration: Infinity });
    try {
      const msg = await invoke(direction === 'push' ? 'cloud_sync_push' : 'cloud_sync_pull', {
        url: url.trim(),
        password,
      });
      toast.success(msg, { id: toastId, duration: 4000 });
      if (direction === 'pull') {
        // The whole local environment was replaced in-process — reload the UI
        // so every store rehydrates from the imported state.
        setTimeout(() => window.location.reload(), 800);
      }
    } catch (e) {
      toast.error(String(e?.message || e), { id: toastId, duration: 8000 });
    } finally {
      setSyncing(null);
    }
  };

  return (
    <>
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

    <SettingsSection title="Cloud Sync">
      <SettingRow
        label="Push to cloud"
        description="Replace EVERYTHING on the server with a copy of this machine: projects, agent tasks & chat history, API keys. Projects unchanged since the last sync are skipped automatically; heavy folders (node_modules, target, venv…) never upload."
      >
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={!url.trim() || !password || syncing !== null}
          onClick={() => setConfirming('push')}
        >
          {syncing === 'push' ? (
            <Loader2 className="size-3 animate-spin" />
          ) : (
            <CloudUpload className="size-3" />
          )}
          Push
        </Button>
      </SettingRow>
      <SettingRow
        label="Pull from cloud"
        description="Replace EVERYTHING on this machine with a copy of the server: local projects, tasks and keys are overwritten. Projects unchanged since the last sync are skipped automatically. The app reloads when done."
      >
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={!url.trim() || !password || syncing !== null}
          onClick={() => setConfirming('pull')}
        >
          {syncing === 'pull' ? (
            <Loader2 className="size-3 animate-spin" />
          ) : (
            <CloudDownload className="size-3" />
          )}
          Pull
        </Button>
      </SettingRow>
    </SettingsSection>

    <Dialog open={confirming !== null} onOpenChange={(open) => !open && setConfirming(null)}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {confirming === 'push' ? 'Push everything to the cloud?' : 'Pull everything from the cloud?'}
          </DialogTitle>
          <DialogDescription>
            {confirming === 'push'
              ? 'The server\u2019s current data — projects, tasks, chat history, keys — will be permanently replaced with a copy of this machine. This cannot be undone.'
              : 'Everything on this machine — projects, tasks, chat history, keys — will be permanently replaced with the server\u2019s copy. This cannot be undone.'}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => setConfirming(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            size="sm"
            className="h-7 text-xs"
            onClick={() => runSync(confirming)}
          >
            {confirming === 'push' ? 'Replace cloud data' : 'Replace local data'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    </>
  );
}

export default RemoteBackendSettings;
