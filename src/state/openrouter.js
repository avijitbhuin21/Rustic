import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';

// OpenRouter-specific enrichment, kept separate from the generic live-models
// store because it's the one provider whose catalogue (pricing, context, output
// limits, capabilities) and per-provider stats (speed, TTFT, uptime) are
// machine-readable. Two concerns live here:
//
//   useOpenRouterSpecs     — the whole catalogue as a map id -> spec, used to
//       auto-register OpenRouter models (accurate cost/context, no manual
//       Register-modal entry).
//   useOpenRouterProviders — per-model provider stats for the view-only panel
//       under each OpenRouter model row.
//
// The backend caches both for ~5 minutes; these stores add a frontend cache so
// re-opening the popover doesn't re-pay the IPC round-trip.


export const useOpenRouterSpecs = create((set, get) => ({
  // id -> { name, context_window, max_output_tokens, input_cost_per_m,
  //         output_cost_per_m, cache_read_cost_per_m, cache_write_cost_per_m,
  //         supports_temperature, supports_reasoning_effort }
  byId: {},
  loading: false,
  loaded: false,
  error: null,

  // Fetch the full catalogue once. Idempotent: concurrent callers share the
  // in-flight request, and a successful load short-circuits future calls until
  // `force` is passed.
  async load({ force = false } = {}) {
    const state = get();
    if (state.loading) return state.byId;
    if (!force && state.loaded) return state.byId;
    if (!isTauri()) {
      set({ loaded: true, loading: false });
      return {};
    }

    set({ loading: true, error: null });
    try {
      const list = await invoke('fetch_openrouter_model_specs', {
        forceRefresh: !!force,
      });
      const byId = {};
      for (const m of Array.isArray(list) ? list : []) {
        if (m && m.id) byId[m.id] = m;
      }
      set({ byId, loading: false, loaded: true });
      return byId;
    } catch (e) {
      set({ loading: false, error: String(e) });
      return get().byId;
    }
  },
}));

// Per-model provider allow-list (lowercase slugs). Mirrors the backend
// `openrouter_provider_allowlist` in ai_config. Empty list for a model = no
// restriction (route across all providers). The panel reads/writes this; the
// send path reads the backend copy.
export const useOpenRouterAllowlist = create((set, get) => ({
  byModel: {}, // modelId -> string[] of provider slugs
  loaded: false,

  async load({ force = false } = {}) {
    if (get().loaded && !force) return get().byModel;
    if (!isTauri()) {
      set({ loaded: true });
      return {};
    }
    try {
      const map = await invoke('get_openrouter_provider_allowlist');
      set({ byModel: map || {}, loaded: true });
      return map || {};
    } catch (e) {
      set({ loaded: true });
      return get().byModel;
    }
  },

  // Persist the allow-list for a model. An empty array clears the restriction.
  // Optimistic: updates local state first, then writes through to the backend.
  async setForModel(modelId, slugs) {
    set((s) => ({ byModel: { ...s.byModel, [modelId]: slugs } }));
    if (!isTauri()) return;
    try {
      await invoke('set_openrouter_provider_allowlist', {
        modelId,
        providers: slugs,
      });
    } catch (e) {
      console.error('[openrouter] failed to persist provider allow-list', e);
    }
  },
}));

export const useOpenRouterProviders = create((set, get) => ({
  byModel: {}, // modelId -> OpenRouterProvider[]
  loadingByModel: {},
  errorByModel: {},

  // Lazy per-model loader. Resolves to the provider list either way so callers
  // can await without try/catch.
  async load({ modelId, force = false }) {
    if (!modelId) return [];
    const state = get();
    if (state.loadingByModel[modelId]) return state.byModel[modelId] || [];
    if (!force && state.byModel[modelId]) return state.byModel[modelId];
    if (!isTauri()) {
      set((s) => ({ byModel: { ...s.byModel, [modelId]: [] } }));
      return [];
    }

    set((s) => ({
      loadingByModel: { ...s.loadingByModel, [modelId]: true },
      errorByModel: { ...s.errorByModel, [modelId]: null },
    }));
    try {
      const list = await invoke('fetch_openrouter_providers', {
        modelId,
        forceRefresh: !!force,
      });
      const arr = Array.isArray(list) ? list : [];
      set((s) => ({
        byModel: { ...s.byModel, [modelId]: arr },
        loadingByModel: { ...s.loadingByModel, [modelId]: false },
      }));
      return arr;
    } catch (e) {
      set((s) => ({
        loadingByModel: { ...s.loadingByModel, [modelId]: false },
        errorByModel: { ...s.errorByModel, [modelId]: String(e) },
      }));
      return [];
    }
  },
}));
