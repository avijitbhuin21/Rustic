import { el } from '../../utils/dom.js';
import { createCombobox } from '../../utils/combobox.js';
import * as api from '../../lib/tauri-api.js';

const STORAGE_KEY = 'rustic_tool_config';

const BACKEND_OPTIONS = [
  { value: 'Tavily', label: 'Tavily' },
  { value: 'Brave',  label: 'Brave Search' },
  { value: 'Mcp',    label: 'Tavily MCP (defer to MCP server)' },
];

const DEFAULT_MEDIA_ENTRY = { provider_key: '', model: '', max_per_call: 1 };
const DEFAULT_CONFIG = {
  web_search: { enabled: false, backend: 'Tavily', api_key: '' },
  web_fetch:  { enabled: true },
  media: {
    image:   { ...DEFAULT_MEDIA_ENTRY },
    video:   { ...DEFAULT_MEDIA_ENTRY },
    animate: { ...DEFAULT_MEDIA_ENTRY },
    link_animate_to_video: false,
  },
};

function loadLocal() {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null');
    if (!raw) return structuredClone(DEFAULT_CONFIG);
    const media = raw.media || {};
    return {
      web_search: { ...DEFAULT_CONFIG.web_search, ...(raw.web_search || {}) },
      web_fetch:  { ...DEFAULT_CONFIG.web_fetch,  ...(raw.web_fetch  || {}) },
      media: {
        image:   { ...DEFAULT_MEDIA_ENTRY, ...(media.image   || {}) },
        video:   { ...DEFAULT_MEDIA_ENTRY, ...(media.video   || {}) },
        animate: { ...DEFAULT_MEDIA_ENTRY, ...(media.animate || {}) },
        link_animate_to_video: !!media.link_animate_to_video,
      },
    };
  } catch {
    return structuredClone(DEFAULT_CONFIG);
  }
}

function saveLocal(cfg) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
}

// Mirror of backend ProviderEntry::provider_key() so the dropdown values
// match the strings the tool dispatcher uses when looking the provider up.
function slugifyName(name) {
  return String(name || '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}
function providerKeyOf(p) {
  const t = p.provider_type;
  if (t === 'Compatible') {
    const slug = slugifyName(p.name);
    return slug ? `Compatible:${slug}` : 'Compatible';
  }
  return t;
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
    if (server.media) {
      const m = server.media;
      if (m.image)   Object.assign(state.media.image,   m.image);
      if (m.video)   Object.assign(state.media.video,   m.video);
      if (m.animate) Object.assign(state.media.animate, m.animate);
      if (typeof m.link_animate_to_video === 'boolean') {
        state.media.link_animate_to_video = m.link_animate_to_video;
      }
    }
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
    'Lets the agent run web search queries. Anthropic, Gemini, and OpenAI GPT-5 (via the Responses API) run it server-side for free — no extra key needed. The backend below is only used for OpenAI Chat Completions models, OpenAI-compatible providers, and OpenRouter.'));
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
    'Ready on Anthropic, Gemini, and OpenAI GPT-5 — server-side, no key needed.');
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

  // ── Media tools section ───────────────────────────────────────────────────
  // Three independent tool cards under a single section header. Providers are
  // always selectable (OpenAI / Gemini / OpenRouter) even when the user
  // hasn't registered them yet — the row surfaces a "not configured" hint
  // and a link back to AI Providers, so the user knows what's missing.
  const mediaSection = el('div', { class: 'tool-settings__section' });
  mediaSection.appendChild(el('div', { class: 'tool-settings__section-title' }, 'Media tools'));
  mediaSection.appendChild(el('div', { class: 'tool-settings__section-desc' },
    'Let the agent generate images, videos, and animations. Each tool is enabled when a provider and model are picked. Outputs are saved under .rustic/generated/ inside the project.'));
  container.appendChild(mediaSection);

  // Always-visible provider options. The actual API key + base URL come
  // from the matching ProviderEntry in AI Providers settings.
  const PROVIDER_OPTIONS_ALL = [
    { key: 'OpenAi',     label: 'OpenAI' },
    { key: 'Gemini',     label: 'Google Gemini' },
    { key: 'OpenRouter', label: 'OpenRouter' },
  ];

  // Per-provider live model lists. Populated lazily when the user picks a
  // provider — we hit the provider's /v1/models endpoint via fetchAiModels
  // with include_all=true so image-gen and video-gen models are included
  // alongside chat models. The user is trusted to pick something sensible.
  const liveModelsByProvider = {}; // providerKey -> { state: 'idle'|'loading'|'ready'|'error', models: [...], error: '' }

  /// Fetch the full live model list for a provider and refresh every tool
  /// row that uses it. Caches per session — call with force=true to re-hit
  /// the provider's API (e.g. after the user adds a new key).
  async function loadProviderModels(providerKey, force = false) {
    if (!providerKey) return;
    const cached = liveModelsByProvider[providerKey];
    if (cached && cached.state === 'ready' && !force) return;
    liveModelsByProvider[providerKey] = { state: 'loading', models: [] };
    refreshAllModelDropdowns();
    try {
      const models = await api.fetchAiModels(providerKey, '__STORED__', null, force, true);
      liveModelsByProvider[providerKey] = {
        state: 'ready',
        models: Array.isArray(models) ? models : [],
      };
    } catch (e) {
      liveModelsByProvider[providerKey] = {
        state: 'error',
        models: [],
        error: String(e?.message || e || '').slice(0, 120),
      };
    }
    refreshAllModelDropdowns();
  }

  function refreshAllModelDropdowns() {
    for (const def of TOOL_DEFS) {
      const refresh = toolRows[def.key + '__refreshModel'];
      if (refresh) refresh();
    }
  }

  const TOOL_DEFS = [
    {
      key: 'image',
      title: 'Image creator',
      toolName: 'image_create',
      hint: 'Suggested: OpenAI gpt-image-1 · Gemini gemini-2.5-flash-image · OpenRouter google/gemini-2.5-flash-image-preview',
      placeholder: 'gpt-image-1',
      max_cap: 10,
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
    },
    {
      key: 'video',
      title: 'Video creator',
      toolName: 'video_create',
      hint: 'Suggested: OpenAI sora-2 · Gemini veo-3.1-generate-preview · OpenRouter google/veo-3.1, openai/sora-2-pro, bytedance/seedance-2.0.',
      placeholder: 'sora-2',
      max_cap: 4,
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
    },
    {
      key: 'animate',
      title: 'Animator (image → video)',
      toolName: 'animate',
      hint: 'Animates an existing project image. Uses the same model family as video_create — toggle the switch below to reuse that configuration.',
      placeholder: 'veo-3.1-generate-preview',
      max_cap: 4,
      providers: ['OpenAi', 'Gemini', 'OpenRouter'],
    },
  ];

  let configuredProviderKeys = new Set(); // populated by getAiConfig()
  const toolRows = {}; // key → { card, providerCombo, providerWrap, modelCombo, maxInput, statusEl, fields }

  function refreshToolRow(toolDef) {
    const row = toolRows[toolDef.key];
    if (!row) return;
    // The combobox option list is computed lazily via getOptions(), so the
    // "(not configured)" hints will pick up the latest configuredProviderKeys
    // on the next open. We still call refresh() so the displayed label
    // updates when the dropdown is closed.
    if (row.providerCombo) row.providerCombo.refresh();
    const picked = state.media[toolDef.key].provider_key;
    const isConfigured = picked && configuredProviderKeys.has(picked);
    if (!picked) {
      row.statusEl.textContent = 'Pick a provider above to enable this tool.';
      row.statusEl.classList.remove('tool-settings__media-status--warn');
    } else if (!isConfigured) {
      row.statusEl.textContent = `${picked} has no API key yet — open Settings → AI Providers to add one.`;
      row.statusEl.classList.add('tool-settings__media-status--warn');
    } else {
      row.statusEl.textContent = `Ready: using ${picked}.`;
      row.statusEl.classList.remove('tool-settings__media-status--warn');
    }
  }

  function buildToolCard(toolDef) {
    const card = el('div', { class: 'tool-settings__card tool-settings__media-card' });

    const header = el('div', { class: 'tool-settings__media-header' });
    header.appendChild(el('div', { class: 'tool-settings__label' }, toolDef.title));
    header.appendChild(el('div', { class: 'tool-settings__media-toolname' }, toolDef.toolName));
    card.appendChild(header);
    card.appendChild(el('div', { class: 'tool-settings__desc' }, toolDef.hint));

    // Provider combobox — small list, but searchable for consistency with
    // the model picker. Built via getOptions() so the "(not configured)"
    // hint can refresh after getAiConfig() resolves.
    const providerWrap = el('div', { class: 'tool-settings__field' });
    providerWrap.appendChild(el('div', { class: 'tool-settings__field-label' }, 'Provider'));
    const providerCombo = createCombobox({
      initialValue: state.media[toolDef.key].provider_key,
      placeholder: 'Search providers…',
      allowCustom: false,
      getOptions: () => {
        const opts = [];
        for (const p of PROVIDER_OPTIONS_ALL) {
          if (!toolDef.providers.includes(p.key)) continue;
          const configured = configuredProviderKeys.has(p.key);
          opts.push({
            value: p.key,
            label: p.label,
            hint: configured ? '' : 'not configured',
          });
        }
        return opts;
      },
      onChange: (newKey) => {
        state.media[toolDef.key].provider_key = newKey;
        state.media[toolDef.key].model = '';
        persist();
        refreshToolRow(toolDef);
        const refresh = toolRows[toolDef.key + '__refreshModel'];
        if (refresh) refresh();
        if (newKey && configuredProviderKeys.has(newKey)) {
          loadProviderModels(newKey);
        }
      },
    });
    providerWrap.appendChild(providerCombo.root);
    card.appendChild(providerWrap);

    const statusEl = el('div', { class: 'tool-settings__media-status' });
    card.appendChild(statusEl);

    // Model + max-per-call live in a fields wrapper so the animate card can
    // hide them when the share-with-video toggle is on.
    const fields = el('div', { class: 'tool-settings__media-fields' });

    const modelWrap = el('div', { class: 'tool-settings__field' });
    modelWrap.appendChild(el('div', { class: 'tool-settings__field-label' }, 'Model'));
    // Searchable combobox: lists every model the provider's /v1/models call
    // returns (no chat-only filter), and accepts any free-typed string as a
    // custom model id so users aren't blocked when a new model ships.
    const modelCombo = createCombobox({
      initialValue: state.media[toolDef.key].model || '',
      placeholder: toolDef.placeholder + ' — type to search',
      allowCustom: true,
      getOptions: () => {
        const provider = state.media[toolDef.key].provider_key;
        if (!provider) {
          return [{ value: '', label: 'Pick a provider first', disabled: true, hint: '' }];
        }
        const entry = liveModelsByProvider[provider];
        if (!entry || entry.state === 'idle') return [];
        if (entry.state === 'loading') {
          return [{ value: '', label: 'Loading models…', disabled: true }];
        }
        if (entry.state === 'error') {
          return [{
            value: '',
            label: `Failed to load models — ${entry.error || 'check API key'}`,
            disabled: true,
          }];
        }
        return entry.models.map((id) => ({ value: id, label: id }));
      },
      onChange: (v) => {
        state.media[toolDef.key].model = v;
        persist();
      },
    });
    // Disable the model combobox until a provider is chosen.
    if (!state.media[toolDef.key].provider_key) modelCombo.setDisabled(true);

    const refreshModelSelect = () => {
      const provider = state.media[toolDef.key].provider_key;
      modelCombo.setDisabled(!provider);
      modelCombo.setValue(state.media[toolDef.key].model || '');
      modelCombo.refresh();
    };

    modelWrap.appendChild(modelCombo.root);
    fields.appendChild(modelWrap);

    // Stash refresh handle so the provider-change / model-load handlers can poke it.
    toolRows[toolDef.key + '__refreshModel'] = refreshModelSelect;

    const maxWrap = el('div', { class: 'tool-settings__field' });
    maxWrap.appendChild(el('div', { class: 'tool-settings__field-label' },
      `Max per call (1–${toolDef.max_cap})`));
    const maxInput = el('input', {
      class: 'settings-input',
      type: 'number',
      min: '1',
      max: String(toolDef.max_cap),
      value: String(state.media[toolDef.key].max_per_call || 1),
    });
    maxInput.addEventListener('change', () => {
      let v = parseInt(maxInput.value, 10);
      if (!Number.isFinite(v) || v < 1) v = 1;
      if (v > toolDef.max_cap) v = toolDef.max_cap;
      maxInput.value = String(v);
      state.media[toolDef.key].max_per_call = v;
      persist();
    });
    maxWrap.appendChild(maxInput);
    fields.appendChild(maxWrap);

    card.appendChild(fields);

    toolRows[toolDef.key] = {
      card,
      providerCombo,
      providerWrap,
      modelCombo,
      maxInput,
      statusEl,
      fields,
    };
    return card;
  }

  // Build image + video cards normally.
  container.appendChild(buildToolCard(TOOL_DEFS[0]));
  container.appendChild(buildToolCard(TOOL_DEFS[1]));

  // Animate card with link-to-video toggle.
  const animateCard = buildToolCard(TOOL_DEFS[2]);

  // Insert toggle row at the top of the animate card body.
  const linkRow = el('div', { class: 'tool-settings__media-link-row' });
  const linkLabel = el('div', { class: 'tool-settings__label-block' });
  linkLabel.appendChild(el('div', { class: 'tool-settings__field-label' }, 'Use video_create configuration for animation'));
  linkLabel.appendChild(el('div', { class: 'tool-settings__hint' },
    'When on, the animate tool reuses the Video creator\'s provider and model. Turn this off to pick a separate model for animations.'));
  linkRow.appendChild(linkLabel);
  const linkToggle = el('label', { class: 'settings-toggle' });
  const linkCheckbox = el('input', { type: 'checkbox' });
  linkCheckbox.checked = !!state.media.link_animate_to_video;
  linkToggle.appendChild(linkCheckbox);
  linkToggle.appendChild(el('span', { class: 'settings-toggle__slider' }));
  linkRow.appendChild(linkToggle);
  animateCard.insertBefore(linkRow, animateCard.children[2] || null);

  function applyLinkState() {
    const linked = !!state.media.link_animate_to_video;
    const row = toolRows.animate;
    if (!row) return;
    // Hide animate's own provider + fields when linked.
    if (row.providerWrap) row.providerWrap.style.display = linked ? 'none' : '';
    row.fields.style.display = linked ? 'none' : '';
    if (linked) {
      row.statusEl.textContent = 'Linked to Video creator — using whatever you set above.';
      row.statusEl.classList.remove('tool-settings__media-status--warn');
    } else {
      refreshToolRow(TOOL_DEFS[2]);
    }
  }
  linkCheckbox.addEventListener('change', () => {
    state.media.link_animate_to_video = linkCheckbox.checked;
    persist();
    applyLinkState();
  });

  container.appendChild(animateCard);

  // Initial status pass.
  for (const def of TOOL_DEFS) refreshToolRow(def);
  applyLinkState();

  // Pull the user's actual configured providers so we can mark which
  // dropdown options are "ready" vs "not configured". This is a hint
  // only — the dropdown stays fully selectable either way.
  api.getAiConfig().then((cfg) => {
    if (!cfg || !Array.isArray(cfg.providers)) return;
    configuredProviderKeys = new Set();
    for (const p of cfg.providers) {
      const k = providerKeyOf(p);
      // Only treat a provider as "configured" when an api key is actually
      // stored (the backend redacts to "__STORED__" when present).
      const hasKey = (p.api_key && p.api_key !== '') || p.api_key === '__STORED__';
      if (hasKey) configuredProviderKeys.add(k);
    }
    for (const def of TOOL_DEFS) refreshToolRow(def);
    if (state.media.link_animate_to_video) applyLinkState();

    // Pre-load the live model list for any provider already picked across
    // the three tools so the dropdown is populated as soon as the user opens
    // it. Deduped via the in-memory cache so we hit each provider at most
    // once per settings session.
    const picked = new Set();
    for (const def of TOOL_DEFS) {
      const k = state.media[def.key].provider_key;
      if (k && configuredProviderKeys.has(k)) picked.add(k);
    }
    for (const k of picked) loadProviderModels(k);
  }).catch(() => {});

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
