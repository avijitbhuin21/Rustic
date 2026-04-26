import { el } from '../../utils/dom.js';
import { openModal } from '../../utils/modal.js';
import { saveCustomModel, getCustomModel } from '../../state/custom-models.js';

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
