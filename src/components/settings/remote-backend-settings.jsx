import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, Globe, ExternalLink, CloudUpload, CloudDownload, LogOut } from 'lucide-react';
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

const PHASE_LABELS = {
  connecting: 'Connecting',
  preparing: 'Preparing',
  archiving: 'Packing files',
  compressing: 'Compressing',
  uploading: 'Uploading',
  applying: 'Server applying',
  packing: 'Server packing',
  downloading: 'Downloading',
  extracting: 'Extracting',
  installing: 'Installing',
  writing: 'Writing files',
  finalizing: 'Finalizing',
  done: 'Done',
};

/**
 * Remote backend (thin-client mode): point the app at a deployed
 * rustic-server. Connect opens the remote UI in its own window — explorer,
 * editor, terminals and agents all run in the cloud environment — while this
 * local workspace stays live. Closing that window (or Disconnect) ends the
 * remote session.
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
  const [opening, setOpening] = useState(false);
  const [remoteOpen, setRemoteOpen] = useState(false);
  const [progress, setProgress] = useState(null);

  useEffect(() => {
    invoke('remote_backend_is_open')
      .then((v) => setRemoteOpen(!!v))
      .catch(() => {});
  }, []);

  useEffect(() => {
    let unlisten;
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen('rustic:sync-progress', (e) => setProgress(e.payload)))
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

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
      // Remember the password so the explorer's per-project "Sync with cloud"
      // menu can run without re-prompting.
      invoke('cloud_sync_remember', { password }).catch(() => {});
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
    setOpening(true);
    try {
      await invoke('remote_backend_open', { url: base });
      setRemoteOpen(true);
      toast.success('Remote session opened in its own window');
    } catch (e) {
      toast.error(String(e?.message || e));
    } finally {
      setOpening(false);
    }
  };

  const disconnect = async () => {
    try {
      const was = await invoke('remote_backend_close');
      setRemoteOpen(false);
      if (was) toast.success('Remote session closed');
    } catch (e) {
      toast.error(String(e?.message || e));
    }
  };

  const runSync = async (direction) => {
    setConfirming(null);
    setSyncing(direction);
    setProgress({ direction, phase: 'connecting', detail: url.trim(), done: 0, total: 0 });
    const label = direction === 'push' ? 'Pushing to cloud…' : 'Pulling from cloud…';
    const toastId = toast.loading(label, { duration: Infinity });
    try {
      const msg = await invoke(direction === 'push' ? 'cloud_sync_push' : 'cloud_sync_pull', {
        url: url.trim(),
        password,
      });
      toast.success(msg, { id: toastId, duration: 4000 });
      invoke('cloud_sync_remember', { password }).catch(() => {});
      setProgress({ direction, phase: 'done', detail: msg, done: 1, total: 1 });
      if (direction === 'pull') {
        // The whole local environment was replaced in-process — reload the UI
        // so every store rehydrates from the imported state.
        setTimeout(() => window.location.reload(), 800);
      } else {
        setTimeout(() => setProgress(null), 4000);
      }
    } catch (e) {
      toast.error(String(e?.message || e), { id: toastId, duration: 8000 });
      setProgress(null);
    } finally {
      setSyncing(null);
    }
  };

  return (
    <>
    <SettingsSection title="Remote Backend">
      <SettingRow
        label="Server URL"
        description="A deployed rustic-server instance (e.g. https://rustic.example.com). Connecting opens that environment in a separate window — this local workspace keeps running."
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
          remoteOpen
            ? 'The remote session is open in its own window. Close that window (or Disconnect) to come back — this local workspace keeps running the whole time.'
            : verified
              ? `Verified: ${verified}. Connect opens the remote environment in a separate window; closing it returns you here.`
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
            disabled={testing || opening || !url.trim()}
            onClick={connect}
          >
            {opening ? (
              <Loader2 className="size-3 animate-spin" />
            ) : (
              <ExternalLink className="size-3" />
            )}
            {remoteOpen ? 'Focus' : 'Connect'}
          </Button>
          {remoteOpen && (
            <Button variant="outline" size="sm" className="h-7 text-xs" onClick={disconnect}>
              <LogOut className="size-3" />
              Disconnect
            </Button>
          )}
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
      {progress && <SyncProgressRow progress={progress} />}
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

/**
 * Live cloud-sync progress: current phase, the item being transferred, and a
 * determinate bar whenever the backend knows a total.
 */
function SyncProgressRow({ progress }) {
  const { direction, phase, detail, done = 0, total = 0 } = progress || {};
  const label = PHASE_LABELS[phase] || phase;
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : null;
  const finished = phase === 'done';
  return (
    <div className="space-y-1.5 border-t border-border px-3 py-2.5">
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="flex items-center gap-1.5 font-medium text-foreground">
          {finished ? (
            <CloudUpload className="size-3 text-emerald-500" />
          ) : (
            <Loader2 className="size-3 animate-spin text-muted-foreground" />
          )}
          {direction === 'pull' ? 'Pull' : 'Push'} — {label}
        </span>
        {pct !== null && !finished && (
          <span className="tabular-nums text-muted-foreground">{pct}%</span>
        )}
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className={
            pct === null && !finished
              ? 'h-full w-1/3 animate-pulse rounded-full bg-primary'
              : 'h-full rounded-full bg-primary transition-[width] duration-200'
          }
          style={pct === null && !finished ? undefined : { width: `${finished ? 100 : pct}%` }}
        />
      </div>
      {detail && (
        <div className="truncate text-[11px] text-muted-foreground" title={detail}>
          {detail}
        </div>
      )}
    </div>
  );
}

export default RemoteBackendSettings;