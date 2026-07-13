import React, { useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { Sparkles } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useExplorer } from '@/state/explorer';
import { notesForVersion, LATEST_NOTES } from '@/lib/changelog';

const SEEN_KEY = 'rustic.patchnotes.seen';
const ONBOARDING_KEY = 'rustic.onboarding.completed';

const TAG_LABELS = { new: 'New', improved: 'Improved', fixed: 'Fixed' };

/** Once-per-version "What's New" dialog; auto-shows after an update, re-openable via the rustic:open-patch-notes event. */
export function PatchNotesDialog() {
  const [open, setOpen] = useState(false);
  const [notes, setNotes] = useState(null);
  const [appVersion, setAppVersion] = useState('');
  const projects = useExplorer((s) => s.projects);
  const hasLoaded = useExplorer((s) => s.hasLoaded);
  const decidedRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    getVersion()
      .then((v) => { if (!cancelled) setAppVersion(v); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);

  // Auto-show once per version. Decided a single time after stores load so a
  // project added mid-onboarding can't pop the dialog over the wizard; if the
  // onboarding wizard is about to show instead, wait for it to finish.
  useEffect(() => {
    if (decidedRef.current || !hasLoaded || !appVersion) return;
    decidedRef.current = true;
    const entry = notesForVersion(appVersion);
    if (!entry) return;
    if (localStorage.getItem(SEEN_KEY) === appVersion) return;
    const show = () => { setNotes(entry); setOpen(true); };
    const onboardingPending = !localStorage.getItem(ONBOARDING_KEY) && projects.length === 0;
    if (onboardingPending) {
      window.addEventListener('rustic:onboarding-finished', show, { once: true });
      return;
    }
    show();
  }, [hasLoaded, appVersion, projects.length]);

  useEffect(() => {
    const onOpen = () => {
      setNotes(notesForVersion(appVersion) ?? LATEST_NOTES);
      setOpen(true);
    };
    window.addEventListener('rustic:open-patch-notes', onOpen);
    return () => window.removeEventListener('rustic:open-patch-notes', onOpen);
  }, [appVersion]);

  const handleOpenChange = (next) => {
    if (!next && appVersion) localStorage.setItem(SEEN_KEY, appVersion);
    setOpen(next);
  };

  if (!notes) return null;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Sparkles className="size-4 text-primary" />
            What's New in v{notes.version}
          </DialogTitle>
          <DialogDescription>{notes.date}</DialogDescription>
        </DialogHeader>
        <ScrollArea className="max-h-[55vh] pr-3">
          <ul className="flex flex-col gap-2.5">
            {notes.entries.map((entry, i) => (
              <li key={i} className="flex items-start gap-2.5">
                <span className="mt-0.5 w-16 shrink-0 rounded-md border border-border/60 bg-muted/40 px-1.5 py-0.5 text-center text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                  {TAG_LABELS[entry.tag] ?? entry.tag}
                </span>
                <span className="text-[13px] leading-snug text-foreground/90">{entry.text}</span>
              </li>
            ))}
          </ul>
        </ScrollArea>
        <DialogFooter>
          <Button size="sm" onClick={() => handleOpenChange(false)}>Got it</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
