import { create } from 'zustand';

// Frontend store of user-supplied model specs for models not present in the
// Rust `model_registry::KNOWN_MODELS`. Keyed by the exact model id returned by
// the provider's list-models API.
//
// Persisted to localStorage so specs survive reloads. Kept in a zustand store
// (not just a localStorage helper) because the prompt-box model picker needs
// to react when the Register modal saves a new entry — without the store,
// the picker would stay on its stale snapshot until next mount.
//
// Field shape mirrors what the Rust ModelSpec uses (snake_case on backend,
// camelCase here) plus optional cached costs the UI reasons about.

const KEY = 'rustic_custom_models';

function load() {
  if (typeof window === 'undefined') return {};
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}');
  } catch {
    return {};
  }
}

function persist(map) {
  if (typeof window === 'undefined') return;
  try {
    localStorage.setItem(KEY, JSON.stringify(map));
  } catch {}
}

export const useCustomModels = create((set, get) => ({
  models: load(),

  // Pure getter — does NOT subscribe the caller to changes. Use the selector
  // form (`useCustomModels((s) => s.models[id])`) when you need reactivity.
  get(modelId) {
    if (!modelId) return null;
    return get().models[modelId] || null;
  },

  save(modelId, spec) {
    if (!modelId) return;
    set((s) => {
      const next = {
        ...s.models,
        [modelId]: { ...spec, savedAt: Date.now() },
      };
      persist(next);
      return { models: next };
    });
  },

  remove(modelId) {
    if (!modelId) return;
    set((s) => {
      if (!s.models[modelId]) return s;
      const next = { ...s.models };
      delete next[modelId];
      persist(next);
      return { models: next };
    });
  },
}));
