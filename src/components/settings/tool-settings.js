import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

const STORAGE_KEY = 'rustic_tool_config';

const BACKEND_OPTIONS = [
  { value: 'Tavily', label: 'Tavily' },
  { value: 'Brave',  label: 'Brave Search' },
  { value: 'Mcp',    label: 'Tavily MCP (defer to MCP server)' },
];

const DEFAULT_CONFIG = {
  web_search: { enabled: false, backend: 'Tavily', api_key: '' },
  web_fetch:  { enabled: true },
};

function loadLocal() {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null');
    if (!raw) return structuredClone(DEFAULT_CONFIG);
    return {
      web_search: { ...DEFAULT_CONFIG.web_search, ...(raw.web_search || {}) },
      web_fetch:  { ...DEFAULT_CONFIG.web_fetch,  ...(raw.web_fetch  || {}) },
    };
  } catch {
    return structuredClone(DEFAULT_CONFIG);
  }
}

function saveLocal(cfg) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
}

async function pushToBackend(cfg) {
  try {
    await api.setToolConfig(cfg);
  } catch (e) {
    console.warn('[tool-settings] setToolConfig failed:', e);
  }
}

export function createToolSettings() {
  const container = el('div', { class: 'tool-settings' });
  const state = loadLocal();

  // Fetch authoritative state from the backend on mount so restarts don't
  // drop the user's settings if localStorage was cleared.
  api.getToolConfig().then((server) => {
    if (!server) return;
    if (server.web_search) Object.assign(state.web_search, server.web_search);
    if (server.web_fetch)  Object.assign(state.web_fetch,  server.web_fetch);
    saveLocal(state);
    render();
  }).catch(() => {});

  const persist = () => {
    saveLocal(state);
    pushToBackend(state);
  };

  // ── Web Search row ─────────────────────────────────────────────────────────
  const searchCard = el('div', { class: 'tool-settings__card' });

  const searchHeader = el('div', { class: 'tool-settings__row' });
  const searchLabel = el('div', { class: 'tool-settings__label-block' });
  searchLabel.appendChild(el('div', { class: 'tool-settings__label' }, 'Web Search'));
  searchLabel.appendChild(el('div', { class: 'tool-settings__desc' },
    'Lets the agent run web search queries. Turning the toggle on is all you need for Anthropic and Gemini — they run it server-side for free. The backend below is only used when you switch to OpenAI or an OpenAI-compatible provider.'));
  searchHeader.appendChild(searchLabel);

  const searchToggle = el('label', { class: 'settings-toggle' });
  const searchCheckbox = el('input', { type: 'checkbox' });
  searchCheckbox.checked = !!state.web_search.enabled;
  searchToggle.appendChild(searchCheckbox);
  searchToggle.appendChild(el('span', { class: 'settings-toggle__slider' }));
  searchHeader.appendChild(searchToggle);
  searchCard.appendChild(searchHeader);

  // Details shown when the toggle is on. The status banner makes clear that
  // Anthropic / Gemini need nothing more; the expandable subsection below
  // configures OpenAI / compatible providers.
  const searchDetails = el('div', { class: 'tool-settings__details' });

  const readyBanner = el('div', { class: 'tool-settings__banner tool-settings__banner--ready' },
    'Ready on Anthropic and Gemini — server-side, no key needed.');
  searchDetails.appendChild(readyBanner);

  const fallbackWrap = el('details', { class: 'tool-settings__sub' });
  const fallbackSummary = el('summary', { class: 'tool-settings__sub-summary' },
    'Backend for OpenAI / OpenAI-compatible providers (optional)');
  fallbackWrap.appendChild(fallbackSummary);

  const backendRow = el('div', { class: 'tool-settings__field' });
  backendRow.appendChild(el('div', { class: 'tool-settings__field-label' }, 'Backend'));
  const backendSelect = el('select', { class: 'settings-select' });
  for (const opt of BACKEND_OPTIONS) {
    const o = el('option', { value: opt.value }, opt.label);
    if (opt.value === state.web_search.backend) o.selected = true;
    backendSelect.appendChild(o);
  }
  backendSelect.addEventListener('change', () => {
    state.web_search.backend = backendSelect.value;
    renderKeyRow();
    persist();
  });
  backendRow.appendChild(backendSelect);
  fallbackWrap.appendChild(backendRow);

  const keyRow = el('div', { class: 'tool-settings__field' });
  fallbackWrap.appendChild(keyRow);
  searchDetails.appendChild(fallbackWrap);

  function renderKeyRow() {
    keyRow.innerHTML = '';
    if (state.web_search.backend === 'Mcp') {
      keyRow.appendChild(el('div', { class: 'tool-settings__hint' },
        'With Tavily MCP selected, no built-in web_search tool is registered for non-Anthropic/Gemini providers. Configure the Tavily MCP server under MCP Servers.'));
      return;
    }
    keyRow.appendChild(el('div', { class: 'tool-settings__field-label' }, 'API Key'));
    const keyInput = el('input', {
      class: 'settings-input',
      type: 'password',
      placeholder: state.web_search.backend === 'Tavily' ? 'tvly-…' : 'brave api key',
      value: state.web_search.api_key || '',
    });
    keyInput.addEventListener('change', () => {
      state.web_search.api_key = keyInput.value.trim();
      persist();
    });
    keyRow.appendChild(keyInput);

    const hintText = state.web_search.backend === 'Tavily'
      ? 'Only needed if you use OpenAI or a compatible provider. Sign up at tavily.com — free tier included.'
      : 'Only needed if you use OpenAI or a compatible provider. Free tier at api.search.brave.com.';
    keyRow.appendChild(el('div', { class: 'tool-settings__hint' }, hintText));
  }

  searchCheckbox.addEventListener('change', () => {
    state.web_search.enabled = searchCheckbox.checked;
    searchDetails.style.display = searchCheckbox.checked ? '' : 'none';
    persist();
  });
  searchDetails.style.display = searchCheckbox.checked ? '' : 'none';
  renderKeyRow();

  searchCard.appendChild(searchDetails);
  container.appendChild(searchCard);

  // ── Web Fetch row ──────────────────────────────────────────────────────────
  const fetchCard = el('div', { class: 'tool-settings__card' });
  const fetchRow = el('div', { class: 'tool-settings__row' });
  const fetchLabel = el('div', { class: 'tool-settings__label-block' });
  fetchLabel.appendChild(el('div', { class: 'tool-settings__label' }, 'Web Fetch'));
  fetchLabel.appendChild(el('div', { class: 'tool-settings__desc' },
    'Lets the agent fetch and summarize a URL. Anthropic and Gemini run this server-side; other providers download the page locally and summarize with your currently selected model.'));
  fetchRow.appendChild(fetchLabel);

  const fetchToggle = el('label', { class: 'settings-toggle' });
  const fetchCheckbox = el('input', { type: 'checkbox' });
  fetchCheckbox.checked = !!state.web_fetch.enabled;
  fetchToggle.appendChild(fetchCheckbox);
  fetchToggle.appendChild(el('span', { class: 'settings-toggle__slider' }));
  fetchRow.appendChild(fetchToggle);
  fetchCard.appendChild(fetchRow);
  container.appendChild(fetchCard);

  fetchCheckbox.addEventListener('change', () => {
    state.web_fetch.enabled = fetchCheckbox.checked;
    persist();
  });

  // Re-render on programmatic state refresh (after backend fetch).
  function render() {
    searchCheckbox.checked = !!state.web_search.enabled;
    searchDetails.style.display = searchCheckbox.checked ? '' : 'none';
    for (const opt of backendSelect.options) {
      opt.selected = opt.value === state.web_search.backend;
    }
    renderKeyRow();
    fetchCheckbox.checked = !!state.web_fetch.enabled;
  }

  return container;
}
