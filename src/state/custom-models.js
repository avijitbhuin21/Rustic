// Frontend store of user-supplied model specs for models not present in the
// Rust `model_registry`. Keyed by the exact model id as returned by the
// provider's list-models API.
//
// Fields mirror `ModelSpec` in crates/rustic-agent/src/model_registry.rs plus
// optional cached-token costs (which the backend doesn't use for condensing
// but may display in the UI).

const KEY = 'rustic_custom_models';

export function loadCustomModels() {
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}');
  } catch {
    return {};
  }
}

function saveAll(all) {
  localStorage.setItem(KEY, JSON.stringify(all));
}

export function getCustomModel(modelId) {
  if (!modelId) return null;
  return loadCustomModels()[modelId] || null;
}

export function saveCustomModel(modelId, spec) {
  const all = loadCustomModels();
  all[modelId] = { ...spec, savedAt: Date.now() };
  saveAll(all);
}

export function removeCustomModel(modelId) {
  const all = loadCustomModels();
  if (!all[modelId]) return;
  delete all[modelId];
  saveAll(all);
}
