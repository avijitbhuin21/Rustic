import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

const PROVIDERS = [
  { id: 'Claude',     label: 'Anthropic',        placeholder: 'sk-ant-…',        largeContextSupport: true  },
  { id: 'OpenAi',    label: 'OpenAI',             placeholder: 'sk-…',            largeContextSupport: false },
  { id: 'Gemini',    label: 'Google Gemini',      placeholder: 'AIza…',           largeContextSupport: true  },
  { id: 'Compatible',label: 'OpenAI-Compatible',  placeholder: 'API key (if any)', largeContextSupport: false },
];

const MODEL_MAX_OUTPUT = {
  'claude-opus-4-20250514': 32000, 'claude-sonnet-4-20250514': 64000, 'claude-haiku-4-20250307': 16000,
  'gpt-4o': 16384, 'gpt-4o-mini': 16384, 'o1': 100000, 'o1-mini': 65536, 'o3': 100000, 'o3-mini': 100000,
  'gemini-2.5-pro': 65536, 'gemini-2.5-flash': 65536, 'gemini-2.0-flash': 65536, 'gemini-1.5-pro': 8192,
};

function maxOutputFor(modelId) {
  if (MODEL_MAX_OUTPUT[modelId]) return MODEL_MAX_OUTPUT[modelId];
  for (const [k, v] of Object.entries(MODEL_MAX_OUTPUT)) { if (modelId.startsWith(k)) return v; }
  return 8192;
}

export function loadProviderConfigs() {
  try { return JSON.parse(localStorage.getItem('rustic_provider_configs') || '{}'); }
  catch { return {}; }
}

function saveProviderConfigs(configs) {
  localStorage.setItem('rustic_provider_configs', JSON.stringify(configs));
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

function buildProviderCard(providerId) {
  const p = PROVIDERS.find(x => x.id === providerId);
  const configs = loadProviderConfigs();
  const saved = configs[providerId] || {};
  const isConnected = !!(saved.apiKey && saved.models?.length);

  const card = el('div', { class: `ai-provider-card${isConnected ? ' ai-provider-card--connected' : ''}` });

  // ── Header (always visible) ──────────────────────────────────────────────────
  const cardHeader = el('div', { class: 'ai-provider-card__header' });
  const headerLeft = el('div', { class: 'ai-provider-card__header-left' });
  const statusDot = el('span', { class: `ai-provider-card__dot${isConnected ? ' ai-provider-card__dot--on' : ''}` });
  const nameEl = el('span', { class: 'ai-provider-card__name' }, p.label);
  headerLeft.appendChild(statusDot);
  headerLeft.appendChild(nameEl);

  const modelCountEl = el('span', { class: 'ai-provider-card__model-count' });
  if (isConnected) modelCountEl.textContent = `${saved.models.length} models`;
  headerLeft.appendChild(modelCountEl);

  cardHeader.appendChild(headerLeft);

  // Right side: context toggle (Claude & Gemini only) + trash
  const headerRight = el('div', { class: 'ai-provider-card__header-right' });

  // 1M context toggle — only for providers that support it, only shown when connected
  let largeContextEnabled = saved.largeContext || false;
  if (p.largeContextSupport) {
    const ctxToggleWrap = el('label', {
      class: 'ai-context-toggle',
      title: 'Use 1M token context window instead of 200k',
      style: isConnected ? '' : 'display:none',
    });
    const ctxCheckbox = el('input', { type: 'checkbox' });
    ctxCheckbox.checked = largeContextEnabled;
    const ctxSlider = el('span', { class: 'ai-context-toggle__slider' });
    const ctxLabel = el('span', { class: 'ai-context-toggle__label' }, '1M ctx');
    ctxToggleWrap.appendChild(ctxCheckbox);
    ctxToggleWrap.appendChild(ctxSlider);
    ctxToggleWrap.appendChild(ctxLabel);

    ctxCheckbox.addEventListener('change', async () => {
      largeContextEnabled = ctxCheckbox.checked;
      const allConfigs = loadProviderConfigs();
      if (allConfigs[providerId]) {
        allConfigs[providerId].largeContext = largeContextEnabled;
        saveProviderConfigs(allConfigs);
      }
      const cfg = allConfigs[providerId] || {};
      const base = urlInput ? cfg.baseUrl || null : null;
      await api.setAiProvider(providerId, cfg.apiKey, cfg.model, base, largeContextEnabled).catch(() => {});
    });

    headerRight.appendChild(ctxToggleWrap);

    // Store reference so enterConnectedState / enterEditState can show/hide it
    card._ctxToggleWrap = ctxToggleWrap;
  }

  // Clear (trash) button
  const clearBtn = el('button', { class: 'ai-clear-btn', title: 'Remove connection', style: isConnected ? '' : 'display:none' });
  clearBtn.appendChild(icon('M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2', 13));
  headerRight.appendChild(clearBtn);
  cardHeader.appendChild(headerRight);
  card.appendChild(cardHeader);

  // ── Edit area (visible when not connected) ───────────────────────────────────
  const editArea = el('div', { class: 'ai-provider-card__edit', style: isConnected ? 'display:none' : '' });

  // Base URL row (Compatible only)
  let urlInput = null;
  if (providerId === 'Compatible') {
    const urlRow = el('div', { class: 'ai-provider-card__row' });
    urlRow.appendChild(el('span', { class: 'ai-provider-card__row-label' }, 'Base URL'));
    urlInput = el('input', {
      class: 'settings-input',
      type: 'text',
      placeholder: 'Base URL e.g. https://api.groq.com/openai/v1',
      value: saved.baseUrl || '',
    });
    urlRow.appendChild(urlInput);
    editArea.appendChild(urlRow);
  }

  // API key row
  const keyRow = el('div', { class: 'ai-provider-card__row' });

  const keyInput = el('input', {
    class: 'settings-input ai-key-input',
    type: 'password',
    placeholder: p.placeholder,
    value: '',
  });

  // Eye toggle
  const eyeBtn = el('button', { class: 'ai-eye-btn', title: 'Show / hide key' });
  let keyVisible = false;
  eyeBtn.appendChild(icon('M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z M12 12m-3 0a3 3 0 1 0 6 0a3 3 0 1 0 -6 0', 13));
  eyeBtn.addEventListener('click', () => {
    keyVisible = !keyVisible;
    keyInput.type = keyVisible ? 'text' : 'password';
  });

  const connectBtn = el('button', { class: 'ai-connect-btn' }, 'Connect');

  keyRow.appendChild(keyInput);
  keyRow.appendChild(eyeBtn);
  keyRow.appendChild(connectBtn);
  editArea.appendChild(keyRow);

  // Status line (inside edit area)
  const statusLine = el('div', { class: 'ai-status-line' });
  editArea.appendChild(statusLine);

  card.appendChild(editArea);

  // ── Helpers ──────────────────────────────────────────────────────────────────
  function setStatus(msg, type) {
    statusLine.textContent = msg;
    statusLine.className = 'ai-status-line' + (type ? ` ai-status-line--${type}` : '');
  }

  function enterConnectedState(models) {
    card.classList.add('ai-provider-card--connected');
    statusDot.classList.add('ai-provider-card__dot--on');
    modelCountEl.textContent = `${models.length} models`;
    clearBtn.style.display = '';
    editArea.style.display = 'none';
    if (card._ctxToggleWrap) card._ctxToggleWrap.style.display = '';
  }

  function enterEditState() {
    card.classList.remove('ai-provider-card--connected');
    statusDot.classList.remove('ai-provider-card__dot--on');
    modelCountEl.textContent = '';
    clearBtn.style.display = 'none';
    editArea.style.display = '';
    if (card._ctxToggleWrap) card._ctxToggleWrap.style.display = 'none';
    keyInput.value = '';
    keyInput.type = 'password';
    keyVisible = false;
    setStatus('', '');
  }

  // ── Connect ──────────────────────────────────────────────────────────────────
  connectBtn.addEventListener('click', async () => {
    const key = keyInput.value.trim();
    if (!key) { setStatus('Enter an API key first', 'error'); return; }
    const base = urlInput ? urlInput.value.trim() || null : null;
    if (providerId === 'Compatible' && !base) { setStatus('Enter Base URL first', 'error'); return; }

    setStatus('Connecting…', '');
    connectBtn.disabled = true;

    try {
      const models = await api.fetchAiModels(providerId, key, base || null);
      if (!models?.length) { setStatus('No models returned — check your API key', 'error'); connectBtn.disabled = false; return; }

      const defaultModel = models[0];

      // Persist to localStorage
      const allConfigs = loadProviderConfigs();
      allConfigs[providerId] = { apiKey: key, model: defaultModel, models, baseUrl: base, largeContext: largeContextEnabled };
      saveProviderConfigs(allConfigs);

      // Register with backend
      await api.setAiProvider(providerId, key, defaultModel, base, largeContextEnabled);

      enterConnectedState(models);
      showToast(`Connected to ${p.label} — ${models.length} model${models.length !== 1 ? 's' : ''} available`);
    } catch (e) {
      setStatus(`Error: ${e.message || e}`, 'error');
    }

    connectBtn.disabled = false;
  });

  // ── Clear ────────────────────────────────────────────────────────────────────
  clearBtn.addEventListener('click', () => {
    const allConfigs = loadProviderConfigs();
    delete allConfigs[providerId];
    saveProviderConfigs(allConfigs);
    enterEditState();
  });

  // ── On mount: re-register saved key with backend silently ───────────────────
  if (isConnected) {
    const base = urlInput ? (saved.baseUrl || null) : null;
    api.setAiProvider(providerId, saved.apiKey, saved.model || saved.models[0], base, saved.largeContext || false).catch(() => {});
  }

  return card;
}

export function createAiSettings() {
  const container = el('div', { class: 'ai-providers-container' });
  for (const p of PROVIDERS) container.appendChild(buildProviderCard(p.id));
  return container;
}
