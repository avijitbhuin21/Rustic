import React, { useEffect, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { FolderGit2, TerminalSquare, Loader2 } from 'lucide-react';
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command';
import { useExplorer } from '@/state/explorer';
import { useTerminal } from '@/state/terminal';
import { useEditor } from '@/state/editor';

export const TERMINAL_PICKER_EVENT = 'rustic:open-terminal-picker';

async function spawnTerminal({ cwd, label }) {
  const info = await useTerminal.getState().createTerminal({ cwd, label });
  const title = info.pid != null ? `${label} • ${info.pid}` : label;
  // openTerminal shows the terminal in the bottom panel
  useEditor.getState().openTerminal(info.id, title);
}

export function TerminalProjectPicker() {
  const [open, setOpen] = useState(false);
  // 'picking' = list visible; 'spawning' = brief loader frame so the handoff
  // to the terminal doesn't feel like a hard cut. Holding the dialog open
  // ~250ms after the user picks gives time for the terminal to mount under
  // it, so the fade-out reveals an already-live shell.
  const [phase, setPhase] = useState('picking');
  const [spawningLabel, setSpawningLabel] = useState('');
  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);

  useEffect(() => {
    const onOpen = () => {
      setPhase('picking');
      setSpawningLabel('');
      setOpen(true);
    };
    window.addEventListener(TERMINAL_PICKER_EVENT, onOpen);
    return () => window.removeEventListener(TERMINAL_PICKER_EVENT, onOpen);
  }, []);

  const pick = async (project) => {
    const label = project ? project.name : 'Shell';
    setSpawningLabel(label);
    setPhase('spawning');
    try {
      // Kick off the spawn AND the visual delay in parallel; close after both
      // settle so the loader can't appear stuck if spawning is slow, and
      // can't flash by if spawning is fast.
      await Promise.all([
        spawnTerminal({
          cwd: project?.root_path,
          label: project?.name ?? 'shell',
        }),
        new Promise((r) => setTimeout(r, 260)),
      ]);
    } catch (err) {
      console.error('[terminal-picker] failed to spawn terminal:', err);
    } finally {
      setOpen(false);
    }
  };

  return (
    <CommandDialog
      open={open}
      onOpenChange={(o) => {
        // Block user-initiated close while we're spawning so the dialog can
        // own the handoff frame uninterrupted.
        if (phase === 'spawning' && !o) return;
        setOpen(o);
      }}
      title="Open terminal"
      description="Choose a project to open a terminal in."
      // Tailwind-merge lets these override the dialog's baked-in
      // duration-100 / zoom-in-95 so opening feels deliberate rather than
      // snapped. The longer duration + softer zoom matches the dialog's
      // existing fade pattern just at a more readable speed.
      className="duration-200 data-open:zoom-in-90 data-closed:zoom-out-95"
    >
      <Command shouldFilter={phase === 'picking'}>
        <AnimatePresence mode="wait" initial={false}>
          {phase === 'picking' ? (
            <motion.div
              key="picking"
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.18, ease: 'easeOut' }}
            >
              <CommandInput placeholder="Open terminal in…" autoFocus />
              <CommandList>
                <CommandEmpty>No projects found.</CommandEmpty>
                {projects.length > 0 && (
                  <CommandGroup heading="Projects">
                    {projects.map((p) => (
                      <CommandItem
                        key={p.id}
                        value={`${p.name} ${p.root_path}`}
                        onSelect={() => pick(p)}
                      >
                        <FolderGit2 className="size-3.5 text-muted-foreground" />
                        <span className="flex-1 truncate">{p.name}</span>
                        {p.id === activeProjectId && (
                          <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                            active
                          </span>
                        )}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                )}
                <CommandSeparator />
                <CommandGroup heading="Other">
                  <CommandItem value="shell no project" onSelect={() => pick(null)}>
                    <TerminalSquare className="size-3.5 text-muted-foreground" />
                    <span className="flex-1">Shell (no project)</span>
                  </CommandItem>
                </CommandGroup>
              </CommandList>
            </motion.div>
          ) : (
            <motion.div
              key="spawning"
              initial={{ opacity: 0, scale: 0.96 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.98 }}
              transition={{ duration: 0.18, ease: 'easeOut' }}
              className="flex items-center gap-2.5 px-3 py-5 text-sm text-muted-foreground"
            >
              <Loader2 className="size-4 animate-spin text-foreground/70" />
              <span>
                Opening terminal in{' '}
                <span className="text-foreground">{spawningLabel}</span>…
              </span>
            </motion.div>
          )}
        </AnimatePresence>
      </Command>
    </CommandDialog>
  );
}

export default TerminalProjectPicker;
