// Public issue tracker page: list (open / closed, search, paging), detail
// dialog with attachments, and the new-issue form. All user text is rendered
// with textContent — nothing from the API is ever parsed as HTML.
(() => {
  const PER_PAGE = 30;
  const MAX_FILES = 5;
  const MAX_FILE_BYTES = 10 * 1024 * 1024;
  const OK_EXT = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'pdf', 'txt', 'log', 'md', 'json', 'csv', 'yaml', 'yml', 'toml'];

  const $ = (id) => document.getElementById(id);
  const list = $('list');
  const empty = $('empty');
  const more = $('more');
  const search = $('search');
  const detail = $('detail');
  const composer = $('composer');
  const form = $('issue-form');
  const formError = $('form-error');

  const state = { status: 'open', q: '', page: 1, total: 0, loading: false, seq: 0 };

  /** Element helper: tag, class, text. */
  function el(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  /** "3 days ago" style relative time from unix seconds. */
  function ago(secs) {
    if (!secs) return '';
    const d = Math.max(0, Date.now() / 1000 - secs);
    const units = [[31536000, 'year'], [2592000, 'month'], [86400, 'day'], [3600, 'hour'], [60, 'minute']];
    for (const [s, name] of units) {
      const n = Math.floor(d / s);
      if (n >= 1) return `${n} ${name}${n === 1 ? '' : 's'} ago`;
    }
    return 'just now';
  }

  function fmtBytes(n) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  }

  function tags(issue, withId = true) {
    const wrap = el('div', 'iss-tags');
    if (withId) wrap.append(el('span', 'mono', `#${issue.id}`));
    wrap.append(el('span', `iss-tag ${issue.kind}`, issue.kind));
    if (issue.source === 'agent') wrap.append(el('span', 'iss-tag agent', 'via agent'));
    const when = issue.status === 'closed' && issue.closed_at ? `closed ${ago(issue.closed_at)}` : `opened ${ago(issue.created_at)}`;
    wrap.append(el('span', null, when));
    if (issue.attachments) wrap.append(el('span', null, `${issue.attachments} file${issue.attachments === 1 ? '' : 's'}`));
    return wrap;
  }

  function renderItem(issue) {
    const li = el('li', 'iss-item');
    li.tabIndex = 0;
    li.setAttribute('role', 'button');
    li.append(el('span', `iss-dot${issue.status === 'closed' ? ' closed' : ''}`));
    const main = el('div', 'iss-item-main');
    main.append(el('h3', 'iss-item-title', issue.title));
    if (issue.excerpt) main.append(el('p', 'iss-item-excerpt', issue.excerpt));
    main.append(tags(issue));
    li.append(main);
    const open = () => openDetail(issue.id);
    li.addEventListener('click', open);
    li.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } });
    return li;
  }

  function skeleton() {
    list.replaceChildren(...Array.from({ length: 4 }, () => el('li', 'iss-skel')));
    empty.hidden = true;
  }

  /** Fetch a page; `reset` starts over (tab / search change). */
  async function load(reset) {
    const seq = ++state.seq;
    if (reset) { state.page = 1; skeleton(); }
    state.loading = true;
    more.disabled = true;
    const params = new URLSearchParams({ status: state.status, page: String(state.page), per_page: String(PER_PAGE) });
    if (state.q) params.set('q', state.q);
    try {
      const res = await fetch(`/api/issues?${params}`, { headers: { Accept: 'application/json' } });
      const data = await res.json().catch(() => ({}));
      if (seq !== state.seq) return;
      if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
      $('count-open').textContent = data.counts?.open ?? 0;
      $('count-closed').textContent = data.counts?.closed ?? 0;
      state.total = data.total || 0;
      const items = (data.issues || []).map(renderItem);
      if (reset) list.replaceChildren(...items); else list.append(...items);
      const shown = list.querySelectorAll('.iss-item').length;
      empty.hidden = shown > 0;
      empty.textContent = state.q ? 'No issues match your search.' : state.status === 'open' ? 'No open issues. Nice.' : 'No closed issues yet.';
      more.hidden = shown >= state.total;
    } catch (e) {
      if (seq !== state.seq) return;
      list.replaceChildren();
      empty.hidden = false;
      empty.textContent = `Couldn't load issues — ${e.message}`;
      more.hidden = true;
    } finally {
      if (seq === state.seq) { state.loading = false; more.disabled = false; }
    }
  }

  async function openDetail(id) {
    const url = new URL(location.href);
    url.searchParams.set('id', id);
    history.replaceState(null, '', url);
    $('detail-num').textContent = `#${id}`;
    $('detail-title').textContent = 'Loading…';
    $('detail-meta').replaceChildren();
    $('detail-body').textContent = '';
    $('detail-files').replaceChildren();
    $('detail-resolution').hidden = true;
    if (!detail.open) detail.showModal();
    try {
      const res = await fetch(`/api/issues/${encodeURIComponent(id)}`);
      const issue = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(issue.error || `HTTP ${res.status}`);
      $('detail-title').textContent = issue.title;
      const meta = $('detail-meta');
      meta.append(el('span', `iss-tag ${issue.status === 'closed' ? '' : 'feature'}`, issue.status));
      meta.append(tags({ ...issue, attachments: 0 }, false));
      if (issue.app_version) meta.append(el('span', null, `v${issue.app_version}`));
      if (issue.os) meta.append(el('span', null, issue.os));
      if (issue.resolution) { const r = $('detail-resolution'); r.textContent = issue.resolution; r.hidden = false; }
      $('detail-body').textContent = issue.body || '';
      const files = $('detail-files');
      for (const f of issue.attachments || []) {
        const a = el('a', 'iss-file');
        a.href = f.url;
        a.target = '_blank';
        a.rel = 'noopener';
        if (f.content_type.startsWith('image/')) {
          const img = el('img');
          img.src = f.url;
          img.alt = f.filename;
          img.loading = 'lazy';
          a.append(img);
        }
        a.append(el('span', null, f.filename), el('span', 'mono', fmtBytes(f.size)));
        files.append(a);
      }
    } catch (e) {
      $('detail-title').textContent = `Couldn't load issue — ${e.message}`;
    }
  }

  function closeDetail() {
    const url = new URL(location.href);
    url.searchParams.delete('id');
    history.replaceState(null, '', url);
  }

  function showError(msg) {
    formError.textContent = msg;
    formError.hidden = !msg;
  }

  /** Client-side checks mirroring the server limits; returns an error message or ''. */
  function checkForm() {
    const title = form.title.value.trim();
    if (title.length < 4 || title.length > 160) return 'Title must be 4–160 characters.';
    const files = Array.from($('files').files || []);
    if (files.length > MAX_FILES) return `At most ${MAX_FILES} attachments.`;
    for (const f of files) {
      const ext = (f.name.split('.').pop() || '').toLowerCase();
      if (!OK_EXT.includes(ext)) return `${f.name}: file type not accepted.`;
      if (f.size > MAX_FILE_BYTES) return `${f.name} is larger than 10 MB.`;
    }
    return '';
  }

  async function submit(e) {
    e.preventDefault();
    const problem = checkForm();
    showError(problem);
    if (problem) return;
    const btn = $('submit');
    btn.disabled = true;
    btn.querySelector('span').textContent = 'Submitting…';
    const fd = new FormData(form);
    fd.delete('files');
    fd.set('source', 'web');
    for (const f of Array.from($('files').files || [])) fd.append('files', f, f.name);
    try {
      const res = await fetch('/api/issues', { method: 'POST', body: fd });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
      form.reset();
      prefillOs();
      composer.close();
      state.status = 'open';
      syncTabs();
      await load(true);
      if (data.id) openDetail(data.id);
    } catch (err) {
      showError(err.message);
    } finally {
      btn.disabled = false;
      btn.querySelector('span').textContent = 'Submit issue';
    }
  }

  function prefillOs() {
    const ua = navigator.userAgent;
    const os = /Mac/.test(ua) ? 'macOS' : /Win/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
    const f = $('os-field');
    if (f && !f.value) f.value = os;
  }

  function syncTabs() {
    document.querySelectorAll('.iss-tab').forEach((t) => t.setAttribute('aria-selected', String(t.dataset.status === state.status)));
  }

  document.querySelectorAll('.iss-tab').forEach((tab) => {
    tab.addEventListener('click', () => {
      if (state.status === tab.dataset.status) return;
      state.status = tab.dataset.status;
      syncTabs();
      load(true);
    });
  });

  let timer = null;
  search.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(() => { state.q = search.value.trim(); load(true); }, 250);
  });

  more.addEventListener('click', () => { if (!state.loading) { state.page += 1; load(false); } });

  for (const dlg of [detail, composer]) {
    dlg.addEventListener('click', (e) => {
      if (e.target === dlg || e.target.closest('[data-close]')) dlg.close();
    });
  }
  detail.addEventListener('close', closeDetail);

  $('new-issue-btn').addEventListener('click', () => {
    showError('');
    prefillOs();
    composer.showModal();
    form.title.focus();
  });
  form.addEventListener('submit', submit);

  load(true);
  const deep = new URLSearchParams(location.search).get('id');
  if (deep && /^\d+$/.test(deep)) openDetail(deep);
})();
