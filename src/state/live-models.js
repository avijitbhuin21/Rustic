import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

// Lazy cache of live `/v1/models` results, keyed by a stable provider key.
// The backend (`fetch_ai_models`) already caches for 5 minutes; this store
// adds a frontend-side cache so we don't pay the IPC + cache hit on every
// re-render of the model popover.
//
// Provider key conventions match `get_ai_config` provider_type strings:
//   'Claude' | 'OpenAi' | 'Gemini' | 'OpenRouter' | 'Compatible'
// For Compatible providers we suffix with the instance name so multiple
// configured endpoints don't collide: 'Compatible:my-vllm'.

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const useLiveModels = create((set, get) => ({
  byKey: {},
  loadingByKey: {},
  errorByKey: {},

  // Fire-and-forget loader. Resolves to the model list either way (empty on
  // error) so callers can `await` without try/catching every call site.
  async load({ key, providerType, baseUrl = null, force = false }) {
    if (!key || !providerType) return [];
    const state = get();
    if (state.loadingByKey[key]) return state.byKey[key] || [];
    if (!force && state.byKey[key]) return state.byKey[key];
    if (!isTauri()) {
      set((s) => ({
        byKey: { ...s.byKey, [key]: [] },
        loadingByKey: { ...s.loadingByKey, [key]: false },
      }));
      return [];
    }

    set((s) => ({
      loadingByKey: { ...s.loadingByKey, [key]: true },
      errorByKey: { ...s.errorByKey, [key]: null },
    }));

    try {
      const list = await invoke('fetch_ai_models', {
        providerType,
        apiKey: '__STORED__',
        baseUrl: baseUrl || null,
        forceRefresh: !!force,
        includeAll: false,
      });
      const arr = Array.isArray(list) ? list : [];
      set((s) => ({
        byKey: { ...s.byKey, [key]: arr },
        loadingByKey: { ...s.loadingByKey, [key]: false },
      }));
      return arr;
    } catch (e) {
      set((s) => ({
        loadingByKey: { ...s.loadingByKey, [key]: false },
        errorByKey: { ...s.errorByKey, [key]: String(e) },
      }));
      return [];
    }
  },

  reset(key) {
    if (!key) return;
    set((s) => {
      const byKey = { ...s.byKey };
      const errorByKey = { ...s.errorByKey };
      delete byKey[key];
      delete errorByKey[key];
      return { byKey, errorByKey };
    });
  },
}));
