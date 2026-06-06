import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';

// Live Cloudflare quick-tunnel registry (web build only). The server's port
// monitor auto-exposes new dev servers and reaps tunnels whose port dies; it
// announces both over the `/ws` hub (`tunnel-opened` / `tunnel-closed`). This
// store mirrors that so the Tunnels panel always shows the current public URLs
// — the consistent place to find a URL again after you've lost it.

let wired = false;

export const useTunnels = create((set, get) => ({
  // [{ port: number, url: string }]
  tunnels: [],

  wire: async () => {
    if (wired) return;
    wired = true;
    await listen('tunnel-opened', (e) => {
      const { port, url } = e.payload || {};
      if (!port || !url) return;
      set((s) => {
        const rest = s.tunnels.filter((t) => t.port !== port);
        return { tunnels: [...rest, { port, url }].sort((a, b) => a.port - b.port) };
      });
      toast.success(`Port ${port} is public`, {
        description: url,
        action: { label: 'Copy', onClick: () => navigator.clipboard?.writeText(url) },
        duration: 8000,
      });
    });
    await listen('tunnel-closed', (e) => {
      const { port } = e.payload || {};
      if (!port) return;
      set((s) => ({ tunnels: s.tunnels.filter((t) => t.port !== port) }));
    });
    get().refresh();
  },

  refresh: async () => {
    try {
      const list = await invoke('tunnel_list');
      const tunnels = (list ?? [])
        .filter((t) => t?.port && t?.url)
        .map((t) => ({ port: t.port, url: t.url }))
        .sort((a, b) => a.port - b.port);
      set({ tunnels });
    } catch {
      /* transient; the next event re-syncs */
    }
  },

  // Manually open a tunnel for a port (used by the "expose" affordance).
  open: async (port) => {
    const toastId = toast.loading(`Exposing port ${port}…`);
    try {
      const res = await invoke('tunnel_open', { port: Number(port) });
      if (res?.url) {
        toast.success(`Port ${port} is public`, { id: toastId, description: res.url });
        get().refresh();
      } else {
        toast.error('Tunnel did not return a URL', { id: toastId });
      }
    } catch (e) {
      toast.error(`Tunnel failed: ${e?.message || e}`, { id: toastId });
    }
  },

  // Manually close a tunnel.
  close: async (port) => {
    set((s) => ({ tunnels: s.tunnels.filter((t) => t.port !== port) }));
    try {
      await invoke('tunnel_close', { port: Number(port) });
    } catch (e) {
      console.error('[tunnels] tunnel_close failed', e);
      get().refresh();
    }
  },
}));
