import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { SettingsSection, SettingRow } from './setting-row';

/**
 * Web-only Settings section for the idle auto-logout behaviour that pairs with
 * the status-bar power button. "Keep alive" disables the idle timer entirely;
 * otherwise the session powers off (flushes every background process and logs
 * out) after the configured minutes of inactivity. Persists server-side via
 * `set_power_config` and broadcasts the change so the live idle timer re-arms
 * without a reload.
 */
export function PowerSettings() {
  const [keepAlive, setKeepAlive] = useState(false);
  const [idleMinutes, setIdleMinutes] = useState(10);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState(null); // { ok, msg }

  useEffect(() => {
    invoke('get_power_config')
      .then((cfg) => {
        setKeepAlive(!!cfg?.keepAlive);
        setIdleMinutes(cfg?.idleTimeoutMinutes || 10);
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  const save = async (next) => {
    setSaving(true);
    setStatus(null);
    try {
      const res = await invoke('set_power_config', {
        keepAlive: next.keepAlive,
        idleTimeoutMinutes: next.idleMinutes,
      });
      const applied = {
        keepAlive: !!res?.keepAlive,
        idleTimeoutMinutes: res?.idleTimeoutMinutes || 10,
      };
      setKeepAlive(applied.keepAlive);
      setIdleMinutes(applied.idleTimeoutMinutes);
      // Re-arm the live idle timer in the status bar without a reload.
      window.dispatchEvent(
        new CustomEvent('rustic:power-config-changed', { detail: applied }),
      );
      setStatus({ ok: true, msg: 'Saved.' });
    } catch (e) {
      setStatus({ ok: false, msg: String(e?.message || e) });
    } finally {
      setSaving(false);
    }
  };

  if (loading) return null;

  return (
    <SettingsSection title="Session & Power">
      <SettingRow
        label="Keep session alive"
        description="Stay logged in indefinitely. When off, the session automatically powers off after a period of inactivity (below)."
        htmlFor="power-keep-alive"
      >
        <Switch
          id="power-keep-alive"
          checked={keepAlive}
          disabled={saving}
          onCheckedChange={(v) => save({ keepAlive: v, idleMinutes })}
        />
      </SettingRow>

      <SettingRow
        label="Idle timeout"
        description="Minutes of inactivity before the session powers off — flushing all running terminals, dev servers, agents, the browser and tunnels, then logging you out. Your files are never touched. Ignored while 'Keep session alive' is on."
        htmlFor="power-idle-minutes"
      >
        <Input
          id="power-idle-minutes"
          type="number"
          min={1}
          max={1440}
          step={1}
          value={idleMinutes}
          disabled={keepAlive || saving}
          onChange={(e) => setIdleMinutes(Number(e.target.value))}
          onBlur={() => save({ keepAlive, idleMinutes: Math.max(1, idleMinutes || 1) })}
          className="h-7 w-24 text-xs"
        />
      </SettingRow>

      {status && (
        <SettingRow label="" description={status.msg}>
          <span />
        </SettingRow>
      )}
    </SettingsSection>
  );
}
