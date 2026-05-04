import { el, icon } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import * as api from '../../lib/tauri-api.js';
import { showConfirmDialog } from '../confirm-dialog.js';

const SINGLETON_PROVIDERS = [
  { id: 'Claude',  label: 'Anthropic',       placeholder: 'sk-ant-…' },
  { id: 'OpenAi',  label: 'OpenAI',          placeholder: 'sk-…'     },
  { id: 'Gemini',  label: 'Google Gemini',   placeholder: 'AIza…'    },
];

const COMPATIBLE_TYPE = 'Compatible';

const MODEL_MAX_OUTPUT = {
  // Anthropic (Claude)
  'claude-opus-4-7':    128000,
  'claude-opus-4-6':    128000, 'claude-opus-4':    128000,
  'claude-sonnet-4-6':   64000, 'claude-sonnet-4':   64000, 'claude-sonnet-4-5': 64000,
  'claude-haiku-4-5':    64000,
  // Claude Code subscription harness aliases — same caps as the API
  // models they front (`opus` → opus-4-7, `sonnet` → sonnet-4-6, `haiku` →
  // haiku-4-5). Listed separately so the frontend's lookups (UI badges,
  // budget hints) work on the bare alias the CLI uses.
  'opus':    128000,
  'sonnet':   64000,
  'haiku':    64000,
  // OpenAI — GPT-5.5 family
  'gpt-5.5-pro': 128000, 'gpt-5.5': 128000,
  // OpenAI — GPT-5.4 family
  'gpt-5.4-pro': 128000, 'gpt-5.4': 128000, 'gpt-5.4-mini': 128000, 'gpt-5.4-nano': 128000,
  // OpenAI — Codex
  'gpt-5.3-codex': 128000, 'gpt-5-codex': 128000,
  // OpenAI — Reasoning
  'o4-mini': 100000, 'o3': 100000, 'o3-mini': 100000,
  // OpenAI — Legacy
  'gpt-4.1': 32768, 'gpt-4.1-mini': 32768, 'gpt-4.1-nano': 32768,
  'gpt-4o': 16384, 'gpt-4o-mini': 16384,
  // Gemini — 3.x (current)
  'gemini-3.1-pro': 65536, 'gemini-3.1-flash-lite': 65536, 'gemini-3-flash': 65536,
  // Gemini — 2.x
  'gemini-2.5-pro': 65536, 'gemini-2.5-flash': 65536, 'gemini-2.5-flash-lite': 65536,
  'gemini-2.0-flash': 8192,
};

// Pricing entries are `{ input, output, cachedInput?, cachedOutput? }` —
// per 1M tokens. Anthropic cache reads are 0.1× the input rate; output has
// no cached tier so cachedOutput is omitted (treated as 0 downstream).
const MODEL_PRICING = {
  // Claude — API
  'claude-opus-4':      { input: 5.0, output: 25.0, cachedInput: 0.50 },
  'claude-sonnet-4':    { input: 3.0, output: 15.0, cachedInput: 0.30 },
  'claude-haiku-4':     { input: 1.0, output:  5.0, cachedInput: 0.10 },
  // Claude Code subscription harness aliases — billing happens against the
  // user's subscription not by token, so these numbers are display-only
  // (cost pill, not-configured detection). Mirror the underlying API
  // model that each alias points at: `opus` → opus-4-7, `sonnet` →
  // sonnet-4-6, `haiku` → haiku-4-5.
  'opus':               { input: 5.0, output: 25.0, cachedInput: 0.50 },
  'sonnet':             { input: 3.0, output: 15.0, cachedInput: 0.30 },
  'haiku':              { input: 1.0, output:  5.0, cachedInput: 0.10 },
  // OpenAI — GPT-5.5 family. Pro tier MUST come before the bare family
  // name so `gpt-5.5-pro-2026-04-23` matches Pro pricing instead of falling
  // through to the cheaper base via prefix-matching. Standard `gpt-5.5`
  // also has a long-context tier (>272K input → 2x input / 1.5x output for
  // the whole session) — we publish the under-tier numbers since most
  // sessions stay under that threshold; if you hit the tier the actual
  // bill from OpenAI will run higher than the pill in the UI.
  'gpt-5.5-pro':        { input: 30.0,  output: 180.0 },
  'gpt-5.5':            { input:  5.0,  output:  30.0 },
  // OpenAI — GPT-5.4
  'gpt-5.4-pro':        { input: 30.0,  output: 180.0 },
  'gpt-5.4-mini':       { input: 0.75,  output: 4.50  },
  'gpt-5.4-nano':       { input: 0.20,  output: 1.25  },
  'gpt-5.4':            { input: 2.50,  output: 15.0  },
  // OpenAI — Codex
  'gpt-5.3-codex':      { input: 1.75,  output: 14.0  },
  'gpt-5-codex':        { input: 1.25,  output: 10.0  },
  // OpenAI — Reasoning
  'o4-mini':            { input: 1.10,  output: 4.40  },
  'o3':                 { input: 2.0,   output: 8.0   },
  'o3-mini':            { input: 1.10,  output: 4.40  },
  // OpenAI — Legacy
  'gpt-4.1':            { input: 2.0,   output: 8.0   },
  'gpt-4.1-mini':       { input: 0.40,  output: 1.60  },
  'gpt-4.1-nano':       { input: 0.10,  output: 0.40  },
  'gpt-4o':             { input: 2.50,  output: 10.0  },
  'gpt-4o-mini':        { input: 0.15,  output: 0.60  },
  // Gemini — 3.x
  'gemini-3.1-pro':     { input: 2.0,   output: 12.0  },
  'gemini-3.1-flash-lite': { input: 0.25, output: 1.50 },
  'gemini-3-flash':     { input: 0.50,  output: 3.0   },
  // Gemini — 2.x
  'gemini-2.5-pro':     { input: 1.25,  output: 10.0  },
  'gemini-2.5-flash':   { input: 0.15,  output: 0.60  },
  'gemini-2.5-flash-lite': { input: 0.10, output: 0.40 },
  'gemini-2.0-flash':   { input: 0.10,  output: 0.40  },
};

export function pricingFor(modelId) {
  if (MODEL_PRICING[modelId]) return MODEL_PRICING[modelId];
  for (const [k, v] of Object.entries(MODEL_PRICING)) { if (modelId.startsWith(k)) return v; }
  return null;
}

// Context-window registry — only listed for models whose default in the
// Rust backend is wrong or missing. The frontend pushes the value below to
// `setAiProvider` so condensing/budgeting use the correct ceiling. Same
// most-specific-prefix-first ordering as MODEL_PRICING so
// `gpt-5.5-pro-2026-04-23` resolves to the Pro window before the bare
// family name.
const MODEL_CONTEXT_WINDOW = {
  // OpenAI — GPT-5.5  (1M-token context tier; Pro is technically 1.05M
  // but the backend's budget calc uses a single ceiling, so we pin both
  // to a conservative 1,000,000 to keep the math consistent with the
  // standard model.)
  'gpt-5.5-pro': 1_000_000,
  'gpt-5.5':     1_000_000,
  // Claude Code subscription harness aliases — Anthropic ships the
  // subscription with an extended 1M-token context for Sonnet and Opus;
  // Haiku stays on the standard 200K tier. Without these, condensing
  // would clip turns long before the CLI's actual ceiling.
  'opus':      1_000_000,
  'sonnet':    1_000_000,
  'haiku':       200_000,
};

export function contextWindowFor(modelId) {
  if (!modelId) return 0;
  if (MODEL_CONTEXT_WINDOW[modelId]) return MODEL_CONTEXT_WINDOW[modelId];
  for (const [k, v] of Object.entries(MODEL_CONTEXT_WINDOW)) {
    if (modelId.startsWith(k)) return v;
  }
  return 0;
}

// Migrate legacy single-Compatible entry to the new keyed shape on first load.
function migrateConfigs(configs) {
  if (configs.Compatible && !configs.Compatible.name) {
    const legacy = configs.Compatible;
    delete configs.Compatible;
    configs['Compatible:default'] = { ...legacy, name: 'Default' };
    localStorage.setItem('rustic_provider_configs', JSON.stringify(configs));
  }
  return configs;
}

// Scrub any plaintext `apiKey` field still present in localStorage from the
// pre-keychain era. The backend already moved real keys into the OS keychain
// at startup; the only thing left to do here is replace `apiKey` strings with
// a `hasKey: true` boolean. Mutates the input and persists.
function stripPlaintextKeys(configs) {
  let mutated = false;
  for (const k of Object.keys(configs)) {
    const cfg = configs[k];
    if (cfg && typeof cfg.apiKey === 'string' && cfg.apiKey.length > 0 && cfg.apiKey !== '__STORED__') {
      cfg.hasKey = true;
      delete cfg.apiKey;
      mutated = true;
    } else if (cfg && cfg.apiKey === '__STORED__') {
      cfg.hasKey = true;
      delete cfg.apiKey;
      mutated = true;
    } else if (cfg && cfg.hasKey === undefined) {
      // Older shape without either field — assume not configured.
      cfg.hasKey = false;
      mutated = true;
    }
  }
  if (mutated) {
    localStorage.setItem('rustic_provider_configs', JSON.stringify(configs));
  }
  return configs;
}

export function loadProviderConfigs() {
  try {
    const raw = JSON.parse(localStorage.getItem('rustic_provider_configs') || '{}');
    return stripPlaintextKeys(migrateConfigs(raw));
  } catch {
    return {};
  }
}

export function saveProviderConfigs(configs) {
  // Defensive: strip any `apiKey` field a caller forgot to remove. The real
  // key lives in the OS keychain; localStorage only carries the `hasKey`
  // boolean for UI state.
  const sanitized = {};
  for (const k of Object.keys(configs)) {
    const cfg = configs[k];
    if (!cfg || typeof cfg !== 'object') {
      sanitized[k] = cfg;
      continue;
    }
    const { apiKey: _apiKey, ...rest } = cfg;
    sanitized[k] = rest;
  }
  localStorage.setItem('rustic_provider_configs', JSON.stringify(sanitized));
  // Notify the chat view (and anyone else watching) that provider config
  // changed so the Send button's enabled-state and the welcome CTA stay in
  // sync without polling.
  try {
    window.dispatchEvent(new CustomEvent('rustic:provider-configs-changed'));
  } catch {}
}

/// Returns true if at least one provider is connected (has a saved key AND
/// at least one model). Used by the chat view to decide whether the Send
/// button should be enabled and whether to show a "Connect a provider" CTA.
export function hasAnyConnectedProvider() {
  const configs = loadProviderConfigs();
  return Object.values(configs).some((c) => c?.hasKey && Array.isArray(c.models) && c.models.length > 0);
}

/// Restore the frontend `rustic_provider_configs` localStorage entry from the
/// backend's persisted `ai_config` (SQLite) on app boot.
///
/// **Why this exists**
/// API keys live in the OS keychain and provider metadata lives in SQLite —
/// both survive a rebuild. But the UI's "is this provider connected?" flag
/// (`hasKey: true`) lives in the WebView's localStorage, which Tauri 2's
/// WebView profile sometimes wipes between dev rebuilds. When that happens,
/// SQLite + keychain still hold the real config, but the AI Settings panel
/// shows everything as "Not connected" and the Send button is disabled —
/// users had to re-enter their key on every rebuild even though the key
/// itself was still there.
///
/// This function syncs localStorage to whatever the backend says is
/// configured, so a wiped localStorage gets repopulated transparently.
/// Should be awaited *once* at app boot before any UI reads
/// `loadProviderConfigs()`.
export async function hydrateProviderConfigsFromBackend() {
  let backendConfig;
  try {
    backendConfig = await api.getAiConfig();
  } catch (e) {
    console.warn('[hydrate] getAiConfig failed:', e);
    return;
  }
  if (!backendConfig?.providers?.length) return;

  const local = loadProviderConfigs();
  let mutated = false;

  for (const entry of backendConfig.providers) {
    const providerType = String(entry.provider_type);
    const isCompatible = providerType === COMPATIBLE_TYPE;
    // Singletons (Claude/OpenAi/Gemini/ClaudeCode/Codex) use provider_type
    // as the storage key; Compatible providers use `Compatible:<slug>` so
    // multiple instances stay distinct. The slug must match the one
    // `slugify_name` produces in `crates/rustic-agent/src/config.rs`.
    let storageKey;
    if (isCompatible && entry.name) {
      storageKey = compatibleKey(slugify(entry.name));
    } else {
      storageKey = providerType;
    }

    // The backend redacts api_key to "__STORED__" when a real key is set,
    // and leaves it empty when the keychain doesn't have one. Either way
    // SQLite keeps a row for the provider — but only the configured ones
    // should hydrate as "connected" in the UI.
    const isConfigured = entry.api_key === '__STORED__';
    if (!isConfigured) continue;

    const existing = local[storageKey] || {};
    if (existing.hasKey) continue;

    // Restore the localStorage shape the UI expects. We don't have the live
    // model list here (backend stores only `default_model`), so seed it
    // with the default and let `refreshAllProviderModels` fill the rest on
    // its next pass — same code path the regular settings panel uses.
    local[storageKey] = {
      hasKey: true,
      model: entry.default_model || existing.model || '',
      models: existing.models?.length
        ? existing.models
        : (entry.default_model ? [entry.default_model] : []),
      baseUrl: entry.base_url || existing.baseUrl || null,
      customMaxOutputTokens: entry.custom_max_output_tokens || 0,
      customInputCost: entry.custom_input_cost || 0,
      customOutputCost: entry.custom_output_cost || 0,
      customCachedInputCost: entry.custom_cached_input_cost || 0,
      customCachedOutputCost: entry.custom_cached_output_cost || 0,
      name: entry.name || existing.name || null,
    };
    mutated = true;
    console.log(`[hydrate] restored "${storageKey}" from backend`);
  }

  if (mutated) saveProviderConfigs(local);
}

/// Minimal provider-connect helper used by the onboarding wizard. Validates
/// the API key by fetching the model list, persists the config, and registers
/// the provider with the Rust backend. Returns `{ models }` on success or
/// throws a string-friendly error.
///
/// `providerType` must be one of the SINGLETON_PROVIDERS ids (Claude / OpenAi
/// / Gemini). For Compatible providers the existing settings UI is needed
/// because the user must also supply a base URL.
export async function quickConnectProvider(providerType, apiKey) {
  const trimmed = (apiKey || '').trim();
  if (!trimmed) throw new Error('Enter an API key first');

  const storageKey = providerType;
  const meta = SINGLETON_PROVIDERS.find((p) => p.id === providerType);
  if (!meta) throw new Error(`Unknown provider: ${providerType}`);

  const models = await api.fetchAiModels(providerType, trimmed, null);
  if (!models?.length) {
    throw new Error('No models returned — check your API key');
  }
  const defaultModel = models[0];

  const allConfigs = loadProviderConfigs();
  allConfigs[storageKey] = {
    hasKey: true,
    model: defaultModel,
    models,
    baseUrl: null,
    customMaxOutputTokens: 0,
    customInputCost: 0,
    customOutputCost: 0,
    customCachedInputCost: 0,
    customCachedOutputCost: 0,
    name: meta.label,
  };
  saveProviderConfigs(allConfigs);

  // Context window + thinking budget come from the backend model registry
  // (`get_context_window`) and from the per-task chat-view selector — neither
  // is a per-provider input anymore.
  await api.setAiProvider(
    providerType, trimmed, defaultModel, null, null,
    0, 0, 0, 0, 0,
    null, null, meta.label,
  );

  return { models, defaultModel };
}

/**
 * Re-fetch models for every connected provider using the backend 5-min TTL
 * cache and overwrite the persisted `models` array. The user's selected
 * `model` id is preserved when still present in the fresh list; otherwise it
 * falls back to the first id (first-run behavior). Errors are swallowed per-
 * provider so one dead key can't starve the others.
 *
 * Returns a Set of storage keys whose model lists actually changed, so the
 * caller can update UI (e.g. model-count badges) in place.
 */
export async function refreshAllProviderModels(forceRefresh = false) {
  const configs = loadProviderConfigs();
  const entries = Object.entries(configs).filter(([, cfg]) => cfg.hasKey);
  if (entries.length === 0) return new Set();

  const results = await Promise.allSettled(
    entries.map(async ([key, cfg]) => {
      // Harness providers don't expose models through the regular
      // `fetch_ai_models` route (no API key, different transport). Route
      // them to their dedicated commands: hardcoded for Claude Code,
      // live JSON-RPC for Codex (see lib/tauri-api.js / harness_models.rs).
      if (key === 'ClaudeCode') {
        const fresh = await api.listClaudeCodeModels();
        return [key, fresh];
      }
      if (key === 'Codex') {
        // Codex needs the binary path override (stored in `baseUrl`) so a
        // user with a non-PATH install still gets a model list. Errors
        // here surface as a Promise rejection and we fall through to the
        // existing `provider fetch failed` warning — same UX as a dead
        // API key elsewhere.
        const fresh = await api.listCodexModels(cfg.baseUrl || null);
        return [key, fresh];
      }
      const type = key.startsWith(`${COMPATIBLE_TYPE}:`) ? COMPATIBLE_TYPE : key;
      // Sentinel; backend resolves to the real keychain-stored key.
      const fresh = await api.fetchAiModels(type, '__STORED__', cfg.baseUrl || null, forceRefresh);
      return [key, fresh];
    }),
  );

  const updated = loadProviderConfigs();
  const changed = new Set();
  for (const r of results) {
    if (r.status === 'rejected') {
      console.warn('[refreshAllProviderModels] provider fetch failed:', r.reason);
      continue;
    }
    const [key, models] = r.value;
    if (!Array.isArray(models) || models.length === 0) continue;
    const prev = updated[key];
    if (!prev) continue;
    const prevList = prev.models || [];
    const sameList = prevList.length === models.length && prevList.every((m, i) => m === models[i]);
    if (sameList) {
      console.log(`[refreshAllProviderModels] ${key} unchanged (${models.length} models)`);
      continue;
    }
    console.log(
      `[refreshAllProviderModels] ${key} CHANGED`,
      { before: prevList, after: models },
    );
    const keepSelected = prev.model && models.includes(prev.model) ? prev.model : models[0];
    updated[key] = { ...prev, models, model: keepSelected };
    changed.add(key);
  }
  if (changed.size > 0) saveProviderConfigs(updated);
  return changed;
}

function slugify(name) {
  return name
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

function compatibleKey(slug) {
  return `${COMPATIBLE_TYPE}:${slug}`;
}

/**
 * Build a labeled cell — small uppercase label on top, control below. Used by
 * every row in the edit area so the label-above-input pattern stays uniform.
 * `options.grow` tunes the flex-grow factor relative to a default of 1.
 */
function buildFieldCell(labelText, controlEl, options = {}) {
  const grow = options.grow ?? 1;
  const cell = el('div', { class: 'ai-provider-card__cell', style: `flex: ${grow} 1 0; min-width: 0;` });
  cell.appendChild(el('span', { class: 'ai-provider-card__cell-label' }, labelText));
  cell.appendChild(controlEl);
  return cell;
}


function showToast(msg, type = 'success') {
  const toast = el('div', { class: `ai-toast ai-toast--${type}` }, msg);
  document.body.appendChild(toast);
  requestAnimationFrame(() => toast.classList.add('ai-toast--visible'));
  setTimeout(() => {
    toast.classList.remove('ai-toast--visible');
    setTimeout(() => toast.remove(), 300);
  }, 2800);
}

/**
 * Build a card for one provider.
 *
 * @param descriptor  { type, label, placeholder, largeContextSupport, storageKey, displayName }
 *   - type: ProviderType string ("Claude" | "OpenAi" | "Gemini" | "Compatible")
 *   - storageKey: localStorage key AND the `provider_key` used by the backend.
 *     For singletons this equals `type`; for Compatible it's `Compatible:<slug>`.
 *   - displayName: extra name shown beside the label (Compatible only).
 * @param onRemoved  callback invoked after a Compatible card is cleared/removed
 *   so the parent can remove the card from the DOM.
 */
function buildProviderCard(descriptor, onRemoved) {
  const {
    type,
    label,
    placeholder,
    storageKey,
    displayName,
  } = descriptor;

  const isCompatible = type === COMPATIBLE_TYPE;
  const configs = loadProviderConfigs();
  const saved = configs[storageKey] || {};
  const isConnected = !!(saved.hasKey && saved.models?.length);

  const card = el('div', { class: `ai-provider-card${isConnected ? ' ai-provider-card--connected' : ''}` });
  // Lets the background refresh locate this card's badge after fetching fresh models.
  card.dataset.storageKey = storageKey;

  // ── Header ──────────────────────────────────────────────────────────────────
  const cardHeader = el('div', { class: 'ai-provider-card__header' });
  const headerLeft = el('div', { class: 'ai-provider-card__header-left' });
  const statusDot = el('span', { class: `ai-provider-card__dot${isConnected ? ' ai-provider-card__dot--on' : ''}` });
  const nameText = displayName ? `${label} — ${displayName}` : label;
  const nameEl = el('span', { class: 'ai-provider-card__name' }, nameText);
  headerLeft.appendChild(statusDot);
  headerLeft.appendChild(nameEl);

  const modelCountEl = el('span', { class: 'ai-provider-card__model-count' });
  if (isConnected) modelCountEl.textContent = `${saved.models.length} models`;
  headerLeft.appendChild(modelCountEl);

  cardHeader.appendChild(headerLeft);

  const headerRight = el('div', { class: 'ai-provider-card__header-right' });

  const editBtn = el('button', { class: 'ai-edit-btn', title: 'Edit connection', style: isConnected ? '' : 'display:none' });
  editBtn.appendChild(icon('M12 20h9 M16.5 3.5a2.121 2.121 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z', 13));
  headerRight.appendChild(editBtn);

  const clearBtn = el('button', { class: 'ai-clear-btn', title: 'Remove connection', style: isConnected ? '' : 'display:none' });
  clearBtn.appendChild(icon('M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 13));
  headerRight.appendChild(clearBtn);
  cardHeader.appendChild(headerRight);
  card.appendChild(cardHeader);

  // ── Edit area ───────────────────────────────────────────────────────────────
  const editArea = el('div', { class: 'ai-provider-card__edit', style: isConnected ? 'display:none' : '' });

  // Every card uses the same label-above-input pattern so the layout stays
  // consistent row-to-row instead of mixing horizontal and vertical labels.

  // Context window comes from the model registry on the backend (see
  // `crates/rustic-agent/src/model_registry.rs` and `condense::get_context_window`).
  // Thinking budget is a per-task client setting set via the chat-view's
  // agent-config popover. Neither needs a per-provider input here.

  const keyInput = el('input', {
    class: 'settings-input ai-key-input',
    type: 'password',
    placeholder: placeholder,
    value: '',
  });

  const eyeBtn = el('button', { class: 'ai-eye-btn', title: 'Show / hide key' });
  let keyVisible = false;
  eyeBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 12m-3 0a3 3 0 1 0 6 0a3 3 0 1 0 -6 0', 13));
  eyeBtn.addEventListener('click', () => {
    keyVisible = !keyVisible;
    keyInput.type = keyVisible ? 'text' : 'password';
  });

  const connectBtn = el('button', { class: 'ai-connect-btn' }, isConnected ? 'Save' : 'Connect');

  // Cancel is Compatible-only — it either deletes a just-added card, or closes
  // the edit area for a connected one without saving.
  const cancelBtn = isCompatible
    ? el('button', { class: 'ai-cancel-btn', title: 'Cancel' }, 'Cancel')
    : null;

  // **Per-provider cost / max-output fields removed for Compatible
  // providers.** These live at the *model* level now — when the user
  // picks a model the chat-view's `pickModel` flow registers it via the
  // custom-model registry, which carries the per-model max-output cap and
  // per-token pricing. Asking the user to enter pricing twice (once on
  // the provider, once on the model) was confusing and the provider-level
  // numbers had no clear semantics when several models from the same
  // OpenAI-compatible endpoint had different pricing tiers.
  //
  // The Compatible card now collects only what's truly provider-scoped:
  // base URL and API key. Max-output and pricing flow through model
  // registration. Backend's `set_ai_provider` accepts null for these
  // fields, so we just don't pass them.
  let urlInput = null;
  if (isCompatible) {
    urlInput = el('input', {
      class: 'settings-input',
      type: 'text',
      placeholder: 'e.g. https://api.groq.com/openai/v1',
      value: saved.baseUrl || '',
    });
  }

  // ── Row 1: Base URL (Compatible) | API Key ───────────────────────────────────
  const topRow = el('div', { class: 'ai-provider-card__grid-row ai-provider-card__top-row' });
  if (isCompatible) {
    topRow.appendChild(buildFieldCell('Base URL', urlInput, { grow: 1 }));
  }
  const keyGroup = el('div', { class: 'ai-provider-card__key-group' });
  keyGroup.appendChild(keyInput);
  keyGroup.appendChild(eyeBtn);
  topRow.appendChild(buildFieldCell('API Key', keyGroup, { grow: 1 }));
  editArea.appendChild(topRow);

  if (isConnected) {
    keyInput.placeholder = 'Leave blank to keep existing key';
  }

  // ── Footer: status text on the left, Cancel + Connect pinned bottom-right ───
  const footer = el('div', { class: 'ai-provider-card__footer' });
  const statusLine = el('div', { class: 'ai-status-line' });
  footer.appendChild(statusLine);
  const footerActions = el('div', { class: 'ai-provider-card__footer-actions' });
  if (cancelBtn) footerActions.appendChild(cancelBtn);
  footerActions.appendChild(connectBtn);
  footer.appendChild(footerActions);
  editArea.appendChild(footer);
  card.appendChild(editArea);

  function setStatus(msg, type) {
    statusLine.textContent = msg;
    statusLine.className = 'ai-status-line' + (type ? ` ai-status-line--${type}` : '');
  }

  function enterConnectedState(models) {
    card.classList.add('ai-provider-card--connected');
    statusDot.classList.add('ai-provider-card__dot--on');
    modelCountEl.textContent = `${models.length} models`;
    clearBtn.style.display = '';
    editBtn.style.display = '';
    editArea.style.display = 'none';
  }

  function enterEditState() {
    card.classList.remove('ai-provider-card--connected');
    statusDot.classList.remove('ai-provider-card__dot--on');
    modelCountEl.textContent = '';
    clearBtn.style.display = 'none';
    editBtn.style.display = 'none';
    editArea.style.display = '';
    keyInput.value = '';
    keyInput.type = 'password';
    keyVisible = false;
    keyInput.placeholder = placeholder;
    connectBtn.textContent = 'Connect';
    setStatus('', '');
  }

  // Reveal the edit area for an already-connected provider, keeping saved
  // values in the fields. Used by the pencil/edit button. Cost / max-output
  // fields no longer exist on the form (model-level now), so only the URL
  // is restored.
  function openEditForExisting() {
    const cur = loadProviderConfigs()[storageKey] || {};
    if (urlInput) urlInput.value = cur.baseUrl || '';
    keyInput.value = '';
    keyInput.type = 'password';
    keyVisible = false;
    keyInput.placeholder = 'Leave blank to keep existing key';
    connectBtn.textContent = 'Save';
    setStatus('', '');
    editArea.style.display = '';
  }

  editBtn.addEventListener('click', openEditForExisting);

  connectBtn.addEventListener('click', async () => {
    const existing = loadProviderConfigs()[storageKey] || {};
    const typedKey = keyInput.value.trim();
    const hasExistingConnection = !!(existing.hasKey && existing.models?.length);
    // The real key never lives in the webview. If the user typed something
    // new we use that; otherwise we send the sentinel so the backend keeps
    // the existing keychain entry.
    const keyForBackend = typedKey || (existing.hasKey ? '__STORED__' : '');

    if (!keyForBackend) { setStatus('Enter an API key first', 'error'); return; }
    const base = urlInput ? urlInput.value.trim() || null : null;
    if (isCompatible && !base) { setStatus('Enter Base URL first', 'error'); return; }

    connectBtn.disabled = true;
    setStatus(hasExistingConnection && !typedKey ? 'Saving…' : 'Connecting…', '');

    try {
      // Re-fetch models only when there's no existing connection, or when the
      // user has typed a new key / changed the Base URL for Compatible.
      const keyChanged = !!typedKey;
      const baseChanged = isCompatible && base !== (existing.baseUrl || null);
      const needsFetch = !hasExistingConnection || keyChanged || baseChanged;

      let models = existing.models || [];
      if (needsFetch) {
        models = await api.fetchAiModels(type, keyForBackend, base || null);
        if (!models?.length) {
          setStatus('No models returned — check your API key', 'error');
          connectBtn.disabled = false;
          return;
        }
      }

      const defaultModel = existing.model && models.includes(existing.model) ? existing.model : models[0];

      const allConfigs = loadProviderConfigs();
      allConfigs[storageKey] = {
        hasKey: true, model: defaultModel, models, baseUrl: base,
        // Cost / max-output fields are model-level now (registered via
        // chat-view's pickModel → setAiProvider with the per-model
        // numbers). We leave them at 0 here — the backend treats 0 as
        // "no provider-level override, use the model's own values."
        customMaxOutputTokens: 0,
        customInputCost: 0, customOutputCost: 0,
        customCachedInputCost: 0, customCachedOutputCost: 0,
        name: displayName || null,
      };
      saveProviderConfigs(allConfigs);

      // All custom-* fields pass null — provider-level overrides are gone.
      // The chat-view's pickModel flow re-calls setAiProvider with the
      // selected model's actual cost/max-output when the user picks a
      // model. Context window + thinking budget are also omitted; the
      // backend's model registry handles the former and per-task chat
      // settings the latter.
      await api.setAiProvider(
        type, keyForBackend, defaultModel, base, null,
        null, null, null,
        null, null,
        null,
        null,
        displayName || null,
      );

      enterConnectedState(models);
      showToast(
        hasExistingConnection && !needsFetch
          ? `${nameText} updated`
          : `Connected to ${nameText} — ${models.length} model${models.length !== 1 ? 's' : ''} available`,
      );
    } catch (e) {
      setStatus(`Error: ${e.message || e}`, 'error');
    }

    connectBtn.disabled = false;
  });

  clearBtn.addEventListener('click', async () => {
    const cur = loadProviderConfigs()[storageKey] || {};
    const isConnected = !!(cur.hasKey && (cur.models?.length || cur.apiKey));

    // Confirm before destructive removal of a connected provider — both the
    // built-in singletons (Claude/OpenAI/Gemini) where this clears the API
    // key, and Compatible cards where this also removes the backend entry.
    if (isConnected) {
      const ok = await showConfirmDialog(
        isCompatible ? 'Remove this provider?' : 'Disconnect this provider?',
        isCompatible
          ? `${nameText || 'This provider'} will be removed and its saved API key forgotten. ` +
            `You can add it again from the + button. Tasks already running will keep ` +
            `using the current key until they finish.`
          : `Your saved API key for ${nameText || 'this provider'} will be cleared. ` +
            `You'll need to re-enter it to send messages with this provider.`,
        {
          confirmLabel: isCompatible ? 'Remove' : 'Disconnect',
          cancelLabel: 'Cancel',
          danger: true,
        },
      );
      if (!ok) return;
    }

    const allConfigs = loadProviderConfigs();
    delete allConfigs[storageKey];
    saveProviderConfigs(allConfigs);

    // For Compatible: also drop the backend entry and remove the card from DOM.
    if (isCompatible) {
      try { await api.removeAiProvider(storageKey); } catch {}
      if (typeof onRemoved === 'function') onRemoved();
      return;
    }

    enterEditState();
  });

  // Cancel button (Compatible only). Semantics:
  //   - never-connected card (no apiKey yet) → delete the card entirely.
  //     Matches the trash button; the + icon re-adds a fresh one if desired.
  //   - editing an already-connected card → close the edit area, drop unsaved
  //     field changes, keep the connection.
  if (cancelBtn) {
    cancelBtn.addEventListener('click', async () => {
      const cur = loadProviderConfigs()[storageKey] || {};
      const hasConnection = !!(cur.hasKey && cur.models?.length);

      if (hasConnection) {
        // Revert field values to saved state so the next edit starts clean.
        // Cost/max-output fields are gone — only the URL is restored.
        if (urlInput) urlInput.value = cur.baseUrl || '';
        enterConnectedState(cur.models);
        return;
      }

      const allConfigs = loadProviderConfigs();
      delete allConfigs[storageKey];
      saveProviderConfigs(allConfigs);
      try { await api.removeAiProvider(storageKey); } catch {}
      if (typeof onRemoved === 'function') onRemoved();
    });
  }

  // Re-register saved key with backend silently on mount. The backend
  // already has the real key in its keychain; the sentinel tells
  // set_ai_provider to keep it as-is and just refresh the model/base.
  // All custom-* fields pass null because the cost/max-output overrides
  // are now model-level (set by chat-view's pickModel when the user
  // picks a model), not provider-level.
  if (isConnected) {
    const base = isCompatible ? (saved.baseUrl || null) : null;
    api.setAiProvider(
      type, '__STORED__', saved.model || saved.models[0], base, null,
      null, null, null,
      null, null,
      null,
      null,
      saved.name || displayName || null,
    ).catch(() => {});
  }

  return card;
}

function openAddCompatibleModal(onDone) {
  const body = el('div', { class: 'skills-edit-form' });

  const nameInput = el('input', {
    class: 'rustic-modal__input',
    type: 'text',
    placeholder: 'e.g. Groq, DeepSeek, Together',
  });
  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Display Name'));
  body.appendChild(nameInput);

  const hint = el('div', { class: 'ai-status-line' },
    'Pick a short name to identify this provider. Base URL, API key and pricing are set on the card after you add it.');
  body.appendChild(hint);

  const err = el('div', { class: 'skills-install-form__status' });
  body.appendChild(err);

  openModal({
    title: 'Add OpenAI-compatible provider',
    body,
    buttons: [
      { label: 'Cancel', variant: 'secondary' },
      {
        label: 'Add',
        variant: 'primary',
        onClick: () => {
          const name = nameInput.value.trim();
          if (!name) {
            err.textContent = 'Display name is required';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }
          const slug = slugify(name);
          if (!slug) {
            err.textContent = 'Display name must contain at least one alphanumeric character';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }

          const configs = loadProviderConfigs();
          const key = compatibleKey(slug);
          if (configs[key]) {
            err.textContent = `A provider named "${name}" already exists`;
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }

          // Reserve the slot (empty placeholder) so the card is rendered.
          configs[key] = { name, baseUrl: '', hasKey: false, models: [] };
          saveProviderConfigs(configs);
          onDone?.({ slug, name, storageKey: key });
          return true;
        },
      },
    ],
  });

  setTimeout(() => nameInput.focus(), 0);
}

/// Card for a harness-backed (subscription) provider. Unlike API-key cards,
/// there's nothing for the user to type — they either have the CLI installed
/// + signed in or they don't. We only persist a marker entry so the model
/// picker shows it; the harness runtime checks the binary lazily on send.
///
/// Layout mirrors the regular API-key card's header row so the AI Providers
/// list reads as one consistent column: status dot + name + status badge on
/// the left, action buttons on the right. The binary-path override (rarely
/// needed — only for non-PATH installs) is tucked behind a pencil button so
/// the common one-click case stays uncluttered.
function buildSubscriptionCard({ storageKey, label, placeholderModel }) {
  const card = el('div', { class: 'ai-provider-card', 'data-storage-key': storageKey });

  // ── Header row (always visible) ──────────────────────────────────────────
  const header = el('div', { class: 'ai-provider-card__header' });
  const headerLeft = el('div', { class: 'ai-provider-card__header-left' });
  const statusDot = el('span', { class: 'ai-provider-card__dot' });
  const nameEl = el('span', { class: 'ai-provider-card__name' }, label);
  // Same .ai-provider-card__model-count pill the API-key cards use for "N
  // models" — repurposed here for the probe status badge so the styling
  // (small uppercase rounded chip) carries over without new CSS.
  const statusBadge = el('span', { class: 'ai-provider-card__model-count' });
  headerLeft.appendChild(statusDot);
  headerLeft.appendChild(nameEl);
  headerLeft.appendChild(statusBadge);
  header.appendChild(headerLeft);

  const headerRight = el('div', { class: 'ai-provider-card__header-right' });
  // Pencil opens the binary-path override. Reuses the same .ai-edit-btn
  // class as the API-key cards so the icon and hover behavior match.
  const editBtn = el('button', { class: 'ai-edit-btn', type: 'button', title: 'Override binary path' });
  editBtn.appendChild(icon('M12 20h9 M16.5 3.5a2.121 2.121 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z', 13));
  const recheckBtn = el('button', { class: 'btn', type: 'button', title: 'Re-run the install + sign-in probe' }, 'Re-check');
  const enableBtn = el('button', { class: 'btn btn-primary', type: 'button' }, 'Enable');
  const disableBtn = el('button', { class: 'btn', type: 'button' }, 'Disable');
  headerRight.appendChild(editBtn);
  headerRight.appendChild(recheckBtn);
  headerRight.appendChild(enableBtn);
  headerRight.appendChild(disableBtn);
  header.appendChild(headerRight);
  card.appendChild(header);

  // ── Edit area: binary path override (collapsed by default) ───────────────
  // Hidden until the user clicks the pencil. Empty = use PATH (default for
  // Homebrew, npm-global, and the standard installer).
  const editArea = el('div', { class: 'ai-provider-card__edit', style: 'display:none; margin-top:8px;' });
  editArea.appendChild(el('label', {
    style: 'display:block; font-size:0.85em; opacity:0.8; margin-bottom:2px;',
  }, 'Binary path override (leave empty to use PATH)'));
  const binaryPathInput = el('input', {
    type: 'text',
    class: 'settings-input',
    placeholder: storageKey === 'ClaudeCode' ? 'e.g. C:\\Users\\you\\AppData\\Roaming\\npm\\claude.cmd' : 'e.g. /usr/local/bin/codex',
  });
  editArea.appendChild(binaryPathInput);
  card.appendChild(editArea);

  // Hydrate the input from the previously-saved config and auto-expand if a
  // non-default path is already set so the user can see/edit it without
  // hunting for the pencil.
  {
    const cfg = loadProviderConfigs()[storageKey];
    if (cfg?.baseUrl) {
      binaryPathInput.value = cfg.baseUrl;
      editArea.style.display = '';
    }
  }
  editBtn.addEventListener('click', () => {
    editArea.style.display = editArea.style.display === 'none' ? '' : 'none';
  });

  function currentBinaryPath() {
    const v = binaryPathInput.value.trim();
    return v || null;
  }

  // Latest probe result; null until the first probe completes.
  let lastProbe = null;

  /// Pull a `1.2.3`-style version number out of whatever the probe reports.
  /// Claude Code answers `2.1.113 (Claude Code)`, Codex answers
  /// `codex-cli 0.125.0` — both should render as just the number so the
  /// inlined name stays compact.
  function extractVersion(raw) {
    if (!raw) return '';
    const m = String(raw).match(/(\d+(?:\.\d+)+)/);
    return m ? m[1] : String(raw);
  }

  /// Render the row's state from the cached probe + the saved enabled flag.
  /// Called after every probe and after enable/disable. The label inlines
  /// the CLI version when the probe is healthy, so the row reads as
  /// "Claude Code 2.1.113" instead of label + separate badge. The badge is
  /// only shown for unhealthy states (sign-in needed, CLI missing, etc.).
  function refreshStatus() {
    const configs = loadProviderConfigs();
    const enabled = !!configs[storageKey]?.hasKey;

    // Toggle the connected look + which action button shows.
    card.classList.toggle('ai-provider-card--connected', enabled);
    statusDot.classList.toggle('ai-provider-card__dot--on', enabled);
    enableBtn.style.display = enabled ? 'none' : '';
    disableBtn.style.display = enabled ? '' : 'none';

    // Default presentation — overridden below per probe state.
    nameEl.textContent = label;
    statusBadge.style.display = '';
    statusBadge.removeAttribute('title');

    if (!lastProbe) {
      statusBadge.textContent = enabled ? 'Checking…' : 'Not enabled';
      enableBtn.disabled = true;
      return;
    }

    let badgeText = '';
    let canEnable = false;
    switch (lastProbe.status) {
      case 'authenticated': {
        // Healthy → fold the version into the name and hide the badge so
        // the whole row collapses to a single inline label.
        const version = extractVersion(lastProbe.version);
        nameEl.textContent = version ? `${label} ${version}` : label;
        statusBadge.style.display = 'none';
        canEnable = true;
        break;
      }
      case 'not_authenticated':
        badgeText = `Run \`${storageKey === 'ClaudeCode' ? 'claude' : 'codex login'}\``;
        break;
      case 'not_installed':
        badgeText = 'CLI not found';
        break;
      case 'probe_failed':
        badgeText = 'Probe failed';
        break;
      default:
        badgeText = 'Unknown';
    }

    if (badgeText) {
      statusBadge.textContent = badgeText;
      statusBadge.title = lastProbe.detail || badgeText;
    }
    enableBtn.disabled = !canEnable;
  }
  refreshStatus();

  async function probe() {
    recheckBtn.disabled = true;
    statusBadge.textContent = 'Probing…';
    try {
      lastProbe = await api.probeHarnessAuth(storageKey, currentBinaryPath());
    } catch (err) {
      lastProbe = { status: 'probe_failed', detail: err?.message || String(err) };
    } finally {
      recheckBtn.disabled = false;
      refreshStatus();
    }
  }

  // Re-probe when the user finishes editing the binary-path field so the
  // status reflects the new path without needing a Re-check click.
  binaryPathInput.addEventListener('change', probe);

  // Probe on mount so the user sees install state without clicking anything.
  probe();

  recheckBtn.addEventListener('click', probe);

  enableBtn.addEventListener('click', async () => {
    enableBtn.disabled = true;
    enableBtn.textContent = 'Enabling…';
    try {
      const overridePath = currentBinaryPath();

      // Re-probe right before enable (with the current path override) so we
      // don't register a provider entry for a CLI that isn't installed or
      // signed in. The button is supposed to be disabled in that case but a
      // stale probe result could let the user click anyway.
      lastProbe = await api.probeHarnessAuth(storageKey, overridePath);
      if (lastProbe.status !== 'authenticated') {
        refreshStatus();
        return;
      }

      // Pull the real model list up front so the picker has something to
      // show as soon as the user clicks Enable. Falls back to the bare
      // placeholder on failure (e.g. Codex sign-in expired between probe
      // and enable) — refreshAllProviderModels will retry on next reload.
      let models = [placeholderModel];
      let defaultModel = placeholderModel;
      try {
        const fresh = storageKey === 'ClaudeCode'
          ? await api.listClaudeCodeModels()
          : await api.listCodexModels(overridePath || null);
        if (Array.isArray(fresh) && fresh.length > 0) {
          models = fresh;
          defaultModel = fresh[0];
        }
      } catch (e) {
        console.warn(`[${storageKey}] model list fetch failed; falling back to placeholder`, e);
      }

      // Persist a marker entry on the backend so it shows up in ai_config
      // and tasks can store provider_type = "ClaudeCode". `api_key` is
      // unused for harness providers; we re-use the `base_url` slot to
      // carry the binary path override so the harness runtime can read it
      // back without a new column.
      await api.setAiProvider(
        storageKey, '', defaultModel, overridePath, null,
        0, 0, 0, 0, 0,
        null, null, label,
      );
      const configs = loadProviderConfigs();
      configs[storageKey] = {
        hasKey: true,
        model: defaultModel,
        models,
        baseUrl: overridePath,
        name: label,
      };
      saveProviderConfigs(configs);
      refreshStatus();
    } catch (err) {
      statusBadge.textContent = `Enable failed`;
      statusBadge.title = err?.message || String(err);
    } finally {
      enableBtn.textContent = 'Enable';
      refreshStatus();
    }
  });

  disableBtn.addEventListener('click', async () => {
    disableBtn.disabled = true;
    disableBtn.textContent = 'Disabling…';
    try {
      await api.removeAiProvider(storageKey);
      const configs = loadProviderConfigs();
      delete configs[storageKey];
      saveProviderConfigs(configs);
    } catch (err) {
      statusBadge.textContent = `Disable failed`;
      statusBadge.title = err?.message || String(err);
    } finally {
      disableBtn.textContent = 'Disable';
      refreshStatus();
    }
  });

  return card;
}

export function createAiSettings() {
  const container = el('div', { class: 'ai-providers-container' });

  // Holders for each section so we can rebuild them after hydrate completes
  // without touching the rest of the panel. Singletons are kept in their
  // own holder (was previously appended directly to `container`) so the
  // post-hydrate rebuild can re-render them as a unit. Without this the
  // singleton cards were built once from the initial (pre-hydrate)
  // localStorage state and never refreshed — so on every app rebuild that
  // wiped the WebView's localStorage you saw "Not connected" cards even
  // though the API keys were still in the OS keychain. The user then
  // re-entered keys to "fix" it, which only worked because re-entering
  // wrote a fresh keychain entry over the existing one.
  const singletonHolder = el('div', { class: 'ai-providers-singletons' });
  container.appendChild(singletonHolder);

  function renderSingletonCards() {
    singletonHolder.replaceChildren();
    for (const p of SINGLETON_PROVIDERS) {
      singletonHolder.appendChild(buildProviderCard({
        type: p.id,
        label: p.label,
        placeholder: p.placeholder,
        storageKey: p.id,
        displayName: null,
      }));
    }
  }
  renderSingletonCards();

  // Subscription-mode providers (Claude Code, Codex). These don't use API keys
  // — the user authenticates with the CLI itself (`claude` / `codex login`)
  // and Rustic just spawns the binary. See the harness module in
  // `crates/rustic-agent/src/harness/`. Their connected state is probed
  // live by each card on mount, so they don't need re-rendering on hydrate.
  container.appendChild(buildSubscriptionCard({
    storageKey: 'ClaudeCode',
    label: 'Claude Code',
    placeholderModel: 'claude-code',
  }));

  container.appendChild(buildSubscriptionCard({
    storageKey: 'Codex',
    label: 'Codex',
    placeholderModel: 'codex',
  }));

  // Render one card per saved Compatible entry
  const compatibleHolder = el('div', { class: 'ai-providers-compatible' });
  container.appendChild(compatibleHolder);

  function renderCompatibleCards() {
    compatibleHolder.innerHTML = '';
    const configs = loadProviderConfigs();
    for (const [key, cfg] of Object.entries(configs)) {
      if (!key.startsWith(`${COMPATIBLE_TYPE}:`)) continue;
      const card = buildProviderCard({
        type: COMPATIBLE_TYPE,
        label: 'OpenAI-Compatible',
        placeholder: 'API key (if any)',
        storageKey: key,
        displayName: cfg.name || key.slice(COMPATIBLE_TYPE.length + 1),
      }, () => renderCompatibleCards());
      compatibleHolder.appendChild(card);
    }
  }

  renderCompatibleCards();

  // Expose the add-action so the AI Providers collapsible header can call it.
  container.addCompatibleProvider = () => {
    openAddCompatibleModal(() => renderCompatibleCards());
  };

  // ── Post-hydrate rebuild ──────────────────────────────────────────────
  // Hydrate from the backend's persisted ai_config (SQLite, with keys in
  // the OS keychain). After it lands we **rebuild both singleton and
  // compatible cards** so the UI reflects the restored state — the cards
  // mounted above were built from whatever localStorage held at panel-open
  // time, which on a fresh rebuild is empty.
  hydrateProviderConfigsFromBackend()
    .then(() => {
      // Rebuild from the now-up-to-date localStorage (hydrate filled in
      // the missing entries from SQLite, so loadProviderConfigs() reflects
      // the keychain truth).
      renderSingletonCards();
      renderCompatibleCards();
    })
    .then(() => refreshAllProviderModels())
    .then((changed) => {
      if (!changed?.size) return;
      // Compatible cards rebuild because their model lists may have changed.
      renderCompatibleCards();
      // For singletons we just patch the model-count badge — rebuilding
      // would drop the user's in-flight key-input edits if they're typing
      // when the refresh lands.
      const configs = loadProviderConfigs();
      for (const key of changed) {
        const card = container.querySelector(`.ai-provider-card[data-storage-key="${CSS.escape(key)}"]`);
        if (!card) continue;
        const badge = card.querySelector('.ai-provider-card__model-count');
        const count = configs[key]?.models?.length ?? 0;
        if (badge && count > 0) badge.textContent = `${count} models`;
      }
    })
    .catch((e) => {
      console.warn('[ai-settings] hydrate/refresh failed:', e);
    });

  return container;
}
