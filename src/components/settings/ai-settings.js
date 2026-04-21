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

export function loadProviderConfigs() {
  try {
    const raw = JSON.parse(localStorage.getItem('rustic_provider_configs') || '{}');
    return migrateConfigs(raw);
  } catch {
    return {};
  }
}

function saveProviderConfigs(configs) {
  localStorage.setItem('rustic_provider_configs', JSON.stringify(configs));
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
  const isConnected = !!(saved.apiKey && saved.models?.length);

  const card = el('div', { class: `ai-provider-card${isConnected ? ' ai-provider-card--connected' : ''}` });

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

  let urlInput = null;
  let maxOutputInput = null;
  let inputCostInput = null;
  let outputCostInput = null;

  if (isCompatible) {
    const urlRow = el('div', { class: 'ai-provider-card__row' });
    urlRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Base URL'));
    urlInput = el('input', {
      class: 'settings-input',
      type: 'text',
      placeholder: 'e.g. https://api.groq.com/openai/v1',
      value: saved.baseUrl || '',
    });
    urlRow.appendChild(urlInput);
    editArea.appendChild(urlRow);

    const maxRow = el('div', { class: 'ai-provider-card__row' });
    maxRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Max Output Tokens'));
    maxOutputInput = el('input', {
      class: 'settings-input',
      type: 'number',
      placeholder: '16384',
      value: saved.customMaxOutputTokens || '',
    });
    maxRow.appendChild(maxOutputInput);
    editArea.appendChild(maxRow);

    const costRow = el('div', { class: 'ai-provider-card__row ai-provider-card__cost-row' });
    costRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Cost ($/1M tokens)'));
    const costFields = el('div', { class: 'ai-provider-card__cost-fields' });
    inputCostInput = el('input', {
      class: 'settings-input ai-cost-input', type: 'number', step: '0.01',
      placeholder: 'Input', value: saved.customInputCost || '',
    });
    outputCostInput = el('input', {
      class: 'settings-input ai-cost-input', type: 'number', step: '0.01',
      placeholder: 'Output', value: saved.customOutputCost || '',
    });
    costFields.appendChild(inputCostInput);
    costFields.appendChild(outputCostInput);
    costRow.appendChild(costFields);
    editArea.appendChild(costRow);
  }

  // Context window input — shown for every provider
  const ctxRow = el('div', { class: 'ai-provider-card__row' });
  ctxRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Context Window'));
  const ctxWindowDefault = isCompatible
    ? COMPATIBLE_DEFAULT_CONTEXT_WINDOW
    : (defaultContextWindow || 128000);
  const ctxWindowInput = el('input', {
    class: 'settings-input',
    type: 'number',
    placeholder: String(ctxWindowDefault),
    value: saved.customContextWindow || '',
    title: 'Max tokens the model will accept. Leave blank for the provider default.',
  });
  ctxRow.appendChild(ctxWindowInput);
  editArea.appendChild(ctxRow);

  // Thinking budget input — shown for every provider (ignored if provider
  // doesn't support extended thinking). 0 or blank = use per-provider default
  // (10k for Claude, 0 elsewhere).
  const thinkRow = el('div', { class: 'ai-provider-card__row' });
  thinkRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Thinking Budget'));
  const thinkInput = el('input', {
    class: 'settings-input',
    type: 'number',
    placeholder: '10000 (Claude) / 0',
    value: saved.customThinkingBudget || '',
    title: 'Tokens reserved for extended thinking. Lower = cheaper, less deep reasoning. 0 disables thinking.',
  });
  thinkRow.appendChild(thinkInput);
  editArea.appendChild(thinkRow);

  // API key row
  const keyRow = el('div', { class: 'ai-provider-card__row' });
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
  keyRow.appendChild(keyInput);
  keyRow.appendChild(eyeBtn);
  keyRow.appendChild(connectBtn);
  editArea.appendChild(keyRow);

  if (isConnected) {
    keyInput.placeholder = 'Leave blank to keep existing key';
  }

  const statusLine = el('div', { class: 'ai-status-line' });
  editArea.appendChild(statusLine);
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
    const hasExistingConnection = !!(existing.apiKey && existing.models?.length);
    const key = typedKey || existing.apiKey || '';

    if (!key) { setStatus('Enter an API key first', 'error'); return; }
    const base = urlInput ? urlInput.value.trim() || null : null;
    if (isCompatible && !base) { setStatus('Enter Base URL first', 'error'); return; }

    connectBtn.disabled = true;
    setStatus(hasExistingConnection && !typedKey ? 'Saving…' : 'Connecting…', '');

    try {
      // Re-fetch models only when there's no existing connection, or when the
      // user has typed a new key / changed the Base URL for Compatible.
      const keyChanged = !!typedKey && typedKey !== existing.apiKey;
      const baseChanged = isCompatible && base !== (existing.baseUrl || null);
      const needsFetch = !hasExistingConnection || keyChanged || baseChanged;

      let models = existing.models || [];
      if (needsFetch) {
        models = await api.fetchAiModels(type, key, base || null);
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
      const customCtxWindow = parseInt(ctxWindowInput.value, 10) || 0;
      const customThinkBudget = parseInt(thinkInput.value, 10) || 0;

      const allConfigs = loadProviderConfigs();
      allConfigs[storageKey] = {
        apiKey: key, model: defaultModel, models, baseUrl: base,
        customMaxOutputTokens: customMaxOut, customInputCost: customInCost, customOutputCost: customOutCost,
        customContextWindow: customCtxWindow,
        customThinkingBudget: customThinkBudget,
        name: displayName || null,
      };
      saveProviderConfigs(allConfigs);

      await api.setAiProvider(
        type, key, defaultModel, base, null,
        customMaxOut, customInCost, customOutCost,
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

  // Re-register saved key with backend silently on mount
  if (isConnected) {
    const base = isCompatible ? (saved.baseUrl || null) : null;
    api.setAiProvider(
      type, saved.apiKey, saved.model || saved.models[0], base, null,
      saved.customMaxOutputTokens || null, saved.customInputCost || null, saved.customOutputCost || null,
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
          configs[key] = { name, baseUrl: '', apiKey: '', models: [] };
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

  return container;
}
