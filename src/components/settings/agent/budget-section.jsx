// Budget / spend ceiling section.
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
import { InfoTip } from '@/components/ui/info-tip';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem, SelectGroup, SelectLabel,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { toast } from 'sonner';
import { toastError } from '@/lib/toast-error';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri } from './shared';
import { RowGroup } from '../setting-row';

// ─── Budget ──────────────────────────────────────────────────────────────────

export function BudgetSection() {
  const [streamsEnabled, setStreamsEnabled] = useState(true);
  const [streams, setStreams] = useState(6);
  const [ceilingEnabled, setCeilingEnabled] = useState(false);
  const [ceilingUsd, setCeilingUsd] = useState(20);
  const [softTurnEnabled, setSoftTurnEnabled] = useState(true);
  const [softTurnLimit, setSoftTurnLimit] = useState(50);
  const [condenseEnabled, setCondenseEnabled] = useState(false);
  const [condenseK, setCondenseK] = useState(400);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const cfg = await invoke('get_ai_config');
      const th = Number(cfg?.condense_threshold) || 0;
      setCondenseEnabled(th > 0);
      if (th > 0) setCondenseK(Math.round(th / 1000));
    } catch {}
    try {
      const b = await invoke('get_budget_settings');
      if (b.max_concurrent_streams === null || b.max_concurrent_streams === undefined) {
        setStreamsEnabled(false);
      } else {
        setStreamsEnabled(true);
        setStreams(b.max_concurrent_streams);
      }
      if (b.daily_cost_ceiling_cents === null || b.daily_cost_ceiling_cents === undefined) {
        setCeilingEnabled(false);
      } else {
        setCeilingEnabled(true);
        setCeilingUsd(Math.round(b.daily_cost_ceiling_cents / 100));
      }
      if (b.soft_turn_limit === null || b.soft_turn_limit === undefined) {
        setSoftTurnEnabled(false);
      } else {
        setSoftTurnEnabled(true);
        setSoftTurnLimit(b.soft_turn_limit);
      }
    } catch {}
  };
  useEffect(() => { refresh(); }, []);

  const save = async () => {
    try {
      await invoke('set_budget_settings', {
        maxConcurrentStreams: streamsEnabled ? Number(streams) : null,
        dailyCostCeilingCents: ceilingEnabled ? Math.max(0, Math.round(Number(ceilingUsd) * 100)) : null,
        softTurnLimit: softTurnEnabled ? Math.max(1, Number(softTurnLimit) || 50) : null,
      });
      await invoke('set_condense_threshold', {
        tokens: condenseEnabled ? Math.max(10, Number(condenseK) || 0) * 1000 : 0,
      });
      toast.success('Budget saved');
    } catch (e) { toastError(null, e); }
  };

  return (
    <Section title="Budget" info="Cross-task limits. Stop runaway parallelism or spend before it bites.">

      <RowGroup inset>
        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              Cap concurrent provider streams
              <InfoTip>
                Parallel API calls across every task and their sub-agents. Default 6. Raise only if your provider's rate
                limit can handle it.
              </InfoTip>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={streamsEnabled} onCheckedChange={setStreamsEnabled} />
            <Input
              type="number" min={1} max={64} value={streams}
              onChange={(e) => setStreams(parseInt(e.target.value, 10) || 1)}
              disabled={!streamsEnabled}
              className="h-7 w-16 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">streams</span>
          </div>
        </div>

        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              Daily cost ceiling (native API)
              <InfoTip>Stops new turns when today's native-API spend hits the cap. Resets at midnight UTC.</InfoTip>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={ceilingEnabled} onCheckedChange={setCeilingEnabled} />
            <Input
              type="number" min={0} value={ceilingUsd}
              onChange={(e) => setCeilingUsd(parseFloat(e.target.value) || 0)}
              disabled={!ceilingEnabled}
              className="h-7 w-20 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">usd/day</span>
          </div>
        </div>

        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              Soft turn ceiling
              <InfoTip>
                After this many model calls in one continuous run, the agent is nudged to wrap up and
                check in with you (re-nudged every 25 after). A runaway-loop guard — a nudge, not a hard stop.
              </InfoTip>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={softTurnEnabled} onCheckedChange={setSoftTurnEnabled} />
            <Input
              type="number" min={1} max={500} value={softTurnLimit}
              onChange={(e) => setSoftTurnLimit(parseInt(e.target.value, 10) || 50)}
              disabled={!softTurnEnabled}
              className="h-7 w-16 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">calls</span>
          </div>
        </div>

        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              Auto-condense at
              <InfoTip>
                Off: history is condensed at ~93% of the model's context window (e.g. ~900k on a 1M model).
                On: condense once a chat reaches this many tokens instead — lower values keep long sessions cheaper.
                Never exceeds the model's own window. Applies to new turns.
              </InfoTip>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={condenseEnabled} onCheckedChange={setCondenseEnabled} />
            <Input
              type="number" min={10} max={10000} value={condenseK}
              onChange={(e) => setCondenseK(parseInt(e.target.value, 10) || 10)}
              disabled={!condenseEnabled}
              className="h-7 w-20 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">k tokens</span>
          </div>
        </div>
      </RowGroup>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save budget settings</Button>
      </div>
    </Section>
  );
}

