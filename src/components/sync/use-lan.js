// Shared LAN state for the sync overlay: status, devices (polled while
// mounted), pairing with cancel, and refresh on network / peer events.
import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export const errText = (e) => String(e?.message || e);

/** Status + devices + pairing for the overlay. */
export function useLan() {
  const [status, setStatus] = useState(null);
  const [devices, setDevices] = useState([]);
  const [busy, setBusy] = useState(false);
  const [pairing, setPairing] = useState(null);

  const refreshStatus = useCallback(() => invoke('lan_status').then(setStatus).catch(() => {}), []);
  const refreshDevices = useCallback(
    () => invoke('lan_devices').then((l) => setDevices(Array.isArray(l) ? l : [])).catch(() => {}),
    [],
  );
  const enabled = !!status?.enabled;

  useEffect(() => { refreshStatus(); }, [refreshStatus]);
  useEffect(() => {
    if (!enabled) { setDevices([]); return undefined; }
    refreshDevices();
    const t = setInterval(refreshDevices, 3000);
    return () => clearInterval(t);
  }, [enabled, refreshDevices]);
  useEffect(() => {
    const unlistens = [];
    let disposed = false;
    import('@tauri-apps/api/event').then(async ({ listen }) => {
      for (const ev of ['lan-status-changed', 'lan-peers-changed', 'lan-peer-unpaired']) {
        const fn = await listen(ev, () => { refreshStatus(); refreshDevices(); });
        if (disposed) fn(); else unlistens.push(fn);
      }
    }).catch(() => {});
    // The OS reports Wi-Fi drops / switches here instantly; re-check now and
    // again once the new network has had a moment to come up.
    const onNet = () => {
      refreshStatus(); refreshDevices();
      setTimeout(() => { invoke('lan_announce').catch(() => {}); refreshStatus(); refreshDevices(); }, 2500);
    };
    window.addEventListener('online', onNet);
    window.addEventListener('offline', onNet);
    return () => {
      disposed = true;
      unlistens.forEach((f) => f());
      window.removeEventListener('online', onNet);
      window.removeEventListener('offline', onNet);
    };
  }, [refreshStatus, refreshDevices]);

  const toggle = async (v) => {
    setBusy(true);
    try { await invoke('lan_set_enabled', { enabled: v }); await refreshStatus(); }
    catch (e) { toast.error(errText(e)); }
    finally { setBusy(false); }
  };
  const rename = async (name) => {
    try { await invoke('lan_set_device_name', { name }); await refreshStatus(); toast.success('Name saved'); }
    catch (e) { toast.error(errText(e)); }
  };

  /** Pair (or re-pair) with a device; resolves true when paired. */
  const pair = async (d) => {
    try {
      const code = await invoke('lan_pair_code', { deviceId: d.device_id });
      setPairing({ deviceId: d.device_id, name: d.name, code });
      const name = await invoke('lan_pair', { deviceId: d.device_id });
      toast.success(`Paired with ${name}. Choose what to share under Sharing.`);
      await refreshDevices();
      return true;
    } catch (e) {
      const msg = errText(e);
      if (msg !== 'Pairing cancelled') toast.error(msg, { duration: 9000 });
      return false;
    } finally {
      setPairing(null);
    }
  };
  const cancelPairing = () => {
    if (pairing) invoke('lan_cancel_outgoing', { deviceId: pairing.deviceId }).catch(() => {});
  };

  return {
    status, devices, enabled, busy, pairing,
    refreshStatus, refreshDevices, toggle, rename, pair, cancelPairing,
  };
}
