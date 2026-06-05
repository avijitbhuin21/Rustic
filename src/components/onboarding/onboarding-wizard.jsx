import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { Sparkles, FolderPlus, KeyRound, Check } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
} from '@/components/ui/select';
import { useExplorer } from '@/state/explorer';
import { IS_WEB } from '@/lib/platform';

const STORAGE_KEY = 'rustic.onboarding.completed';

function isTauri() {
  return IS_WEB || (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window);
}

// Pull the human message out of a `HTTP 401: {"error":{"message":"…"}}` style
// provider error so the user reads the reason, not raw JSON.
function prettyProviderError(raw) {
  const s = String(raw || '').trim();
  const brace = s.indexOf('{');
  if (brace !== -1) {
    try {
      const obj = JSON.parse(s.slice(brace));
      const msg = obj?.error?.message || obj?.message || obj?.error;
      if (typeof msg === 'string' && msg.trim()) {
        const prefix = s.slice(0, brace).trim().replace(/:$/, '');
        return prefix ? `${prefix} — ${msg.trim()}` : msg.trim();
      }
    } catch { /* fall through */ }
  }
  return s;
}

// Maps the friendly slug used in the UI to the backend `ProviderType`
// variant name expected by `set_ai_provider`, plus a sensible default model.
const PROVIDERS = [
  { value: 'anthropic', label: 'Anthropic (Claude)', providerType: 'Claude', defaultModel: 'claude-sonnet-4-5' },
  { value: 'openai', label: 'OpenAI', providerType: 'OpenAi', defaultModel: 'gpt-5-mini' },
  { value: 'gemini', label: 'Google Gemini', providerType: 'Gemini', defaultModel: 'gemini-2.5-flash' },
];

export function OnboardingWizard() {
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState(0);
  const [provider, setProvider] = useState('anthropic');
  const [apiKey, setApiKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [keyError, setKeyError] = useState('');

  const projects = useExplorer((s) => s.projects);
  const hasLoaded = useExplorer((s) => s.hasLoaded);
  const addProject = useExplorer((s) => s.addProject);

  useEffect(() => {
    if (!hasLoaded) return;
    const completed = localStorage.getItem(STORAGE_KEY);
    if (!completed && projects.length === 0) {
      setOpen(true);
    }
  }, [hasLoaded, projects.length]);

  // Re-open the wizard on demand (Settings → Shortcuts → Run Setup Wizard).
  useEffect(() => {
    const onOpen = () => { setStep(0); setOpen(true); };
    window.addEventListener('rustic:open-onboarding', onOpen);
    return () => window.removeEventListener('rustic:open-onboarding', onOpen);
  }, []);

  const finish = () => {
    localStorage.setItem(STORAGE_KEY, '1');
    setOpen(false);
  };

  const handleAddFolder = async () => {
    if (!isTauri()) {
      setStep((s) => s + 1);
      return;
    }
    try {
      const path = await openDirDialog();
      if (typeof path === 'string') {
        await addProject(path);
        setStep((s) => s + 1);
      }
    } catch (e) {}
  };

  const handleSaveProvider = async () => {
    if (!apiKey.trim()) {
      setStep((s) => s + 1);
      return;
    }
    setBusy(true);
    setKeyError('');
    try {
      if (isTauri() || IS_WEB) {
        const entry = PROVIDERS.find((p) => p.value === provider) ?? PROVIDERS[0];
        // Verify the key against the live provider before storing it, so an
        // invalid key reports the real reason here instead of failing later.
        try {
          await invoke('fetch_ai_models', {
            providerType: entry.providerType,
            apiKey: apiKey.trim(),
            baseUrl: null,
            forceRefresh: true,
            includeAll: false,
          });
        } catch (e) {
          setKeyError(prettyProviderError(e));
          return;
        }
        await invoke('set_ai_provider', {
          providerType: entry.providerType,
          apiKey: apiKey.trim(),
          model: entry.defaultModel,
          baseUrl: null,
          name: null,
        });
      }
      setStep((s) => s + 1);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-md">
        {step === 0 && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <Sparkles className="size-4 text-primary" />
                Welcome to Rustic
              </DialogTitle>
              <DialogDescription>
                A VS Code-style editor with a built-in AI agent. Let's get you set up in two steps.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-end gap-2 pt-4">
              <Button variant="ghost" size="sm" onClick={finish}>Skip</Button>
              <Button size="sm" onClick={() => setStep(1)}>Get started</Button>
            </div>
          </>
        )}

        {step === 1 && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <FolderPlus className="size-4 text-primary" />
                Add your first project
              </DialogTitle>
              <DialogDescription>
                Pick a folder to open. You can add more later from the Explorer.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-end gap-2 pt-4">
              <Button variant="ghost" size="sm" onClick={() => setStep(2)}>Skip</Button>
              <Button size="sm" onClick={handleAddFolder}>Choose folder…</Button>
            </div>
          </>
        )}

        {step === 2 && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <KeyRound className="size-4 text-primary" />
                Configure an AI provider
              </DialogTitle>
              <DialogDescription>
                Add a key so the AI agent can run. You can change this later in Settings → AI.
              </DialogDescription>
            </DialogHeader>
            <div className="flex flex-col gap-3 pt-2">
              <div className="flex flex-col gap-1">
                <Label className="text-xs">Provider</Label>
                <Select value={provider} onValueChange={setProvider}>
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROVIDERS.map((p) => (
                      <SelectItem key={p.value} value={p.value}>{p.label}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1">
                <Label className="text-xs">API key</Label>
                <Input
                  type="password"
                  value={apiKey}
                  onChange={(e) => { setApiKey(e.target.value); if (keyError) setKeyError(''); }}
                  placeholder="Paste your key (leave blank to skip)"
                  className="h-8 text-xs"
                />
                {keyError && (
                  <div className="text-[11px] text-destructive break-all">{keyError}</div>
                )}
              </div>
            </div>
            <div className="flex justify-end gap-2 pt-4">
              <Button variant="ghost" size="sm" onClick={() => setStep(3)} disabled={busy}>Skip</Button>
              <Button size="sm" onClick={handleSaveProvider} disabled={busy}>
                {busy ? 'Verifying…' : 'Continue'}
              </Button>
            </div>
          </>
        )}

        {step === 3 && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <Check className="size-4 text-emerald-500" />
                You're all set
              </DialogTitle>
              <DialogDescription>
                Press Ctrl+P to open files, Ctrl+Shift+P for the command palette, and the Agent tab in the activity bar to chat.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-end pt-4">
              <Button size="sm" onClick={finish}>Done</Button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

async function openDirDialog() {
  return open({ directory: true, multiple: false });
}
