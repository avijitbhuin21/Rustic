import React, { useEffect, useState } from 'react';
import { Loader2, ExternalLink, Copy, KeyRound, MonitorSmartphone } from 'lucide-react';
import { GithubIcon } from '@/components/github/icon';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Button } from '@/components/ui/button';
import { useGithubAuth } from '@/state/github';

function PatPanel({ onSignedIn }) {
  const signInWithToken = useGithubAuth((s) => s.signInWithToken);
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(e) {
    e?.preventDefault();
    if (!token.trim() || busy) return;
    setBusy(true);
    try {
      const user = await signInWithToken(token);
      if (user) {
        toast.success(`Signed in as ${user.login}`);
        onSignedIn?.();
      } else {
        toast.error('Token saved but GitHub did not return a user. Check token scopes.');
      }
    } catch (err) {
      const msg = String(err);
      if (!msg.includes('cancelled')) toast.error(`Sign-in failed: ${msg}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3 py-2">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gh-pat">Personal Access Token</Label>
        <Input
          id="gh-pat"
          type="password"
          placeholder="ghp_… or github_pat_…"
          value={token}
          onChange={(e) => setToken(e.target.value)}
          autoFocus
          autoComplete="off"
          spellCheck={false}
        />
        <p className="text-[11px] leading-snug text-muted-foreground">
          Needs the <code className="rounded bg-muted px-1 py-px text-[10px]">repo</code> scope (and{' '}
          <code className="rounded bg-muted px-1 py-px text-[10px]">read:user</code> to show your username).
        </p>
      </div>
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={() =>
            openUrl(
              'https://github.com/settings/tokens/new?scopes=repo,read:user&description=Rustic%20IDE'
            ).catch(() => {})
          }
          className="inline-flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
        >
          <ExternalLink className="size-3" />
          Create a token on GitHub
        </button>
        <Button type="submit" size="sm" disabled={busy || !token.trim()}>
          {busy ? <Loader2 className="size-3.5 animate-spin" /> : null}
          Sign in
        </Button>
      </div>
    </form>
  );
}

function DevicePanel({ onSignedIn }) {
  const startDeviceFlow = useGithubAuth((s) => s.startDeviceFlow);
  const pollDeviceFlow = useGithubAuth((s) => s.pollDeviceFlow);
  const cancelDeviceFlow = useGithubAuth((s) => s.cancelDeviceFlow);
  const device = useGithubAuth((s) => s.device);
  const polling = useGithubAuth((s) => s.devicePolling);
  const error = useGithubAuth((s) => s.deviceError);
  const [starting, setStarting] = useState(false);

  // Reset on unmount — leaving the dialog while polling should stop the loop.
  useEffect(() => () => cancelDeviceFlow(), [cancelDeviceFlow]);

  async function begin() {
    setStarting(true);
    try {
      const d = await startDeviceFlow();
      if (!d) return;
      // Open the verification URL in the user's default browser.
      openUrl(d.verification_uri).catch(() => {});
      try {
        const user = await pollDeviceFlow();
        if (user) {
          toast.success(`Signed in as ${user.login}`);
          onSignedIn?.();
        }
      } catch (err) {
        const msg = String(err);
        if (!msg.includes('cancelled')) toast.error(`Sign-in failed: ${msg}`);
      }
    } catch (err) {
      toast.error(`Could not start device flow: ${err}`);
    } finally {
      setStarting(false);
    }
  }

  if (!device) {
    return (
      <div className="flex flex-col gap-3 py-2">
        <p className="text-[12px] leading-snug text-muted-foreground">
          We&apos;ll open GitHub in your browser and show a one-time code. Enter the code
          there to authorize Rustic — no need to copy any tokens by hand.
        </p>
        {error && (
          <p className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-[11px] text-destructive">
            {error}
          </p>
        )}
        <Button onClick={begin} disabled={starting} size="sm" className="self-start">
          {starting ? <Loader2 className="size-3.5 animate-spin" /> : <GithubIcon className="size-3.5" />}
          Continue with browser
        </Button>
      </div>
    );
  }

  // We have a device code — show it big, plus a copy button.
  const code = device.user_code;
  return (
    <div className="flex flex-col gap-3 py-2">
      <div className="flex flex-col gap-1.5">
        <Label className="text-[11px] uppercase tracking-wide text-muted-foreground">
          Your one-time code
        </Label>
        <div className="flex items-center gap-2">
          <code className="flex-1 rounded-md border border-border bg-muted/50 px-3 py-2 text-center font-mono text-lg tracking-[0.3em] text-foreground">
            {code}
          </code>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            onClick={() => {
              navigator.clipboard.writeText(code).then(
                () => toast.success('Code copied'),
                () => toast.error('Copy failed'),
              );
            }}
          >
            <Copy className="size-3.5" />
          </Button>
        </div>
        <p className="text-[11px] leading-snug text-muted-foreground">
          Paste this code at{' '}
          <button
            type="button"
            onClick={() => openUrl(device.verification_uri).catch(() => {})}
            className="inline-flex items-center gap-0.5 underline underline-offset-2 hover:text-foreground"
          >
            {device.verification_uri}
            <ExternalLink className="size-2.5" />
          </button>
          .
        </p>
      </div>

      <div className="flex items-center justify-between gap-2">
        <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground">
          {polling ? (
            <>
              <Loader2 className="size-3 animate-spin" />
              Waiting for authorization…
            </>
          ) : (
            'Stopped'
          )}
        </span>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => {
            cancelDeviceFlow();
          }}
        >
          Cancel
        </Button>
      </div>

      {error && (
        <p className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-[11px] text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}

export default function GithubSignInDialog() {
  const open = useGithubAuth((s) => s.dialogOpen);
  const closeDialog = useGithubAuth((s) => s.closeDialog);

  return (
    <Dialog open={open} onOpenChange={(o) => (o ? null : closeDialog())}>
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <GithubIcon className="size-4" />
            Sign in to GitHub
          </DialogTitle>
          <DialogDescription>
            Connect Rustic to your GitHub account to publish, push, and pull repositories.
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="browser" className="gap-3">
          <TabsList className="w-full">
            <TabsTrigger value="browser" className="gap-1.5">
              <MonitorSmartphone className="size-3.5" />
              Browser
            </TabsTrigger>
            <TabsTrigger value="token" className="gap-1.5">
              <KeyRound className="size-3.5" />
              Token
            </TabsTrigger>
          </TabsList>
          <TabsContent value="browser">
            <DevicePanel onSignedIn={closeDialog} />
          </TabsContent>
          <TabsContent value="token">
            <PatPanel onSignedIn={closeDialog} />
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
