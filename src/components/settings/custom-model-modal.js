import { el } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { saveCustomModel, getCustomModel, loadCustomModels } from '../../state/custom-models.js';
import { listKnownModels } from '../../lib/tauri-api.js';

const PROVIDERS = ['Claude', 'OpenAi', 'Gemini', 'Compatible'];

/**
 * Show a modal prompting the user to register specs for a model that isn't
 * in the built-in registry and hasn't been saved locally yet.
 *
 * @param {Object} opts
 * @param {string} opts.modelId          — the exact model id the user picked
 * @param {string} [opts.providerType]   — pre-filled + locked when present
 * @param {Function} [opts.onSaved]      — called with the saved spec
 * @param {Function} [opts.onCancelled]  — called if the user dismissed
 */
export function openCustomModelModal({ modelId, providerType = null, onSaved, onCancelled }) {
  const existing = getCustomModel(modelId) || {};

  const body = el('div', { class: 'skills-edit-form' });

  body.appendChild(el('div', {
    class: 'ai-status-line',
    style: 'margin-bottom: 10px;',
  }, `"${modelId}" isn't in the built-in model registry. Fill in its specs so cost and context-window calculations stay accurate.`));

  const nameInput = el('input', {
    class: 'rustic-modal__input',
    type: 'text',
    placeholder: modelId,
    value: existing.name || '',
  });

  const mkNum = (placeholder, value, step = '1') => el('input', {
    class: 'rustic-modal__input',
    type: 'number',
    step,
    placeholder,
    value: value != null ? String(value) : '',
  });

  const ctxWindowInput   = mkNum('e.g. 200000',  existing.contextWindow);
  const maxOutputInput   = mkNum('e.g. 64000',   existing.maxOutputTokens);
  const inputCostInput   = mkNum('$ per 1M tok', existing.inputCost,  '0.01');
  const outputCostInput  = mkNum('$ per 1M tok', existing.outputCost, '0.01');
  const cachedInCostIn   = mkNum('$ per 1M tok (optional)', existing.cachedInputCost,  '0.01');
  const cachedOutCostIn  = mkNum('$ per 1M tok (optional)', existing.cachedOutputCost, '0.01');

  // ── Template dropdown ─────────────────────────────────────────────
  // Same model offered by different OpenAI-compatible providers (Groq,
  // OpenRouter, DeepInfra, …) shares context window and prices. The dropdown
  // mixes two sources:
  //   1. User-saved custom models (most relevant — the user just registered them).
  //   2. The Rust-side built-in registry (Anthropic / OpenAI / Gemini) so the
  //      user can spin up "Claude Sonnet 4.6 specs but on a Compatible provider"
  //      without typing all the numbers themselves.
  //
  // Built-in models are loaded async; the dropdown re-renders when they arrive.
  const allTemplates = loadCustomModels();
  const userEntries = Object.entries(allTemplates)
    .filter(([id]) => id !== modelId)
    .sort(([, a], [, b]) => (b.savedAt || 0) - (a.savedAt || 0));

  const tmplLabel = el('label', { class: 'rustic-modal__label' }, 'Use template (optional)');
  const tmplSelect = el('select', { class: 'rustic-modal__input' });
  body.appendChild(tmplLabel);
  body.appendChild(tmplSelect);

  // Map of dropdown-option-value → the spec to apply on selection. Filled by
  // both the user-template loop and the (later-arriving) built-in fetch.
  const optionSpecs = new Map();

  function applyTemplateSpec(spec) {
    if (!spec) return;
    ctxWindowInput.value  = spec.contextWindow != null ? String(spec.contextWindow) : '';
    maxOutputInput.value  = spec.maxOutputTokens != null ? String(spec.maxOutputTokens) : '';
    inputCostInput.value  = spec.inputCost != null ? String(spec.inputCost) : '';
    outputCostInput.value = spec.outputCost != null ? String(spec.outputCost) : '';
    cachedInCostIn.value  = spec.cachedInputCost  ? String(spec.cachedInputCost)  : '';
    cachedOutCostIn.value = spec.cachedOutputCost ? String(spec.cachedOutputCost) : '';
    // Display Name is left alone — same model, new naming.
  }

  tmplSelect.addEventListener('change', () => {
    applyTemplateSpec(optionSpecs.get(tmplSelect.value));
  });

  function rebuildOptions(builtins) {
    tmplSelect.innerHTML = '';
    optionSpecs.clear();
    tmplSelect.appendChild(el('option', { value: '' }, '— start fresh —'));

    if (userEntries.length > 0) {
      const userGroup = el('optgroup', { label: 'Your saved templates' });
      for (const [id, spec] of userEntries) {
        const display = spec.name && spec.name !== id ? `${spec.name} — ${id}` : id;
        const suffix = spec.provider ? ` (${spec.provider})` : '';
        const key = `user:${id}`;
        userGroup.appendChild(el('option', { value: key }, `${display}${suffix}`));
        optionSpecs.set(key, spec);
      }
      tmplSelect.appendChild(userGroup);
    }

    if (Array.isArray(builtins) && builtins.length > 0) {
      // Group built-ins by provider so the user can scan within Anthropic /
      // OpenAI / Gemini independently.
      const byProvider = new Map();
      for (const m of builtins) {
        if (!byProvider.has(m.provider)) byProvider.set(m.provider, []);
        byProvider.get(m.provider).push(m);
      }
      const providerLabel = { Claude: 'Anthropic (Claude)', OpenAi: 'OpenAI', Gemini: 'Google Gemini' };
      for (const [provider, models] of byProvider) {
        const group = el('optgroup', { label: providerLabel[provider] || provider });
        for (const m of models) {
          const key = `builtin:${m.id}`;
          const opt = el('option', { value: key }, `${m.name} — ${m.id}`);
          group.appendChild(opt);
          optionSpecs.set(key, {
            contextWindow: m.context_window,
            maxOutputTokens: m.max_output_tokens,
            inputCost: m.input_cost_per_m,
            outputCost: m.output_cost_per_m,
            cachedInputCost: m.cache_read_cost_per_m,
            cachedOutputCost: m.cache_write_cost_per_m,
          });
        }
        tmplSelect.appendChild(group);
      }
    }

    if (userEntries.length === 0 && (!builtins || builtins.length === 0)) {
      // Truly empty — show the helpful placeholder.
      tmplSelect.innerHTML = '';
      const ph = el('option', { value: '', disabled: 'true', selected: 'true' },
        '— no templates available —');
      tmplSelect.appendChild(ph);
      tmplSelect.disabled = true;
      tmplSelect.style.opacity = '0.6';
    } else {
      tmplSelect.disabled = false;
      tmplSelect.style.opacity = '';
    }
  }

  // Initial render with no built-ins yet (just user templates if any).
  rebuildOptions(null);

  // Pull built-ins async. Backend rarely fails this; on error we silently
  // keep the user-template-only view.
  listKnownModels().then((builtins) => {
    rebuildOptions(builtins || []);
  }).catch(() => { /* keep current options */ });

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Display Name'));
  body.appendChild(nameInput);

  let providerInput;
  if (providerType) {
    providerInput = el('input', {
      class: 'rustic-modal__input',
      type: 'text',
      value: providerType,
      disabled: 'true',
      style: 'opacity: 0.7;',
    });
  } else {
    providerInput = el('select', { class: 'rustic-modal__input' });
    for (const p of PROVIDERS) {
      const opt = el('option', { value: p }, p);
      if (existing.provider === p) opt.selected = true;
      providerInput.appendChild(opt);
    }
  }
  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Provider'));
  body.appendChild(providerInput);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Context Window (tokens)'));
  body.appendChild(ctxWindowInput);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Max Output Tokens'));
  body.appendChild(maxOutputInput);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Input Cost ($/1M tokens)'));
  body.appendChild(inputCostInput);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Output Cost ($/1M tokens)'));
  body.appendChild(outputCostInput);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Cached Input Cost (optional)'));
  body.appendChild(cachedInCostIn);

  body.appendChild(el('label', { class: 'rustic-modal__label' }, 'Cached Output Cost (optional)'));
  body.appendChild(cachedOutCostIn);

  const err = el('div', { class: 'skills-install-form__status' });
  body.appendChild(err);

  let confirmed = false;

  openModal({
    title: 'Register model',
    body,
    size: '',
    buttons: [
      {
        label: 'Cancel',
        variant: 'secondary',
      },
      {
        label: 'Save',
        variant: 'primary',
        onClick: () => {
          const contextWindow  = parseInt(ctxWindowInput.value, 10);
          const maxOutput      = parseInt(maxOutputInput.value, 10);
          const inputCost      = parseFloat(inputCostInput.value);
          const outputCost     = parseFloat(outputCostInput.value);
          const cachedInCost   = parseFloat(cachedInCostIn.value);
          const cachedOutCost  = parseFloat(cachedOutCostIn.value);
          const provider       = providerType || providerInput.value;
          const name           = nameInput.value.trim() || modelId;

          if (!provider) {
            err.textContent = 'Provider is required';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }
          if (!Number.isFinite(contextWindow) || contextWindow <= 0) {
            err.textContent = 'Context window must be a positive integer';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }
          if (!Number.isFinite(maxOutput) || maxOutput <= 0) {
            err.textContent = 'Max output tokens must be a positive integer';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }
          if (!Number.isFinite(inputCost) || inputCost < 0) {
            err.textContent = 'Input cost must be a non-negative number';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }
          if (!Number.isFinite(outputCost) || outputCost < 0) {
            err.textContent = 'Output cost must be a non-negative number';
            err.className = 'skills-install-form__status skills-install-form__status--err';
            return false;
          }

          const spec = {
            name,
            provider,
            contextWindow,
            maxOutputTokens: maxOutput,
            inputCost,
            outputCost,
            cachedInputCost:  Number.isFinite(cachedInCost)  ? cachedInCost  : 0,
            cachedOutputCost: Number.isFinite(cachedOutCost) ? cachedOutCost : 0,
          };
          saveCustomModel(modelId, spec);
          confirmed = true;
          onSaved?.(spec);
          return true;
        },
      },
    ],
    onClose: () => {
      if (!confirmed) onCancelled?.();
    },
  });

  setTimeout(() => nameInput.focus(), 0);
}
