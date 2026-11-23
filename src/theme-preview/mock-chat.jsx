// Agent chat column and the sample dialog of the theme preview's mock window.
// Rows copy the markup of chat-turn / tool-call-card / ask-user-inline /
// prompt-box; every control is the real component.
import React from 'react';
import {
  ArrowUp, BookOpen, CheckCircle2, ChevronDown, CircleDotDashed, CircleX, Ellipsis, FolderGit2, HelpCircle,
  PanelRight, Pencil, Plus, ScrollText, Server, SquareTerminal, Trash2, Workflow,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import {
  DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

// The preview sits in an iframe next to the host page's form: nothing here may
// take keyboard focus on its own, and the open scenes stay open.
const keep = (e) => e.preventDefault();
const PINNED = { onOpenAutoFocus: keep, onCloseAutoFocus: keep, onEscapeKeyDown: keep, onPointerDownOutside: keep, onInteractOutside: keep };

const INLINE_CODE = 'rounded bg-muted px-1 py-0.5 font-mono text-[11px]';

function ToolRow({ status, name, detail, outcome }) {
  return (
    <div className="group flex w-full items-center gap-2 rounded-md py-1 pr-2 text-left text-xs">
      <span className="relative z-10 flex shrink-0 bg-sidebar">
        <span className="flex items-center justify-center px-0.5">
          {status === 'done' ? <CheckCircle2 className="size-4 text-success" />
            : status === 'failed' ? <CircleX className="size-4 text-danger" />
            : <CircleDotDashed className="size-4 animate-spin text-info [animation-duration:3s]" />}
        </span>
      </span>
      <span className="flex min-w-0 flex-1 items-baseline gap-1.5 truncate">
        <span className="shrink-0 font-mono text-foreground">{name}</span>
        <span className="min-w-0 truncate font-sans text-muted-foreground">{detail}</span>
      </span>
      {outcome && (
        <span
          className={cn(
            'shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium',
            status === 'failed' ? 'bg-destructive/10 text-destructive' : 'bg-primary/10 text-primary',
          )}
        >
          {outcome}
        </span>
      )}
    </div>
  );
}

function ChatMenu({ open }) {
  return (
    <DropdownMenu open={open} modal={false}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-xs" aria-label="Agent tools"><Ellipsis className="size-3.5" /></Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52" {...PINNED}>
        <DropdownMenuLabel>Agent tools</DropdownMenuLabel>
        <DropdownMenuItem><Server className="size-3.5" /> MCP servers</DropdownMenuItem>
        <DropdownMenuItem><ScrollText className="size-3.5" /> Rules</DropdownMenuItem>
        <DropdownMenuItem><BookOpen className="size-3.5" /> Skills <DropdownMenuShortcut>⌘K</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem><Workflow className="size-3.5" /> Workflows</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuCheckboxItem checked>Auto-approve edits</DropdownMenuCheckboxItem>
        <DropdownMenuItem disabled>Export chat (JSON)</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive"><Trash2 className="size-3.5" /> Clear chat</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AskCard() {
  return (
    <div data-ink-frame="card" className="ml-7 overflow-hidden rounded-lg border border-border/70 bg-card shadow-sm">
      <div className="flex items-center justify-between gap-3 border-b border-border/60 px-3 py-2">
        <div className="flex items-center gap-1.5 text-xs font-medium">
          <HelpCircle className="size-3 text-primary" /> Question from the agent
        </div>
        <Badge variant="secondary">1 of 1</Badge>
      </div>
      <div className="space-y-2 px-3 py-2.5 text-xs">
        <div>Which port should the dev server use?</div>
        <label className="flex items-center gap-2"><Checkbox defaultChecked /> 4001 (next free port)</label>
        <label className="flex items-center gap-2"><Checkbox /> Stop whatever is using 4000</label>
      </div>
      <div className="flex justify-end gap-2 border-t border-border/60 px-3 py-2">
        <Button variant="outline" size="sm">Skip</Button>
        <Button size="sm">Send answer</Button>
      </div>
    </div>
  );
}

function Composer() {
  return (
    <div
      data-ink-frame="prompt"
      className="rounded-3xl border border-border/70 bg-popover p-2 shadow-[0_8px_30px_color-mix(in_oklab,var(--scrim)_24%,transparent)]"
    >
      <div className="min-h-[44px] px-3 py-2 text-xs leading-relaxed text-muted-foreground">Ask the agent…</div>
      <div className="flex items-center justify-between gap-2 pt-2">
        <div className="inline-flex items-center overflow-hidden rounded-md border border-border/60 bg-muted/20">
          <span className="flex h-7 items-center px-2"><Pencil className="size-3.5" /></span>
          <div className="h-4 w-px bg-border/60" />
          <span className="flex h-7 items-center gap-1 px-2 text-xs">Model <ChevronDown className="size-3 text-muted-foreground" /></span>
        </div>
        <button type="button" aria-label="Send" className="flex size-7 items-center justify-center rounded-full bg-primary text-primary-foreground">
          <ArrowUp className="size-4" />
        </button>
      </div>
    </div>
  );
}

/** The agent chat column. `menuOpen` keeps its "…" menu open. */
export function MockChat({ menuOpen }) {
  return (
    <div className="flex h-full flex-col bg-sidebar">
      <div className="flex h-8 shrink-0 items-center gap-1.5 border-b border-border px-2">
        <Button variant="ghost" size="xs" className="gap-1.5">
          <FolderGit2 className="size-3.5" /> moonlight <ChevronDown className="size-3 text-muted-foreground" />
        </Button>
        <div className="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="icon-xs"><Plus className="size-3.5" /></Button>
          <Button variant="ghost" size="icon-xs"><SquareTerminal className="size-3.5" /></Button>
          <ChatMenu open={menuOpen} />
          <Button variant="ghost" size="icon-xs"><PanelRight className="size-3.5" /></Button>
        </div>
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-hidden px-3 py-3">
        <div data-ink-frame="card" className="rounded-md border border-border/50 bg-muted/60 px-3 py-2">
          <div className="whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground">
            The dev server will not start. Can you find out why and fix it?
          </div>
        </div>
        <div className="relative">
          <div className="pointer-events-none absolute bottom-2 left-[9px] top-2 border-l-2 border-dashed border-muted-foreground/25" />
          <div className="space-y-1">
            <ToolRow status="done" name="read_file" detail="src/server.rs" outcome="Done" />
            <ToolRow status="failed" name="run_command" detail="cargo run" outcome="Failed" />
            <div className="py-1 pl-7 text-xs leading-relaxed">
              Port <code className={INLINE_CODE}>4000</code> is already taken, so{' '}
              <code className={INLINE_CODE}>bind()</code> fails. See{' '}
              <a className="text-primary underline">server.rs:42</a>.
            </div>
            <ToolRow status="running" name="edit_file" detail="Reading the port from PORT" />
            <AskCard />
          </div>
        </div>
      </div>
      <div className="shrink-0 px-3 pb-3">
        <Composer />
      </div>
    </div>
  );
}

/** A sample dialog holding one of each form control. */
export function MockDialog({ open }) {
  // Non-modal: a modal dialog traps focus and pulls it back in whenever its
  // content changes, which would take the keyboard away from the host page.
  // Radix draws no overlay for a non-modal dialog, so the scrim is drawn here
  // with the real overlay's classes.
  return (
    <Dialog open={open} modal={false}>
      {open && (
        <div
          data-slot="dialog-overlay"
          className="fixed inset-0 isolate z-50 bg-scrim/10 supports-backdrop-filter:backdrop-blur-xs"
        />
      )}
      <DialogContent className="sm:max-w-md" {...PINNED}>
        <DialogHeader>
          <DialogTitle>Project settings</DialogTitle>
          <DialogDescription>These apply to moonlight only.</DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="general">
          <TabsList>
            <TabsTrigger value="general">General</TabsTrigger>
            <TabsTrigger value="agent">Agent</TabsTrigger>
            <TabsTrigger value="git">Git</TabsTrigger>
          </TabsList>
        </Tabs>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="mock-name">Name</Label>
            <Input id="mock-name" defaultValue="moonlight" />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="mock-cmd">Start command</Label>
            <Input id="mock-cmd" placeholder="cargo run" />
          </div>
          <div className="flex items-center justify-between">
            <Label htmlFor="mock-save">Format on save</Label>
            <Switch id="mock-save" defaultChecked />
          </div>
          <div className="flex items-center justify-between">
            <Label htmlFor="mock-watch">Watch for changes</Label>
            <Switch id="mock-watch" />
          </div>
          <label className="flex items-center gap-2 text-sm"><Checkbox defaultChecked /> Let the agent run commands</label>
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge>Rust</Badge>
            <Badge variant="secondary">TypeScript</Badge>
            <Badge variant="outline">2 warnings</Badge>
            <Badge variant="destructive">1 error</Badge>
          </div>
        </div>
        <DialogFooter>
          <Button variant="destructive">Remove project</Button>
          <Button variant="outline">Cancel</Button>
          <Button>Save changes</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
