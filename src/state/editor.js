import { create } from 'zustand';
import { useSettings } from './settings';
import { useLayout } from './layout';
import { useTerminal } from './terminal';

const EXT_LANGUAGE = {
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  ts: 'typescript', tsx: 'typescript',
  json: 'json', jsonc: 'json', json5: 'json',
  css: 'css', scss: 'scss', sass: 'scss', less: 'less',
  html: 'html', htm: 'html', xml: 'xml', svg: 'xml', vue: 'html',
  md: 'markdown', markdown: 'markdown',
  rs: 'rust', py: 'python', pyi: 'python', go: 'go', sql: 'sql',
  toml: 'ini', ini: 'ini', cfg: 'ini', conf: 'ini', env: 'ini', properties: 'ini',
  yaml: 'yaml', yml: 'yaml',
  sh: 'shell', bash: 'shell', zsh: 'shell', fish: 'shell',
  ps1: 'powershell', psm1: 'powershell', bat: 'bat', cmd: 'bat',
  c: 'c', h: 'c', cpp: 'cpp', cxx: 'cpp', cc: 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp',
  cs: 'csharp', java: 'java', kt: 'kotlin', kts: 'kotlin', swift: 'swift',
  m: 'objective-c', mm: 'objective-c', php: 'php', rb: 'ruby', pl: 'perl',
  lua: 'lua', r: 'r', scala: 'scala', dart: 'dart', vb: 'vb',
  fs: 'fsharp', fsx: 'fsharp', ex: 'elixir', exs: 'elixir', erl: 'erlang',
  hs: 'haskell', jl: 'julia', clj: 'clojure', cljs: 'clojure',
  graphql: 'graphql', gql: 'graphql', dockerfile: 'dockerfile',
  tf: 'hcl', hcl: 'hcl', txt: 'plaintext', log: 'plaintext',
};

const IMAGE_EXT  = new Set(['png','jpg','jpeg','gif','webp','bmp','ico','avif']);
const MARKDOWN_EXT = new Set(['md','markdown','mdx']);
const PDF_EXT    = new Set(['pdf']);
const SVG_EXT    = new Set(['svg']);
const HTML_EXT   = new Set(['html','htm']);
const VIDEO_EXT  = new Set(['mp4','webm','mov','mkv','m4v','ogv','avi']);
const DOCX_EXT   = new Set(['docx']);
const XLSX_EXT   = new Set(['xlsx','xls','xlsm','xlsb','ods','csv']);
// Binary fallbacks. Audio/video deliberately removed — they have dedicated
// previews now. Office formats route to docx/xlsx previews above the
// binary fallback.
const BINARY_EXT = new Set([
  'exe','dll','so','dylib','bin','o','a','class',
  'zip','tar','gz','bz2','xz','7z','rar',
  'mp3','wav','flac',
  'ttf','otf','woff','woff2','eot',
  'db','sqlite','sqlite3',
]);

// Preview kinds that are backed by editable text, so the editor can flip
// between a rendered "preview" and the raw Monaco "edit" view. Edit is the
// default (see makeFileTab); the user opts into preview via the tab toggle.
// Binary/opaque kinds (image, video, pdf, docx, xlsx, hex) are NOT here —
// there's no text to edit, so they stay preview-only.
export const TOGGLEABLE_PREVIEW_KINDS = new Set(['markdown', 'svg', 'html']);

export function getFileKind(path) {
  if (!path) return 'code';
  const ext = path.toLowerCase().split('.').pop() ?? '';
  if (IMAGE_EXT.has(ext))  return 'image';
  if (VIDEO_EXT.has(ext))  return 'video';
  if (MARKDOWN_EXT.has(ext)) return 'markdown';
  if (PDF_EXT.has(ext))    return 'pdf';
  if (SVG_EXT.has(ext))    return 'svg';
  if (HTML_EXT.has(ext))   return 'html';
  if (DOCX_EXT.has(ext))   return 'docx';
  if (XLSX_EXT.has(ext))   return 'xlsx';
  if (BINARY_EXT.has(ext)) return 'hex';
  return 'code';
}

export function getMonacoLanguage(path) {
  if (!path) return 'plaintext';
  const lower = path.toLowerCase();
  const base  = lower.replace(/\\/g, '/').split('/').pop() ?? '';
  if (base === 'dockerfile' || base.endsWith('.dockerfile')) return 'dockerfile';
  if (base === 'makefile' || base === 'gnumakefile') return 'makefile';
  if (base === '.env' || base.startsWith('.env.')) return 'ini';
  if (base.endsWith('ignore')) return 'ini';
  if (['.babelrc','.prettierrc','.eslintrc','.stylelintrc','.swcrc'].includes(base)) return 'json';
  if (['.yarnrc','.yarnrc.yml','.yarnrc.yaml'].includes(base)) return 'yaml';
  if (['.editorconfig','.npmrc','.npmignore','.htaccess','.gitconfig','.gitattributes'].includes(base)) return 'ini';
  if (['.bashrc','.bash_profile','.zshrc','.zprofile','.profile','.bash_logout'].includes(base)) return 'shell';
  if (base === 'caddyfile' || base.endsWith('.nginx')) return 'ini';
  if (base === 'procfile') return 'shell';
  if (base === 'requirements.txt' || (base.startsWith('requirements') && base.endsWith('.txt'))) return 'pip-requirements';
  if (base === 'go.mod') return 'go';
  if (base === 'go.sum') return 'plaintext';
  if (base === 'package-lock.json' || base === 'composer.lock') return 'json';
  if (base === 'pnpm-lock.yaml') return 'yaml';
  const ext = base.slice(base.lastIndexOf('.') + 1);
  return EXT_LANGUAGE[ext] ?? 'plaintext';
}

export function basename(path) {
  if (!path) return 'untitled';
  const norm = path.replace(/\\/g, '/');
  const i = norm.lastIndexOf('/');
  return i < 0 ? norm : norm.slice(i + 1);
}

function makeFileTab(path) {
  return {
    id: `f:${path}`, path, title: basename(path),
    kind: getFileKind(path), language: getMonacoLanguage(path),
    dirty: false, pinned: false,
    // For TOGGLEABLE_PREVIEW_KINDS, controls edit-vs-preview; ignored for
    // other kinds. Edit-first so markdown/svg/html open in the editor.
    viewMode: 'edit',
  };
}

let _gid = 1;
const newGroupId = () => `g${_gid++}`;
let _sid = 1;

const INITIAL_GROUP_ID = newGroupId();

export const useEditor = create((set, get) => ({
  // Each group: { id, tabs, activeId }
  groups: [{ id: INITIAL_GROUP_ID, tabs: [], activeId: null }],
  activeGroupId: INITIAL_GROUP_ID,

  cursor: { line: 1, column: 1 },
  pendingNav: null,

  // ── Cursor / nav ──────────────────────────────────────────────────────────
  setCursor: (line, column) => set({ cursor: { line, column } }),

  saveCursorForTab: (id, line, column) =>
    set(s => ({
      groups: (s.groups ?? []).map(g => ({
        ...g,
        tabs: g.tabs.map(t => t.id === id ? { ...t, lastCursor: { line, column } } : t),
      })),
    })),

  clearPendingNav: () => set({ pendingNav: null }),

  // ── Group management ──────────────────────────────────────────────────────
  setActiveGroup: (groupId) => set({ activeGroupId: groupId }),

  splitGroup: (groupId) => {
    const newId = newGroupId();
    set(s => {
      const idx = (s.groups ?? []).findIndex(g => g.id === groupId);
      const newGroup = { id: newId, tabs: [], activeId: null };
      const groups = [...s.groups];
      groups.splice(idx + 1, 0, newGroup);
      return { groups, activeGroupId: newId };
    });
    return newId;
  },

  closeGroup: (groupId) => {
    const groups = get().groups ?? [];
    if (groups.length === 1) return;
    set(s => {
      const next = (s.groups ?? []).filter(g => g.id !== groupId);
      const activeGroupId = s.activeGroupId === groupId
        ? (next[next.length - 1]?.id ?? next[0]?.id)
        : s.activeGroupId;
      return { groups: next, activeGroupId };
    });
  },

  // ── Open helpers ─────────────────────────────────────────────────────────
  _openTabInGroup: (tab, groupId, nav = null) => {
    set(s => ({
      groups: s.groups.map(g =>
        g.id === groupId ? { ...g, tabs: [...g.tabs, tab], activeId: tab.id } : g
      ),
      activeGroupId: groupId,
      ...(nav ? { pendingNav: { tabId: tab.id, ...nav } } : {}),
    }));
    return tab.id;
  },

  openFile: (path, nav = null) => get().openFileInGroup(path, get().activeGroupId, nav),

  openFileInGroup: (path, groupId, nav = null) => {
    if (!path) return null;
    const group = get().groups.find(g => g.id === groupId);
    if (!group) return null;
    const existing = group.tabs.find(t => t.path === path);
    if (existing) {
      set(s => ({
        groups: s.groups.map(g => g.id === groupId ? { ...g, activeId: existing.id } : g),
        activeGroupId: groupId,
        ...(nav ? { pendingNav: { tabId: existing.id, ...nav } } : {}),
      }));
      return existing.id;
    }
    return get()._openTabInGroup(makeFileTab(path), groupId, nav);
  },

  openDiff: ({ projectId, filePath, oid = null, title = null, worktreeTaskId = null, fhAnchor = null }) => {
    if (!projectId || !filePath) return null;
    const groupId = get().activeGroupId;
    const group = get().groups.find(g => g.id === groupId);
    if (!group) return null;
    const id = `d:${projectId}:${fhAnchor?.messageId ? `fh-${fhAnchor.messageId}` : worktreeTaskId ? `wt-${worktreeTaskId}` : (oid ?? 'working')}:${filePath}`;
    const existing = group.tabs.find(t => t.id === id);
    if (existing) {
      set(s => ({
        groups: s.groups.map(g => g.id === groupId ? { ...g, activeId: id } : g),
      }));
      return id;
    }
    const tab = {
      id, path: filePath, title: title ?? `Δ ${basename(filePath)}`,
      kind: 'diff', language: getMonacoLanguage(filePath), dirty: false, pinned: false,
      diff: { projectId, path: filePath, oid, worktreeTaskId, fhAnchor },
    };
    return get()._openTabInGroup(tab, groupId);
  },

  openScratch: (title = 'Untitled', language = 'plaintext') => {
    const id = `s:${_sid++}`;
    const tab = { id, path: null, title, kind: 'code', language, dirty: false, pinned: false, scratch: true };
    return get()._openTabInGroup(tab, get().activeGroupId);
  },

  openTerminal: (sessionId, label) => {
    // All terminals now open in the bottom panel only
    useTerminal.setState({ activeSessionId: sessionId });
    useLayout.getState().setBottomPanelVisible(true);
    return `t:${sessionId}`;
  },

  // ── Tab lifecycle ─────────────────────────────────────────────────────────

  // Close a tab only within a specific group — prevents closing the same
  // file ID from every group when the same file is open in multiple splits.
  closeTabInGroup: (id, groupId) => {
    const groups = get().groups ?? [];
    const group  = groups.find(g => g.id === groupId);
    if (!group) return;
    const tab = group.tabs.find(t => t.id === id);
    if (!tab) return;
    set(s => ({
      groups: (s.groups ?? []).map(g => {
        if (g.id !== groupId) return g;
        const idx  = g.tabs.findIndex(t => t.id === id);
        if (idx < 0) return g;
        const tabs = g.tabs.filter(t => t.id !== id);
        const activeId = g.activeId === id
          ? (tabs.length > 0 ? tabs[Math.min(idx, tabs.length - 1)].id : null)
          : g.activeId;
        return { ...g, tabs, activeId };
      }),
    }));
  },

  // Flip a tab between 'edit' (Monaco) and 'preview' (rendered) for the
  // TOGGLEABLE_PREVIEW_KINDS. No-op for tabs of other kinds.
  setTabViewMode: (id, groupId, mode) => {
    set(s => ({
      groups: (s.groups ?? []).map(g =>
        g.id !== groupId
          ? g
          : { ...g, tabs: g.tabs.map(t => (t.id === id ? { ...t, viewMode: mode } : t)) }
      ),
    }));
  },

  // Backward-compat: closes from the active group (or first group that has it)
  closeTab: (id) => {
    const { groups = [], activeGroupId } = get();
    const target =
      groups.find(g => g.id === activeGroupId && g.tabs.some(t => t.id === id)) ??
      groups.find(g => g.tabs.some(t => t.id === id));
    if (target) get().closeTabInGroup(id, target.id);
  },

  setActive: (id) => {
    const group = get().groups.find(g => g.tabs.find(t => t.id === id));
    if (!group) return;
    set(s => ({
      groups: s.groups.map(g => g.id === group.id ? { ...g, activeId: id } : g),
      activeGroupId: group.id,
    }));
  },

  setActiveInGroup: (id, groupId) =>
    set(s => ({
      groups: s.groups.map(g => g.id === groupId ? { ...g, activeId: id } : g),
      activeGroupId: groupId,
    })),

  setDirty: (id, dirty) =>
    set(s => ({
      groups: (s.groups ?? []).map(g => ({
        ...g,
        tabs: g.tabs.map(t => t.id === id ? { ...t, dirty } : t),
      })),
    })),

  // Move a tab from one group to another. Inserts before `beforeTabId` if given, else appends.
  moveTabToGroup: (tabId, fromGroupId, toGroupId, beforeTabId = null) => {
    if (fromGroupId === toGroupId) return;
    set(s => {
      const groups = s.groups ?? [];
      const srcGroup = groups.find(g => g.id === fromGroupId);
      const dstGroup = groups.find(g => g.id === toGroupId);
      if (!srcGroup || !dstGroup) return s;
      const tab = srcGroup.tabs.find(t => t.id === tabId);
      if (!tab) return s;

      // Remove from source
      const srcIdx  = srcGroup.tabs.findIndex(t => t.id === tabId);
      const srcTabs = srcGroup.tabs.filter(t => t.id !== tabId);
      const srcActiveId = srcGroup.activeId === tabId
        ? (srcTabs.length > 0 ? srcTabs[Math.min(srcIdx, srcTabs.length - 1)].id : null)
        : srcGroup.activeId;

      // Insert into destination
      const dstTabs = [...dstGroup.tabs];
      const insertAt = beforeTabId ? dstTabs.findIndex(t => t.id === beforeTabId) : -1;
      if (insertAt >= 0) dstTabs.splice(insertAt, 0, tab);
      else dstTabs.push(tab);

      return {
        groups: groups.map(g => {
          if (g.id === fromGroupId) return { ...g, tabs: srcTabs, activeId: srcActiveId };
          if (g.id === toGroupId)   return { ...g, tabs: dstTabs, activeId: tabId };
          return g;
        }),
        activeGroupId: toGroupId,
      };
    });
  },

  reorderTabsInGroup: (fromId, toId, groupId) => {
    if (fromId === toId) return;
    set(s => ({
      groups: (s.groups ?? []).map(g => {
        if (g.id !== groupId) return g;
        const from = g.tabs.findIndex(t => t.id === fromId);
        const to   = g.tabs.findIndex(t => t.id === toId);
        if (from < 0 || to < 0) return g;
        const tabs = g.tabs.slice();
        const [moved] = tabs.splice(from, 1);
        tabs.splice(to, 0, moved);
        return { ...g, tabs };
      }),
    }));
  },

  closeOthersInGroup: (id, groupId) => {
    set(s => ({
      groups: s.groups.map(g => {
        if (g.id !== groupId) return g;
        return { ...g, tabs: g.tabs.filter(t => t.id === id), activeId: id };
      }),
    }));
  },

  closeAllInGroup: (groupId) => {
    set(s => ({
      groups: s.groups.map(g => {
        if (g.id !== groupId) return g;
        return { ...g, tabs: [], activeId: null };
      }),
    }));
  },
}));
