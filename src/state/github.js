import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

// Single source of truth for the user's GitHub auth state across the IDE
// (StatusBar GitHub item, SCM header button, future PR widgets, etc.).
//
// Authentication uses the same in-process token store the backend git
// commands already read: `git_set_token` writes to AppState + keychain,
// `git_get_token` reports presence, and `github_get_user` echoes the API
// `/user` response so we can display login + avatar.

export const useGithubAuth = create((set, get) => ({
  user: null,             // { login, avatar_url } | null
  hasToken: false,        // mirrors backend git_token presence
  loading: false,         // true while a sign-in / refresh is in-flight
  initialized: false,     // becomes true after the first init() call resolves
  dialogOpen: false,      // global dialog visibility

  // Device-flow scratch state — only meaningful while dialogOpen + on the
  // Browser tab. Cleared on close.
  device: null,           // { user_code, verification_uri, expires_in, interval, device_code } | null
  devicePolling: false,
  deviceError: null,

  openDialog: () => set({ dialogOpen: true }),
  closeDialog: () => set({
    dialogOpen: false,
    device: null,
    devicePolling: false,
    deviceError: null,
  }),

  // Bootstrap: ask the backend if we have a stored token; if so, resolve the
  // current user. Call once at app startup.
  async init() {
    if (get().initialized) return;
    set({ initialized: true });
    try {
      const has = await invoke('git_get_token');
      if (!has) {
        set({ hasToken: false, user: null });
        return;
      }
      set({ hasToken: true });
      const user = await invoke('github_get_user').catch(() => null);
      set({ user });
    } catch {
      set({ hasToken: false, user: null });
    }
  },

  async signInWithToken(token) {
    const trimmed = (token ?? '').trim();
    if (!trimmed) throw new Error('Token is empty');
    set({ loading: true });
    try {
      await invoke('git_set_token', { token: trimmed });
      const user = await invoke('github_get_user').catch(() => null);
      set({ hasToken: true, user });
      return user;
    } finally {
      set({ loading: false });
    }
  },

  // Kick off the device flow: request a user_code + device_code, then begin
  // polling. Resolves once the user authorizes in the browser (or rejects on
  // error / timeout). Caller is responsible for displaying device.user_code
  // and verification_uri to the user.
  async startDeviceFlow() {
    set({ loading: true, deviceError: null, device: null });
    try {
      const device = await invoke('github_device_code');
      set({ device, devicePolling: true });
      return device;
    } catch (err) {
      set({ deviceError: String(err) });
      throw err;
    } finally {
      set({ loading: false });
    }
  },

  // Poll until we get a token, the user cancels, or the device code expires.
  // Stops automatically if devicePolling flips to false (cancel button).
  async pollDeviceFlow() {
    const device = get().device;
    if (!device) return null;
    const intervalMs = Math.max(1, device.interval ?? 5) * 1000;
    const deadline = Date.now() + (device.expires_in ?? 600) * 1000;

    while (Date.now() < deadline) {
      if (!get().devicePolling) return null; // cancelled
      await new Promise((r) => setTimeout(r, intervalMs));
      if (!get().devicePolling) return null;
      try {
        const token = await invoke('github_poll_token', {
          deviceCode: device.device_code,
        });
        if (token) {
          // git_set_token requires a confirm dialog; github_poll_token already
          // persists the token directly, so just refresh the user here.
          const user = await invoke('github_get_user').catch(() => null);
          set({ hasToken: true, user, devicePolling: false, device: null });
          return user;
        }
      } catch (err) {
        const msg = String(err);
        // "authorization_pending" / "slow_down" are normal; everything else aborts.
        if (msg.includes('authorization_pending')) continue;
        if (msg.includes('slow_down')) {
          await new Promise((r) => setTimeout(r, intervalMs));
          continue;
        }
        set({ deviceError: msg, devicePolling: false });
        throw err;
      }
    }
    set({ deviceError: 'Device code expired. Try again.', devicePolling: false });
    return null;
  },

  cancelDeviceFlow() {
    set({ devicePolling: false, device: null, deviceError: null });
  },

  async signOut() {
    try {
      await invoke('git_set_token', { token: '' });
    } catch {
      // Backend treats empty token as clear — should never reject. Even if it
      // does, fall through and clear local state so the UI reflects intent.
    }
    set({ user: null, hasToken: false });
  },
}));
