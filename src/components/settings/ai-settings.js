import { el, icon } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import * as api from '../../lib/tauri-api.js';

const SINGLETON_PROVIDERS = [
  { id: 'Claude',  label: 'Anthropic',       placeholder: 'sk-ant-…', defaultContextWindow: 200000  },
  { id: 'OpenAi',  label: 'OpenAI',          placeholder: 'sk-…',     defaultContextWindow: 128000  },
  { id: 'Gemini',  label: 'Google Gemini',   placeholder: 'AIza…',    defaultContextWindow: 1048576 },
];

const COMPATIBLE_DEFAULT_CONTEXT_WINDOW = 128000;

const COMPATIBLE_TYPE = 'Compatible';

const MODEL_MAX_OUTPUT = {
  // Anthropic (Claude)
  'claude-opus-4-7':    128000,
  'claude-opus-4-6':    128000, 'claude-opus-4':    128000,
  'claude-sonnet-4-6':   64000, 'claude-sonnet-4':   64000, 'claude-sonnet-4-5': 64000,
  'claude-haiku-4-5':    64000,
  // OpenAI — GPT-5.4 family (current)
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

const MODEL_PRICING = {
  // Claude
  'claude-opus-4':      { input: 5.0,   output: 25.0  },
  'claude-sonnet-4':    { input: 3.0,   output: 15.0  },
  'claude-haiku-4':     { input: 1.0,   output: 5.0   },
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
    defaultContextWindow,
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

  const ctxWindowDefault = isCompatible
    ? COMPATIBLE_DEFAULT_CONTEXT_WINDOW
    : (defaultContextWindow || 128000);

  // Inputs common to every provider.
  const ctxWindowInput = el('input', {
    class: 'settings-input',
    type: 'number',
    placeholder: String(ctxWindowDefault),
    value: saved.customContextWindow || '',
    title: 'Max tokens the model will accept. Leave blank for the provider default.',
  });

  const thinkInput = el('input', {
    class: 'settings-input',
    type: 'number',
    placeholder: '10000 (Claude) / 0',
    value: saved.customThinkingBudget || '',
    title: 'Tokens reserved for extended thinking. Lower = cheaper, less deep reasoning. 0 disables thinking.',
  });

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

  let urlInput = null;
  let maxOutputInput = null;
  let inputCostInput = null;
  let outputCostInput = null;
  let cachedInputCostInput = null;
  let cachedOutputCostInput = null;

  if (isCompatible) {
    urlInput = el('input', {
      class: 'settings-input',
      type: 'text',
      placeholder: 'e.g. https://api.groq.com/openai/v1',
      value: saved.baseUrl || '',
    });

    maxOutputInput = el('input', {
      class: 'settings-input',
      type: 'number',
      placeholder: '16384',
      value: saved.customMaxOutputTokens || '',
    });

    inputCostInput = el('input', {
      class: 'settings-input', type: 'number', step: '0.01',
      placeholder: '$/1M tok', value: saved.customInputCost || '',
    });
    outputCostInput = el('input', {
      class: 'settings-input', type: 'number', step: '0.01',
      placeholder: '$/1M tok', value: saved.customOutputCost || '',
    });
    cachedInputCostInput = el('input', {
      class: 'settings-input', type: 'number', step: '0.01',
      placeholder: '$/1M tok', value: saved.customCachedInputCost || '',
    });
    cachedOutputCostInput = el('input', {
      class: 'settings-input', type: 'number', step: '0.01',
      placeholder: '$/1M tok', value: saved.customCachedOutputCost || '',
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

  // ── Row 2: Max Output (Compatible) | Context Window | Thinking Budget ────────
  const numbersRow = el('div', { class: 'ai-provider-card__grid-row' });
  if (maxOutputInput) {
    numbersRow.appendChild(buildFieldCell('Max Output Tokens', maxOutputInput));
  }
  numbersRow.appendChild(buildFieldCell('Context Window', ctxWindowInput));
  numbersRow.appendChild(buildFieldCell('Thinking Budget', thinkInput));
  editArea.appendChild(numbersRow);

  // ── Row 3: Cost — Input | Output | Cached Input | Cached Output (Compatible) ─
  if (isCompatible) {
    const costRow = el('div', { class: 'ai-provider-card__grid-row' });
    costRow.appendChild(buildFieldCell('Input cost', inputCostInput));
    costRow.appendChild(buildFieldCell('Output cost', outputCostInput));
    costRow.appendChild(buildFieldCell('Cached input', cachedInputCostInput));
    costRow.appendChild(buildFieldCell('Cached output', cachedOutputCostInput));
    editArea.appendChild(costRow);
  }

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
  // values in the fields. Used by the pencil/edit button.
  function openEditForExisting() {
    const cur = loadProviderConfigs()[storageKey] || {};
    if (urlInput) urlInput.value = cur.baseUrl || '';
    if (maxOutputInput) maxOutputInput.value = cur.customMaxOutputTokens || '';
    if (inputCostInput) inputCostInput.value = cur.customInputCost || '';
    if (outputCostInput) outputCostInput.value = cur.customOutputCost || '';
    if (cachedInputCostInput) cachedInputCostInput.value = cur.customCachedInputCost || '';
    if (cachedOutputCostInput) cachedOutputCostInput.value = cur.customCachedOutputCost || '';
    ctxWindowInput.value = cur.customContextWindow || '';
    thinkInput.value = cur.customThinkingBudget || '';
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
      const customMaxOut = maxOutputInput ? parseInt(maxOutputInput.value, 10) || 0 : 0;
      const customInCost = inputCostInput ? parseFloat(inputCostInput.value) || 0 : 0;
      const customOutCost = outputCostInput ? parseFloat(outputCostInput.value) || 0 : 0;
      const customCachedInCost = cachedInputCostInput ? parseFloat(cachedInputCostInput.value) || 0 : 0;
      const customCachedOutCost = cachedOutputCostInput ? parseFloat(cachedOutputCostInput.value) || 0 : 0;
      const customCtxWindow = parseInt(ctxWindowInput.value, 10) || 0;
      const customThinkBudget = parseInt(thinkInput.value, 10) || 0;

      const allConfigs = loadProviderConfigs();
      allConfigs[storageKey] = {
        hasKey: true, model: defaultModel, models, baseUrl: base,
        customMaxOutputTokens: customMaxOut,
        customInputCost: customInCost, customOutputCost: customOutCost,
        customCachedInputCost: customCachedInCost, customCachedOutputCost: customCachedOutCost,
        customContextWindow: customCtxWindow,
        customThinkingBudget: customThinkBudget,
        name: displayName || null,
      };
      saveProviderConfigs(allConfigs);

      await api.setAiProvider(
        type, keyForBackend, defaultModel, base, null,
        customMaxOut, customInCost, customOutCost,
        customCachedInCost, customCachedOutCost,
        customCtxWindow || null,
        customThinkBudget || null,
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
        if (urlInput) urlInput.value = cur.baseUrl || '';
        if (maxOutputInput) maxOutputInput.value = cur.customMaxOutputTokens || '';
        if (inputCostInput) inputCostInput.value = cur.customInputCost || '';
        if (outputCostInput) outputCostInput.value = cur.customOutputCost || '';
        if (cachedInputCostInput) cachedInputCostInput.value = cur.customCachedInputCost || '';
        if (cachedOutputCostInput) cachedOutputCostInput.value = cur.customCachedOutputCost || '';
        ctxWindowInput.value = cur.customContextWindow || '';
        thinkInput.value = cur.customThinkingBudget || '';
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

  // Re-register saved key with backend silently on mount. The backend already
  // has the real key in its keychain; the sentinel tells set_ai_provider to
  // keep it as-is and just refresh the model/base/limits fields.
  if (isConnected) {
    const base = isCompatible ? (saved.baseUrl || null) : null;
    api.setAiProvider(
      type, '__STORED__', saved.model || saved.models[0], base, null,
      saved.customMaxOutputTokens || null, saved.customInputCost || null, saved.customOutputCost || null,
      saved.customCachedInputCost || null, saved.customCachedOutputCost || null,
      saved.customContextWindow || null,
      saved.customThinkingBudget || null,
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

export function createAiSettings() {
  const container = el('div', { class: 'ai-providers-container' });

  for (const p of SINGLETON_PROVIDERS) {
    container.appendChild(buildProviderCard({
      type: p.id,
      label: p.label,
      placeholder: p.placeholder,
      defaultContextWindow: p.defaultContextWindow,
      storageKey: p.id,
      displayName: null,
    }));
  }

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
        defaultContextWindow: COMPATIBLE_DEFAULT_CONTEXT_WINDOW,
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

  // Kick off a background refresh so newly-released models (e.g. a just-
  // published Claude snapshot) appear without forcing the user to re-enter
  // their API key. Backend has a 5-min TTL so repeated opens are free.
  refreshAllProviderModels().then((changed) => {
    if (!changed.size) return;
    // Rebuild compatible cards first — their count and pre-populated fields
    // both depend on saved.models.
    renderCompatibleCards();
    // For the singleton cards that are already mounted, just patch the
    // badge text in place (rebuilding them would drop the user's edits).
    const configs = loadProviderConfigs();
    for (const key of changed) {
      const card = container.querySelector(`.ai-provider-card[data-storage-key="${CSS.escape(key)}"]`);
      if (!card) continue;
      const badge = card.querySelector('.ai-provider-card__model-count');
      const count = configs[key]?.models?.length ?? 0;
      if (badge && count > 0) badge.textContent = `${count} models`;
    }
  }).catch(() => { /* surface-level refresh; swallow errors */ });

  return container;
}
