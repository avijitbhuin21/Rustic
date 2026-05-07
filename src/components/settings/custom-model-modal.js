import { el } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { saveCustomModel, getCustomModel, loadCustomModels } from '../../state/custom-models.js';

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
  // OpenRouter, DeepInfra, …) shares context window and prices. Picking a
  // previously-registered spec here pre-fills the numeric fields so the user
  // doesn't have to re-type them — they can still tweak any value before save.
  const allTemplates = loadCustomModels();
  const templateEntries = Object.entries(allTemplates)
    .filter(([id]) => id !== modelId) // current model's own spec is already loaded above
    .sort(([, a], [, b]) => (b.savedAt || 0) - (a.savedAt || 0));

  if (templateEntries.length > 0) {
    const tmplLabel = el('label', { class: 'rustic-modal__label' }, 'Use template (optional)');
    const tmplSelect = el('select', { class: 'rustic-modal__input' });
    tmplSelect.appendChild(el('option', { value: '' }, '— start fresh —'));
    for (const [id, spec] of templateEntries) {
      const display = spec.name && spec.name !== id ? `${spec.name} — ${id}` : id;
      const suffix = spec.provider ? ` (${spec.provider})` : '';
      tmplSelect.appendChild(el('option', { value: id }, `${display}${suffix}`));
    }
    tmplSelect.addEventListener('change', () => {
      const picked = allTemplates[tmplSelect.value];
      if (!picked) return;
      ctxWindowInput.value  = picked.contextWindow != null ? String(picked.contextWindow) : '';
      maxOutputInput.value  = picked.maxOutputTokens != null ? String(picked.maxOutputTokens) : '';
      inputCostInput.value  = picked.inputCost != null ? String(picked.inputCost) : '';
      outputCostInput.value = picked.outputCost != null ? String(picked.outputCost) : '';
      cachedInCostIn.value  = picked.cachedInputCost  ? String(picked.cachedInputCost)  : '';
      cachedOutCostIn.value = picked.cachedOutputCost ? String(picked.cachedOutputCost) : '';
      // Display Name is intentionally left alone — the user usually wants
      // their own naming for "same model on a different provider", not a
      // verbatim copy of the template's name.
    });
    body.appendChild(tmplLabel);
    body.appendChild(tmplSelect);
  }

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
