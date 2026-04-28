import { el, icon } from '../../utils/dom.js';
import { resolveConflict, mergeCommit, rebaseContinue, rebaseAbort } from '../../state/git.js';
import { showToast, showErrorToast } from '../toast.js';

export function createConflictPanel(project, conflicts) {
  const panel = el('div', { class: 'scm-conflicts' });

  const header = el('div', { class: 'scm-conflicts__header' });
  header.appendChild(icon('M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z', 14));
  header.appendChild(el('span', {}, `Merge Conflicts (${conflicts.length})`));
  panel.appendChild(header);

  // Action buttons for the merge/rebase state
  const actions = el('div', { class: 'scm-conflicts__actions' });

  const continueBtn = el('button', { class: 'scm-conflicts__btn scm-conflicts__btn--primary' }, 'Continue');
  continueBtn.addEventListener('click', async () => {
    try {
      // Try rebase continue first, fall back to merge commit
      try {
        await rebaseContinue(project.id);
        showToast('Rebase continued', { kind: 'success' });
      } catch {
        await mergeCommit(project.id);
        showToast('Merge committed', { kind: 'success' });
      }
    } catch (e) {
      // Was a silent console.error — invisible to the user. Now surfaces the
      // failure as a toast with the underlying message so they know why
      // continue didn't happen.
      const msg = e?.message || String(e);
      showErrorToast(`Could not continue — ${msg}`);
      console.error('Continue failed:', e);
    }
  });

  const abortBtn = el('button', { class: 'scm-conflicts__btn scm-conflicts__btn--danger' }, 'Abort');
  abortBtn.addEventListener('click', async () => {
    try {
      await rebaseAbort(project.id);
      showToast('Rebase aborted — working tree restored', { kind: 'info' });
    } catch (e) {
      const msg = e?.message || String(e);
      showErrorToast(`Abort failed — ${msg}`);
      console.error('Abort failed:', e);
    }
  });

  actions.appendChild(continueBtn);
  actions.appendChild(abortBtn);
  panel.appendChild(actions);

  // List conflict files
  for (const conflict of conflicts) {
    const fileRow = el('div', { class: 'scm-conflict-file' });

    const statusBadge = el('span', {
      class: 'scm-file__status',
      style: { color: 'var(--bright-red)' },
    }, 'C');

    const fileName = conflict.path.split('/').pop() || conflict.path;
    const nameEl = el('span', { class: 'scm-conflict-file__name' }, fileName);

    const resolveActions = el('div', { class: 'scm-conflict-file__actions' });

    const wrapResolve = (side) => async () => {
      try {
        await resolveConflict(project.id, conflict.path, side);
      } catch (e) {
        const msg = e?.message || String(e);
        showErrorToast(`Failed to resolve ${conflict.path} — ${msg}`);
      }
    };

    const oursBtn = el('button', { title: 'Accept Ours' }, 'Ours');
    oursBtn.addEventListener('click', wrapResolve('ours'));

    const theirsBtn = el('button', { title: 'Accept Theirs' }, 'Theirs');
    theirsBtn.addEventListener('click', wrapResolve('theirs'));

    const bothBtn = el('button', { title: 'Accept Both' }, 'Both');
    bothBtn.addEventListener('click', wrapResolve('both'));

    resolveActions.appendChild(oursBtn);
    resolveActions.appendChild(theirsBtn);
    resolveActions.appendChild(bothBtn);

    fileRow.appendChild(statusBadge);
    fileRow.appendChild(nameEl);
    fileRow.appendChild(resolveActions);
    panel.appendChild(fileRow);
  }

  return panel;
}
