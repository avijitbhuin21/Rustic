// Library sections: skills, workflows, rules (+ shared markdown dialogs).
// Split out of agent-settings.jsx (A4).
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight, ChevronDown, Plus, Eye, EyeOff, Pencil, Trash2, Info, RefreshCw,
  ClipboardEdit, X, Check, FileText, Copy, List, Loader2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem, SelectGroup, SelectLabel,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri } from './shared';

// ─── Skills ──────────────────────────────────────────────────────────────────

export function MarkdownEditDialog({ open, title, name: initialName, body: initialBody, onClose, onSave, allowRename = true }) {
  const [name, setName] = useState(initialName || '');
  const [body, setBody] = useState(initialBody || '');
  const [saving, setSaving] = useState(false);

  useEffect(() => { if (open) { setName(initialName || ''); setBody(initialBody || ''); } }, [open, initialName, initialBody]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">{title}</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          {allowRename && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Name</div>
              <Input value={name} onChange={(e) => setName(e.target.value)} className="h-8 text-xs" />
            </div>
          )}
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Body</div>
            <Textarea
              value={body}
              onChange={(e) => setBody(e.target.value)}
              className="min-h-[280px] font-mono text-[11px] resize-none"
              spellCheck={false}
            />
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" disabled={saving || !name.trim()} onClick={async () => {
            setSaving(true);
            try { await onSave({ name: name.trim(), body }); onClose(); }
            catch (e) { toast.error(String(e)); }
            finally { setSaving(false); }
          }}>
            {saving ? 'Saving…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function MarkdownEntryRow({ entry, onPreview, onEdit, onCopy, onDelete, badge }) {
  return (
    <div className="px-3 py-2.5 hover:bg-muted/30 transition-colors group">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="text-[13px] font-medium">{entry.name}</span>
            {badge}
          </div>
          {entry.description && (
            <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-2">{entry.description}</div>
          )}
        </div>
        <div className="flex items-center gap-1 opacity-60 group-hover:opacity-100">
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onPreview} title="Preview">
            <Eye className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onEdit} title="Edit">
            <Pencil className="size-3.5" />
          </Button>
          {onCopy && (
            <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onCopy} title="Copy name">
              <Copy className="size-3.5" />
            </Button>
          )}
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" onClick={onDelete} title="Delete">
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      </div>
    </div>
  );
}

export function PreviewDialog({ open, title, body, onClose }) {
  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">{title}</DialogTitle>
        </DialogHeader>
        <ScrollArea className="max-h-[60vh]">
          <pre className="whitespace-pre-wrap px-5 py-4 text-[12px] font-mono">{body || '(empty)'}</pre>
        </ScrollArea>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button size="sm" className="text-xs" onClick={onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SkillsSection() {
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null); // {name, body} or {name: '', body: ''} for new
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_skills');
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, []);

  const openPreview = async (name) => {
    try {
      const body = await invoke('get_skill_body', { name });
      setPreview({ title: name, body });
    } catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try {
      const body = await invoke('get_skill_body', { name });
      setEdit({ originalName: name, name, body });
    } catch (e) { toast.error(String(e)); }
  };

  const save = async ({ name, body }) => {
    if (edit?.originalName) {
      await invoke('update_skill', { originalName: edit.originalName, name, body });
    } else {
      await invoke('create_skill', { name, body });
    }
    refresh();
  };

  const remove = async (name) => {
    try { await invoke('delete_skill', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="Skills"
      badge="Global"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About skills">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add skill">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No skills installed.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((s) => (
            <MarkdownEntryRow
              key={s.name}
              entry={s}
              onPreview={() => openPreview(s.name)}
              onEdit={() => openEdit(s.name)}
              onCopy={() => { navigator.clipboard.writeText(s.name); toast.success('Copied'); }}
              onDelete={() => remove(s.name)}
            />
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New skill'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About skills</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Skills are reusable instructions the agent can opt into per-task — e.g. "follow brand guidelines", "use the canvas-design conventions".</p>
            <p>Stored as Markdown files under your global Rustic skills directory. The agent sees the title + description in its system prompt and can decide when to use them.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

// ─── Workflows ───────────────────────────────────────────────────────────────

export function WorkflowsSection() {
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null);
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_workflows');
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, []);

  const openPreview = async (name) => {
    try { const body = await invoke('get_workflow_body', { name }); setPreview({ title: name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try { const body = await invoke('get_workflow_body', { name }); setEdit({ originalName: name, name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const save = async ({ name, body }) => {
    if (edit?.originalName) await invoke('update_workflow', { originalName: edit.originalName, name, body });
    else await invoke('create_workflow', { name, body });
    refresh();
  };
  const remove = async (name) => {
    try { await invoke('delete_workflow', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="Workflows"
      badge="Global"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About workflows">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add workflow">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No workflows installed.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((w) => (
            <MarkdownEntryRow
              key={w.name}
              entry={w}
              onPreview={() => openPreview(w.name)}
              onEdit={() => openEdit(w.name)}
              onCopy={() => { navigator.clipboard.writeText(w.name); toast.success('Copied'); }}
              onDelete={() => remove(w.name)}
            />
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New workflow'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About workflows</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Workflows are multi-step procedures the agent invokes by name (e.g. "landing-page-cloning-workflow"). They expand into a recipe the agent then executes.</p>
            <p>Stored as Markdown files under your global Rustic workflows directory.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

// ─── Rules ───────────────────────────────────────────────────────────────────

// Forward-slash + lowercase-drive normalisation so we can match a
// `Project.root_path` (which may use backslashes on Windows) against the
// `active_projects` keys returned by the backend (`project_key` uses
// forward slashes).
export function normaliseProjectKey(p) {
  if (!p) return '';
  let s = String(p).replace(/\\/g, '/');
  // Lowercase the drive letter on Windows-style paths: `D:/foo` → `d:/foo`,
  // matching the canonical form the rule store uses.
  if (/^[A-Za-z]:\//.test(s)) {
    s = s[0].toLowerCase() + s.slice(1);
  }
  return s;
}

export function RuleStatePicker({ value, projectCount, onPick }) {
  const opts = [
    { v: 'inactive', label: 'Off' },
    { v: 'global',   label: 'G'   },
    {
      v: 'project',
      // Show count when the rule is project-scoped in 2+ places so the
      // user knows the picker isn't just "current project only".
      label: projectCount > 0 ? `P · ${projectCount}` : 'P',
    },
  ];
  return (
    <div className="inline-flex rounded-md border border-border/60 bg-muted/30 p-0.5">
      {opts.map((o) => (
        <button
          key={o.v}
          type="button"
          onClick={() => onPick(o.v)}
          className={cn(
            'h-6 px-2 text-[11px] font-medium rounded-sm transition-colors',
            value === o.v
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:text-foreground'
          )}
          title={o.v === 'project' ? 'Pick project(s) where this rule applies' : o.v}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function RuleProjectPickerDialog({ open, onClose, ruleName, initialSelected, onSaved }) {
  const [projects, setProjects] = useState([]);
  const [selected, setSelected] = useState(new Set());
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open || !isTauri()) return;
    invoke('list_projects')
      .then((list) => setProjects(Array.isArray(list) ? list : []))
      .catch(() => setProjects([]));
    // Pre-fill from the backend's stored active_projects (already normalised
    // to forward-slash). Match by normalised root path so the same project
    // matches regardless of slash direction.
    const init = new Set();
    (initialSelected || []).forEach((p) => init.add(normaliseProjectKey(p)));
    setSelected(init);
  }, [open, initialSelected]);

  const toggle = (rootKey) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(rootKey)) next.delete(rootKey);
      else next.add(rootKey);
      return next;
    });
  };

  const save = async () => {
    setSaving(true);
    try {
      // Send the original (un-normalised) root paths for the selected
      // projects — the backend re-normalises with its own `project_key`.
      const picked = projects
        .filter((p) => selected.has(normaliseProjectKey(p.root_path)))
        .map((p) => p.root_path);
      await invoke('set_rule_projects', { name: ruleName, projectRoots: picked });
      onSaved?.();
      onClose();
    } catch (e) { toast.error(String(e)); }
    finally { setSaving(false); }
  };

  // Show project keys we have on file but no longer correspond to a known
  // project (project was deleted but rule still references it). Surface
  // them as read-only rows with a small note so the user can clear them.
  const knownKeys = new Set(projects.map((p) => normaliseProjectKey(p.root_path)));
  const orphans = Array.from(selected).filter((k) => !knownKeys.has(k));

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[520px] sm:max-w-[520px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Projects for "{ruleName}"</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-2">
          <p className="text-[11px] italic text-muted-foreground leading-snug">
            Tick the projects where this rule should apply. Selecting more than one is fine. Saving with nothing
            ticked deactivates the rule everywhere.
          </p>
          <div className="rounded-md border border-border/40 divide-y divide-border/40 max-h-72 overflow-y-auto">
            {projects.length === 0 ? (
              <div className="px-3 py-3 text-[11px] text-muted-foreground">No projects in your workspace yet.</div>
            ) : projects.map((p) => {
              const key = normaliseProjectKey(p.root_path);
              const checked = selected.has(key);
              return (
                <label
                  key={p.id}
                  className="flex cursor-pointer items-center gap-2.5 px-3 py-2 hover:bg-muted/40"
                  onClick={(e) => { e.preventDefault(); toggle(key); }}
                >
                  <div className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors',
                    checked
                      ? 'border-primary bg-primary text-primary-foreground'
                      : 'border-border bg-transparent'
                  )}>
                    {checked && <Check className="size-3" strokeWidth={3} />}
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-[12.5px] font-medium truncate">{p.name}</div>
                    <div className="text-[11px] text-muted-foreground truncate font-mono">{p.root_path}</div>
                  </div>
                </label>
              );
            })}
            {orphans.map((k) => (
              <div key={k} className="flex items-center gap-2.5 px-3 py-2 bg-muted/10">
                <div className="size-4 shrink-0 rounded-sm border border-rose-500/40 bg-rose-500/10" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] text-rose-500/90 font-mono truncate">{k}</div>
                  <div className="text-[10.5px] text-muted-foreground">Project no longer exists — will be cleared on save.</div>
                </div>
                <Button
                  size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive"
                  onClick={() => setSelected((prev) => {
                    const next = new Set(prev);
                    next.delete(k);
                    return next;
                  })}
                >
                  <X className="size-3.5" />
                </Button>
              </div>
            ))}
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving}>
            {saving ? 'Saving…' : `Apply to ${selected.size} project${selected.size === 1 ? '' : 's'}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function RulesSection() {
  const projectRoot = useAgent((s) => s.activeProject.root || null);
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null);
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);
  // Rule currently being edited in the project-picker dialog.
  const [projectPicker, setProjectPicker] = useState(null);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_rules', { projectRoot: projectRoot || null });
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, [projectRoot]);

  const openPreview = async (name) => {
    try { const body = await invoke('get_rule_body', { name }); setPreview({ title: name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try { const body = await invoke('get_rule_body', { name }); setEdit({ originalName: name, name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const save = async ({ name, body }) => {
    if (edit?.originalName) await invoke('update_rule', { originalName: edit.originalName, name, body });
    else await invoke('create_rule', { name, body });
    refresh();
  };
  const remove = async (name) => {
    try { await invoke('delete_rule', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };
  // Off / Global flip directly; Project pops the multi-select dialog so the
  // user can choose which projects this rule applies to.
  const onPickState = (rule, next) => {
    if (next === 'project') {
      setProjectPicker({ name: rule.name, initial: rule.active_projects || [] });
      return;
    }
    invoke('set_rule_activation', { name: rule.name, state: next, projectRoot: projectRoot || null })
      .then(refresh)
      .catch((e) => toast.error(String(e)));
  };

  return (
    <Section
      title="Rules"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About rules">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add rule">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No rules.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((r) => (
            <div key={r.name} className="px-3 py-2.5 hover:bg-muted/30 group">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <span className="text-[13px] font-medium">{r.name}</span>
                    {r.state === 'global'  && <Badge variant="outline" className="h-5 text-[10px] uppercase border-primary/50 text-primary">Global</Badge>}
                    {r.state === 'project' && <Badge variant="outline" className="h-5 text-[10px] uppercase border-amber-500/50 text-amber-500">Project</Badge>}
                  </div>
                  {r.description && (
                    <div className="mt-0.5 text-[11px] italic text-muted-foreground line-clamp-2">{r.description}</div>
                  )}
                </div>
                <div className="flex items-center gap-2 opacity-80 group-hover:opacity-100">
                  <RuleStatePicker
                    value={r.state}
                    projectCount={(r.active_projects || []).length}
                    onPick={(v) => onPickState(r, v)}
                  />
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={() => openPreview(r.name)} title="Preview">
                    <Eye className="size-3.5" />
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={() => openEdit(r.name)} title="Edit">
                    <Pencil className="size-3.5" />
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" onClick={() => remove(r.name)} title="Delete">
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New rule'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <RuleProjectPickerDialog
        open={!!projectPicker}
        ruleName={projectPicker?.name || ''}
        initialSelected={projectPicker?.initial || []}
        onClose={() => setProjectPicker(null)}
        onSaved={refresh}
      />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About rules</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Rules are always-on instructions the agent honors during a chat — e.g. "no unnecessary comments", "always run tests after edits".</p>
            <p><span className="font-medium">Off</span> = not active. <span className="font-medium">G</span> = global, active in every project. <span className="font-medium">P</span> = active only in the current project.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

