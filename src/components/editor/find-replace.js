import { el, icon, iconMulti } from '../../utils/dom.js';

export function createFindReplace() {
  let caseSensitive = false;
  let wholeWord = false;
  let useRegex = false;
  let replaceExpanded = false;

  let _onSearch = null;
  let _onNavigate = null;
  let _onReplace = null;
  let _onReplaceAll = null;
  let _onClose = null;

  // -- Icons (24x24 viewBox, stroke-based) --
  const mkChevronRight = () => icon('M9 18l6-6-6-6', 14);
  const mkChevronDown = () => icon('M6 9l6 6 6-6', 14);
  const mkChevronUp = () => icon('M18 15l-6-6-6 6', 14);
  const mkClose = () => iconMulti(['M18 6L6 18', 'M6 6l12 12'], 14);
  const mkReplace = () => iconMulti(['M5 8h6', 'M5 16h4', 'M16 8v4a3 3 0 01-3 3h-1', 'M14 13l-2 2 2 2'], 14);
  const mkReplaceAll = () => iconMulti(['M5 5h6', 'M5 11h4', 'M5 17h6', 'M16 5v8a3 3 0 01-3 3h-1', 'M14 14l-2 2 2 2'], 14);

  // -- Build DOM --
  const widget = el('div', { class: 'find-replace-widget' });

  // Toggle expand/collapse replace row
  const toggleBtn = el('button', { class: 'find-replace-toggle', title: 'Toggle Replace' });
  toggleBtn.appendChild(mkChevronRight());

  // Find input with option toggles
  const findInputWrap = el('div', { class: 'find-replace-input-wrap' });
  const findInput = el('input', { class: 'find-replace-input', placeholder: 'Find', type: 'text' });
  const caseBtn = el('button', { class: 'find-replace-opt', title: 'Match Case (Alt+C)' }, 'Aa');
  const wordBtn = el('button', { class: 'find-replace-opt find-replace-opt--word', title: 'Match Whole Word (Alt+W)' }, 'ab');
  const regexBtn = el('button', { class: 'find-replace-opt', title: 'Use Regular Expression (Alt+R)' }, '.*');
  findInputWrap.append(findInput, caseBtn, wordBtn, regexBtn);

  // Match info
  const matchInfo = el('span', { class: 'find-replace-info' });

  // Navigation + close buttons
  const prevBtn = el('button', { class: 'find-replace-btn', title: 'Previous Match (Shift+Enter)' });
  prevBtn.appendChild(mkChevronUp());
  const nextBtn = el('button', { class: 'find-replace-btn', title: 'Next Match (Enter)' });
  nextBtn.appendChild(mkChevronDown());
  const closeBtn = el('button', { class: 'find-replace-btn', title: 'Close (Escape)' });
  closeBtn.appendChild(mkClose());

  // Find row
  const findRow = el('div', { class: 'find-replace-row' });
  findRow.append(toggleBtn, findInputWrap, matchInfo, prevBtn, nextBtn, closeBtn);

  // Replace input
  const replaceInputWrap = el('div', { class: 'find-replace-input-wrap' });
  const replaceInput = el('input', { class: 'find-replace-input', placeholder: 'Replace', type: 'text' });
  replaceInputWrap.appendChild(replaceInput);

  const replaceBtn = el('button', { class: 'find-replace-action', title: 'Replace (Enter)' }, 'Replace');
  const replaceAllBtn = el('button', { class: 'find-replace-action', title: 'Replace All (Ctrl+Enter)' }, 'All');

  // Replace row
  const replaceRow = el('div', { class: 'find-replace-row' });
  const replaceSpacer = el('div', { class: 'find-replace-toggle-spacer' });
  replaceRow.append(replaceSpacer, replaceInputWrap, replaceBtn, replaceAllBtn);
  replaceRow.style.display = 'none';

  widget.append(findRow, replaceRow);

  // Prevent editor from stealing focus when clicking the widget
  widget.addEventListener('mousedown', (e) => e.stopPropagation());

  // -- Event handlers --
  function doSearch() {
    if (_onSearch) _onSearch(findInput.value, { caseSensitive, wholeWord, useRegex });
  }

  findInput.addEventListener('input', doSearch);

  findInput.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      hide();
      if (_onClose) _onClose();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (_onNavigate) _onNavigate(e.shiftKey ? 'prev' : 'next');
      return;
    }
    if (e.ctrlKey && e.key === 'h') {
      e.preventDefault();
      setReplaceExpanded(true);
      replaceInput.focus();
      return;
    }
    // Prevent browser find dialog
    if (e.ctrlKey && e.key === 'f') e.preventDefault();
  });

  replaceInput.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      hide();
      if (_onClose) _onClose();
      return;
    }
    if (e.ctrlKey && e.key === 'Enter') {
      e.preventDefault();
      if (_onReplaceAll) _onReplaceAll(replaceInput.value);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (_onReplace) _onReplace(replaceInput.value);
      return;
    }
    if (e.ctrlKey && e.key === 'f') e.preventDefault();
  });

  // Option toggle buttons
  caseBtn.addEventListener('click', () => {
    caseSensitive = !caseSensitive;
    caseBtn.classList.toggle('active', caseSensitive);
    doSearch();
  });
  wordBtn.addEventListener('click', () => {
    wholeWord = !wholeWord;
    wordBtn.classList.toggle('active', wholeWord);
    doSearch();
  });
  regexBtn.addEventListener('click', () => {
    useRegex = !useRegex;
    regexBtn.classList.toggle('active', useRegex);
    doSearch();
  });

  // Toggle replace row visibility
  function setReplaceExpanded(expanded) {
    replaceExpanded = expanded;
    replaceRow.style.display = expanded ? 'flex' : 'none';
    toggleBtn.replaceChildren(expanded ? mkChevronDown() : mkChevronRight());
  }

  toggleBtn.addEventListener('click', () => setReplaceExpanded(!replaceExpanded));

  // Navigation
  prevBtn.addEventListener('click', () => { if (_onNavigate) _onNavigate('prev'); });
  nextBtn.addEventListener('click', () => { if (_onNavigate) _onNavigate('next'); });

  // Replace actions
  replaceBtn.addEventListener('click', () => { if (_onReplace) _onReplace(replaceInput.value); });
  replaceAllBtn.addEventListener('click', () => { if (_onReplaceAll) _onReplaceAll(replaceInput.value); });

  // Close
  closeBtn.addEventListener('click', () => { hide(); if (_onClose) _onClose(); });

  // Alt shortcuts for toggles (work from both inputs)
  widget.addEventListener('keydown', (e) => {
    if (e.altKey && e.key === 'c') { caseBtn.click(); e.preventDefault(); }
    if (e.altKey && e.key === 'w') { wordBtn.click(); e.preventDefault(); }
    if (e.altKey && e.key === 'r') { regexBtn.click(); e.preventDefault(); }
  });

  function show(withReplace, initialQuery) {
    widget.classList.add('visible');
    setReplaceExpanded(withReplace);
    if (initialQuery != null && initialQuery !== '') {
      findInput.value = initialQuery;
    }
    findInput.focus();
    findInput.select();
    if (findInput.value) doSearch();
  }

  function hide() {
    widget.classList.remove('visible');
  }

  function setMatchInfo(current, total) {
    if (total === 0) {
      matchInfo.textContent = findInput.value ? 'No results' : '';
      matchInfo.classList.toggle('no-results', !!findInput.value);
    } else {
      matchInfo.textContent = `${current} of ${total}`;
      matchInfo.classList.remove('no-results');
    }
  }

  return {
    element: widget,
    show,
    hide,
    isVisible: () => widget.classList.contains('visible'),
    setMatchInfo,
    getQuery: () => findInput.value,
    focus: () => { findInput.focus(); findInput.select(); },
    onSearch: (cb) => { _onSearch = cb; },
    onNavigate: (cb) => { _onNavigate = cb; },
    onReplace: (cb) => { _onReplace = cb; },
    onReplaceAll: (cb) => { _onReplaceAll = cb; },
    onClose: (cb) => { _onClose = cb; },
  };
}
