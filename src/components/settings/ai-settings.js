import { el, icon } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';

const PROVIDERS = [
  { id: 'Claude',      label: 'Anthropic Claude' },
  { id: 'OpenAi',     label: 'OpenAI' },
  { id: 'Gemini',     label: 'Google Gemini' },
  { id: 'Compatible', label: 'OpenAI-Compatible' },
];

const LARGE_CONTEXT_PREFIXES = [
  'gemini-1.5-pro', 'gemini-1.5-flash',
  'gemini-2.0-pro', 'gemini-2.0-flash',
  'gemini-2.5-pro', 'gemini-2.5-flash',
];

const MODEL_MAX_OUTPUT = {
  'claude-opus-4-20250514':   32000,
  'claude-sonnet-4-20250514': 64000,
  'claude-haiku-4-20250307':  16000,
  'gpt-4o':                   16384,
  'gpt-4o-mini':              16384,
  'o1':                      100000,
  'o1-mini':                  65536,
  'o3':                      100000,
  'o3-mini':                 100000,
  'gemini-2.5-pro':           65536,
  'gemini-2.5-flash':         65536,
  'gemini-2.0-pro':           65536,
  'gemini-2.0-flash':         65536,
  'gemini-1.5-pro':            8192,
  'gemini-1.5-flash':          8192,
};

function maxOutputFor(modelId) {
  if (MODEL_MAX_OUTPUT[modelId]) return MODEL_MAX_OUTPUT[modelId];
  for (const [key, val] of Object.entries(MODEL_MAX_OUTPUT)) {
    if (modelId.startsWith(key)) return val;
  }
  return 8192;
}

function supportsLargeContext(modelId) {
  return LARGE_CONTEXT_PREFIXES.some(p => modelId.startsWith(p) || modelId.includes(p));
}

function loadProviderConfigs() {
  try { return JSON.parse(localStorage.getItem('rustic_provider_configs') || '{}'); }
  catch { return {}; }
}

function saveProviderConfigs(configs) {
  localStorage.setItem('rustic_provider_configs', JSON.stringify(configs));
}

async function fetchModelsFromApi(providerId, apiKey, baseUrl) {
  // Routed through the Rust backend (reqwest) to avoid CORS/CSP restrictions in Tauri's webview
  const models = await api.fetchAiModels(providerId, apiKey, baseUrl || null);
  return models || [];
}

function buildProviderSection(providerId) {
  const p = PROVIDERS.find(x => x.id === providerId);
  const configs = loadProviderConfigs();
  const saved = configs[providerId] || {};

  const section = el('div', { class: 'settings-provider' });
  section.appendChild(el('div', { class: 'settings-provider__name' }, p.label));

  // Status line (shown below API key row on error/loading)
  const statusLine = el('div', { class: 'ai-status-line' });

  // ── API Key row ─────────────────────────────────────────────────────────────
  const keyRow = el('div', { class: 'settings-row settings-row--compact' });
  keyRow.appendChild(el('div', { class: 'settings-row__label' }, 'API Key'));

  const keyField = el('div', { class: 'ai-key-field' });
  const keyInput = el('input', {
    class: 'settings-input',
    type: 'password',
    placeholder: 'Enter API key…',
    value: saved.apiKey || '',
  });

  const confirmBtn = el('button', { class: 'ai-key-btn ai-key-btn--confirm', title: 'Connect' });
  confirmBtn.appendChild(icon('M5 12l5 5L20 7', 13));

  const clearBtn = el('button', { class: 'ai-key-btn ai-key-btn--clear', title: 'Clear' });
  clearBtn.appendChild(icon('M18 6L6 18M6 6l12 12', 13));

  keyField.appendChild(keyInput);
  keyField.appendChild(confirmBtn);
  keyField.appendChild(clearBtn);
  keyRow.appendChild(keyField);
  section.appendChild(keyRow);

  // ── Base URL row (Compatible only) ──────────────────────────────────────────
  let urlInput = null;
  if (providerId === 'Compatible') {
    const urlRow = el('div', { class: 'settings-row settings-row--compact' });
    urlRow.appendChild(el('div', { class: 'settings-row__label' }, 'Base URL'));
    urlInput = el('input', {
      class: 'settings-input',
      type: 'text',
      placeholder: 'https://api.example.com/v1',
      value: saved.baseUrl || '',
    });
    urlRow.appendChild(urlInput);
    section.appendChild(urlRow);
  }

  section.appendChild(statusLine);

  // ── Model row (hidden until key confirmed) ───────────────────────────────────
  const modelRow = el('div', { class: 'settings-row settings-row--compact', style: { display: 'none' } });
  modelRow.appendChild(el('div', { class: 'settings-row__label' }, 'Model'));
  const modelSelect = el('select', { class: 'settings-select' });
  modelRow.appendChild(modelSelect);
  section.appendChild(modelRow);

  // ── Extended context toggle (hidden until model supports it) ─────────────────
  const ctxRow = el('div', { class: 'settings-row settings-row--compact', style: { display: 'none' } });
  const ctxInfo = el('div', { class: 'settings-row__info' });
  ctxInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Extended Context (1M tokens)'));
  ctxInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Enable 1M token context window for supported models'));
  ctxRow.appendChild(ctxInfo);
  const ctxLabel = el('label', { class: 'settings-toggle' });
  const ctxCheckbox = el('input', { type: 'checkbox' });
  ctxCheckbox.checked = saved.largeContext || false;
  ctxLabel.appendChild(ctxCheckbox);
  ctxLabel.appendChild(el('span', { class: 'settings-toggle__slider' }));
  ctxRow.appendChild(ctxLabel);
  section.appendChild(ctxRow);

  // ── Auto-save helper ─────────────────────────────────────────────────────────
  async function persist() {
    const key = keyInput.value.trim();
    const model = modelSelect.value;
    const base = urlInput ? urlInput.value.trim() || null : null;
    const largeContext = ctxCheckbox.checked;
    if (!key || !model) return;

    const allConfigs = loadProviderConfigs();
    allConfigs[providerId] = { apiKey: key, model, baseUrl: base, largeContext };
    saveProviderConfigs(allConfigs);

    try {
      await api.setAiProvider(providerId, key, model, base);
      await updateSetting('ai.max_tokens', maxOutputFor(model));
    } catch (e) {
      console.error('Failed to persist provider config:', e);
    }
  }

  // ── Model change → auto-save ─────────────────────────────────────────────────
  function syncContextToggle() {
    const show = modelSelect.value && supportsLargeContext(modelSelect.value);
    ctxRow.style.display = show ? '' : 'none';
  }

  modelSelect.addEventListener('change', () => {
    syncContextToggle();
    persist();
  });

  ctxCheckbox.addEventListener('change', persist);

  // ── Populate model dropdown ──────────────────────────────────────────────────
  async function loadModels(key, base, prevModel, silent = false) {
    console.log(`[ai-settings] loadModels() called — provider=${providerId} silent=${silent} prevModel=${prevModel}`);
    console.trace('[ai-settings] loadModels call stack');

    if (!silent) setStatus('Connecting…', '');
    confirmBtn.disabled = true;

    try {
      const models = await fetchModelsFromApi(providerId, key, base);
      console.log(`[ai-settings] fetchModelsFromApi returned ${models.length} models for ${providerId}`);

      if (models.length === 0) {
        setStatus('No models returned', 'error');
        confirmBtn.disabled = false;
        return;
      }

      modelSelect.innerHTML = '';
      for (const m of models) {
        const opt = el('option', { value: m }, m);
        if (m === prevModel) opt.selected = true;
        modelSelect.appendChild(opt);
      }
      modelRow.style.display = '';
      setStatus('', '');
      syncContextToggle();
      // silent=true means called from auto-load on mount; persist() would re-trigger
      // a settings store update → re-render → infinite loop. Skip it here.
      if (!silent) persist();
    } catch (e) {
      console.error(`[ai-settings] loadModels error:`, e);
      setStatus(`Error: ${e.message}`, 'error');
    }
    confirmBtn.disabled = false;
  }

  function setStatus(msg, type) {
    statusLine.textContent = msg;
    statusLine.className = 'ai-status-line' + (type ? ` ai-status-line--${type}` : '');
  }

  // ── Confirm (✓) ─────────────────────────────────────────────────────────────
  confirmBtn.addEventListener('click', () => {
    const key = keyInput.value.trim();
    if (!key) { setStatus('Enter an API key first', 'error'); return; }
    const base = urlInput ? urlInput.value.trim() : null;
    if (providerId === 'Compatible' && !base) { setStatus('Enter Base URL first', 'error'); return; }
    const prevModel = modelSelect.value || saved.model || '';
    loadModels(key, base, prevModel);
  });

  // ── Clear (✗) ────────────────────────────────────────────────────────────────
  clearBtn.addEventListener('click', () => {
    keyInput.value = '';
    if (urlInput) urlInput.value = '';
    modelRow.style.display = 'none';
    ctxRow.style.display = 'none';
    modelSelect.innerHTML = '';
    setStatus('', '');

    const allConfigs = loadProviderConfigs();
    delete allConfigs[providerId];
    saveProviderConfigs(allConfigs);
  });

  // ── On load: if we have a saved key, auto-fetch models ───────────────────────
  if (saved.apiKey) {
    console.log(`[ai-settings] buildProviderSection(${providerId}) — saved key found, auto-loading models`);
    const base = urlInput ? (saved.baseUrl || '') : null;
    loadModels(saved.apiKey, base, saved.model || '', true); // silent=true to avoid re-render loop
  }

  return section;
}

export function createAiSettings(settings) {
  console.log('[ai-settings] createAiSettings() called — default_provider:', settings.ai?.default_provider);
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'AI Providers'));

  // Provider selector
  const providerRow = el('div', { class: 'settings-row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, 'Provider'));
  info.appendChild(el('div', { class: 'settings-row__desc' }, 'Active AI provider used for agent tasks'));
  providerRow.appendChild(info);

  const providerSelect = el('select', { class: 'settings-select' });
  providerSelect.appendChild(el('option', { value: '' }, '— None —'));
  for (const p of PROVIDERS) {
    const opt = el('option', { value: p.id }, p.label);
    if (p.id === (settings.ai?.default_provider || '')) opt.selected = true;
    providerSelect.appendChild(opt);
  }
  providerRow.appendChild(providerSelect);
  container.appendChild(providerRow);

  // Active provider config area
  const configArea = el('div', { class: 'ai-provider-config' });
  container.appendChild(configArea);

  function renderConfig(providerId) {
    configArea.innerHTML = '';
    if (!providerId) return;
    configArea.appendChild(buildProviderSection(providerId));
  }

  providerSelect.addEventListener('change', () => {
    const val = providerSelect.value || null;
    updateSetting('ai.default_provider', val);
    renderConfig(val);
  });

  renderConfig(settings.ai?.default_provider || '');

  return container;
}
