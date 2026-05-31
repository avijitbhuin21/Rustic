import React, { Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { toast } from 'sonner';
import { loader } from '@monaco-editor/react';
import { Skeleton } from '@/components/ui/skeleton';
import { useEditor } from '@/state/editor';
import { useSettings } from '@/state/settings';
import { formatWithPrettier, isPrettierLanguage } from '@/lib/prettier-client';
import {
  setActiveEditor,
  clearActiveEditor,
  formatActiveEditor,
  setActiveSaver,
  clearActiveSaver,
  applyFormattedContent,
} from '@/lib/active-editor';

import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker';
import JsonWorker from 'monaco-editor/esm/vs/language/json/json.worker?worker';
import CssWorker from 'monaco-editor/esm/vs/language/css/css.worker?worker';
import HtmlWorker from 'monaco-editor/esm/vs/language/html/html.worker?worker';
import TsWorker from 'monaco-editor/esm/vs/language/typescript/ts.worker?worker';

if (typeof self !== 'undefined' && !self.MonacoEnvironment) {
  self.MonacoEnvironment = {
    getWorker(_workerId, label) {
      switch (label) {
        case 'json':
          return new JsonWorker();
        case 'css':
        case 'scss':
        case 'less':
          return new CssWorker();
        case 'html':
        case 'handlebars':
        case 'razor':
          return new HtmlWorker();
        case 'typescript':
        case 'javascript':
          return new TsWorker();
        default:
          return new EditorWorker();
      }
    },
  };
}

// Pre-fetch the @monaco-editor/react bundle the moment this module loads.
// Previously it only started loading after file content arrived (because the
// component returned <Fallback /> while content === null, so MonacoReact never
// rendered until content was ready — sequential instead of parallel).
const _monacoReactImport = import('@monaco-editor/react');
const MonacoReact = React.lazy(() =>
  _monacoReactImport.then((m) => ({ default: m.default }))
);

function registerPipRequirementsLanguage(monaco) {
  if (monaco.languages.getLanguages().some((l) => l.id === 'pip-requirements')) return;
  monaco.languages.register({ id: 'pip-requirements' });
  monaco.languages.setLanguageConfiguration('pip-requirements', {
    comments: { lineComment: '#' },
  });
  // Stateless tokenizer — rules are tried top-to-bottom; order matters.
  // Package names start with a letter; version numbers start with a digit.
  // This prevents the ambiguity that caused a stateful tokenizer to mis-color
  // version strings (e.g. "1.0.0") as package-name tokens.
  monaco.languages.setMonarchTokensProvider('pip-requirements', {
    defaultToken: '',
    tokenizer: {
      root: [
        // Full-line comments
        [/#.*$/, 'comment'],
        // CLI flags: -r, -e, --index-url, --extra-index-url, etc.
        [/--?[a-zA-Z][\w-]*/, 'keyword'],
        // Extras block: [standard], [binary,pool]
        [/\[[^\]]*\]/, 'string'],
        // Environment markers after semicolon
        [/;.*$/, 'comment'],
        // Version operators (longest match first)
        [/===|~=|!=|>=|<=|==|[><]/, 'keyword'],
        // Version numbers — MUST start with a digit so "boto3" is not split here
        [/\d[0-9a-zA-Z.*+!]*/, 'number'],
        // Package names — start with letter, may contain hyphens/underscores/dots
        [/[a-zA-Z][a-zA-Z0-9._-]*/, 'variable'],
        // Commas between version constraints
        [/,/, 'delimiter'],
      ],
    },
  });
}

let monacoConfigured = false;
function configureMonaco() {
  if (monacoConfigured) return;
  monacoConfigured = true;
  import('monaco-editor').then((monaco) => {
    loader.config({ monaco });
    return loader.init();
  }).then((monaco) => {
    if (monaco.languages.typescript) {
      monaco.languages.typescript.typescriptDefaults.setDiagnosticsOptions({
        noSemanticValidation: false,
        noSyntaxValidation: false,
      });
      monaco.languages.typescript.javascriptDefaults.setDiagnosticsOptions({
        noSemanticValidation: false,
        noSyntaxValidation: false,
      });
    }
    registerPipRequirementsLanguage(monaco);
    // Custom theme: extends vs-dark with teal colours for the built-in
    // Ctrl+F find widget so it's visually distinct from global-search
    // decorations (which use a yellow inline class defined in globals.css).
    monaco.editor.defineTheme('rustic-dark', {
      base: 'vs-dark',
      inherit: true,
      // Italicize comments so Victor Mono renders them in its signature
      // semi-connected cursive — the "two fonts in one" look from the font's
      // showcase. The cursive glyphs ONLY appear on italic-styled tokens, so
      // without these rules every token used the upright roman face and the
      // editor looked single-font. We also italicize language constants
      // (true/false/null) and `this`-like identifiers, which Victor Mono's
      // demo styles cursively, while leaving control keywords upright.
      // Foreground is repeated because a Monaco rule that sets only fontStyle
      // can drop the inherited token color.
      rules: [
        { token: 'comment', foreground: '6A9955', fontStyle: 'italic' },
        { token: 'comment.line', foreground: '6A9955', fontStyle: 'italic' },
        { token: 'comment.block', foreground: '6A9955', fontStyle: 'italic' },
        { token: 'comment.doc', foreground: '6A9955', fontStyle: 'italic' },
        { token: 'constant.language', fontStyle: 'italic' },
        { token: 'keyword.constant', fontStyle: 'italic' },
        { token: 'variable.language', fontStyle: 'italic' },
      ],
      colors: {
        'editor.findMatchBackground':          '#0d948840',
        'editor.findMatchBorder':              '#0d9488',
        'editor.findMatchHighlightBackground': '#0d948820',
        'editor.findMatchHighlightBorder':     '#0d948860',
      },
    });
    // Route Monaco's link-click (Ctrl/Cmd+click on URLs in code, comments,
    // strings) through Tauri's shell.open so it lands in the user's default
    // browser instead of navigating the WebView itself. Without this, Monaco's
    // built-in opener calls window.open / window.location and the editor
    // panel gets replaced by the link target.
    if (typeof monaco.editor.registerLinkOpener === 'function') {
      monaco.editor.registerLinkOpener({
        async open(resource) {
          try {
            const href =
              typeof resource?.toString === 'function'
                ? resource.toString(true)
                : String(resource);
            if (!/^(https?|mailto):/i.test(href)) return false;
            const { open } = await import('@tauri-apps/plugin-shell');
            await open(href);
            return true;
          } catch {
            return false;
          }
        },
      });
    }
  }).catch(() => {});
}

// Kick off Monaco worker initialisation immediately when this module is first
// imported — not deferred to a useEffect inside the component.
configureMonaco();

// Victor Mono is the IDE's bundled default code font (see globals.css). The
// system-mono tail are only fallbacks for the brief pre-load window.
const DEFAULT_EDITOR_FONT = "'Victor Mono', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace";

// Build a Monaco editor-options object from our settings shape. These are the
// fields that `editor.updateOptions` accepts at runtime. Keep this in sync
// with the fields rendered by editor-settings.jsx.
function buildEditorOptions(e = {}) {
  return {
    fontSize: 13,
    fontFamily: e.font_family || DEFAULT_EDITOR_FONT,
    // Monaco renders OpenType ligatures natively (Victor Mono has a rich set:
    // ->, =>, !=, ===, >=, etc.). Opt-out via the `font_ligatures: false`
    // setting for users who dislike them.
    fontLigatures: e.font_ligatures === false ? false : true,
    minimap: { enabled: !!e.minimap },
    scrollBeyondLastLine: false,
    automaticLayout: true,
    bracketPairColorization: { enabled: e.bracket_pair_colorization !== false },
    stickyScroll: { enabled: e.sticky_scroll !== false },
    smoothScrolling: e.smooth_scrolling !== false,
    cursorBlinking: e.cursor_blink === false ? 'solid' : 'smooth',
    cursorStyle: e.cursor_style || 'line',
    cursorSmoothCaretAnimation: e.cursor_smooth_caret || 'off',
    renderLineHighlight: 'all',
    guides: { indentation: e.indent_guides !== false },
    autoIndent: e.auto_indent || 'advanced',
    wordWrap: e.word_wrap ? 'on' : 'off',
    lineNumbers: e.line_numbers === false ? 'off' : 'on',
    renderWhitespace: e.render_whitespace || 'none',
    // Includes constructor-only counterparts so the first mount honours them
    // even though updateOptions later ignores these — they get routed to the
    // model by Monaco during construction. We separately push them to the
    // model via applyModelOptions below for runtime changes.
    tabSize: e.tab_size ?? 4,
    insertSpaces: e.insert_spaces !== false,
    unicodeHighlight: {
      invisibleCharacters: !!e.show_zero_width_characters,
      ambiguousCharacters: !!e.show_zero_width_characters,
    },
    largeFileOptimizations: true,
  };
}

// `tabSize` and `insertSpaces` are model-level options — `editor.updateOptions`
// silently ignores them, so we have to push them to the model directly when
// settings change.
function applyModelOptions(editor, e = {}) {
  const model = editor.getModel?.();
  if (!model) return;
  model.updateOptions({
    tabSize: e.tab_size ?? 4,
    insertSpaces: e.insert_spaces !== false,
  });
}

const DEFAULT_EDITOR_OPTIONS = buildEditorOptions();

function Fallback() {
  return (
    <div className="flex h-full w-full flex-col gap-2 p-4">
      <Skeleton className="h-4 w-1/3" />
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-4 w-1/2" />
      <Skeleton className="h-4 w-3/4" />
      <Skeleton className="h-4 w-1/4" />
    </div>
  );
}

export default function MonacoEditor({ tab }) {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const editorRef = useRef(null);
  const monacoRef = useRef(null);
  const originalRef = useRef('');
  const pendingContentRef = useRef(null);
  // Active search-result decoration — cleared before placing a new one so
  // navigating between matches in the same file doesn't stack decorations.
  const searchDecoRef = useRef(null);
  const tabRef = useRef(tab);
  tabRef.current = tab;

  const editorSettings = useSettings((s) => s.settings?.editor);

  // Stable saver reference — set inside the save() useCallback below. Keeps a
  // fixed function identity for setActiveSaver so we don't have to re-register
  // every time the save closure changes.
  const saveRef = useRef(async () => {});
  const stableSaver = useCallback(() => saveRef.current(), []);

  const setDirty = useEditor((s) => s.setDirty);
  const setCursor = useEditor((s) => s.setCursor);
  const saveCursorForTab = useEditor((s) => s.saveCursorForTab);
  // Subscribe so we can react when pendingNav changes while this tab is already
  // the active (rendered) tab — in that case applyContent never fires again,
  // so the useEffect below handles the navigation imperatively.
  const pendingNav = useEditor((s) => s.pendingNav);

  // Place a yellow inline decoration on the matched range and scroll to it.
  // Clears any previous search decoration first so navigating between results
  // in the same file never stacks highlights.
  const applySearchHighlight = useCallback((editor, range) => {
    if (searchDecoRef.current) {
      searchDecoRef.current.clear();
      searchDecoRef.current = null;
    }
    searchDecoRef.current = editor.createDecorationsCollection([{
      range,
      options: { inlineClassName: 'search-match-highlight' },
    }]);
    editor.revealRangeInCenter(range);
    editor.setPosition({ lineNumber: range.startLineNumber, column: range.startColumn });
    const pos = editor.getPosition();
    if (pos) setCursor(pos.lineNumber, pos.column);
  }, [setCursor]);

  // Build a Monaco range from pendingNav fields (0-indexed offsets → 1-indexed columns).
  const navToRange = (nav) => ({
    startLineNumber: nav.line,
    startColumn:     (nav.matchStart ?? 0) + 1,
    endLineNumber:   nav.line,
    endColumn:       (nav.matchEnd   ?? nav.matchStart ?? 0) + 1,
  });

  // Apply content to the editor and remove the loading overlay.
  const applyContent = useCallback((editor, text) => {
    originalRef.current = text;
    editor.setValue(text);
    setLoading(false);

    const { pendingNav, clearPendingNav } = useEditor.getState();
    if (pendingNav?.tabId === tab.id && pendingNav.line >= 1) {
      clearPendingNav();
      try { applySearchHighlight(editor, navToRange(pendingNav)); } catch {}
      return;
    }

    // Normal cursor restoration when not navigating from search.
    const stored = tabRef.current.lastCursor;
    if (stored?.line >= 1) {
      try {
        editor.setPosition({ lineNumber: stored.line, column: stored.column });
        editor.revealPositionInCenter({ lineNumber: stored.line, column: stored.column });
      } catch {}
    }
    const pos = editor.getPosition();
    if (pos) setCursor(pos.lineNumber, pos.column);
  }, [setCursor, tab.id, applySearchHighlight]);

  // Handle the case where the target file is already the active tab.
  // applyContent only fires on content load (remount), so if the user clicks
  // a search result in the currently-open file we need to navigate here.
  useEffect(() => {
    if (!pendingNav || pendingNav.tabId !== tab.id) return;
    if (loading) return; // applyContent will handle it once content finishes loading
    const editor = editorRef.current;
    if (!editor) return;
    useEditor.getState().clearPendingNav();
    try { applySearchHighlight(editor, navToRange(pendingNav)); } catch {}
  }, [pendingNav, tab.id, loading, applySearchHighlight]);

  useEffect(() => {
    let cancelled = false;
    pendingContentRef.current = null;
    setLoading(true);
    setError(null);

    if (tab.scratch || !tab.path) {
      if (editorRef.current) {
        applyContent(editorRef.current, '');
      } else {
        originalRef.current = '';
        pendingContentRef.current = '';
        setLoading(false);
      }
      return () => {};
    }

    invoke('read_file_content', { path: tab.path })
      .then((text) => {
        if (cancelled) return;
        const t = text ?? '';
        if (editorRef.current) {
          applyContent(editorRef.current, t);
        } else {
          // Editor hasn't mounted yet; save for handleMount.
          originalRef.current = t;
          pendingContentRef.current = t;
        }
      })
      .catch((err) => {
        if (cancelled) return;
        setError(String(err));
        setLoading(false);
      });

    return () => { cancelled = true; };
  }, [tab.id, tab.path, tab.scratch, applyContent]);

  const handleMount = useCallback((editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    // Apply the persisted editor settings immediately on mount. The reactive
    // useEffect below doesn't re-fire when a new Monaco instance mounts with the
    // same settings reference it already had (deps haven't changed). Without
    // this, freshly-opened tabs render with module defaults until the user
    // edits a setting while an editor is alive.
    const initial = useSettings.getState().settings?.editor;
    if (initial) {
      editor.updateOptions(buildEditorOptions(initial));
      applyModelOptions(editor, initial);
    }

    // If content arrived before the editor was ready, apply it now.
    if (pendingContentRef.current !== null) {
      applyContent(editor, pendingContentRef.current);
      pendingContentRef.current = null;
    }

    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, async () => {
      await save();
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyF, () => {
      editor.getAction('actions.find')?.run();
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyH, () => {
      editor.getAction('editor.action.startFindReplaceAction')?.run();
    });
    // Alt+Shift+F: format the document via our resolver (custom formatter →
    // Prettier → Monaco fallback). The global keybinding-bridge dispatches
    // the same command for editor-internal contexts, but registering it on
    // Monaco directly means it still works if a user disables the bridge.
    editor.addCommand(
      monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.KeyF,
      () => { formatActiveEditor(); }
    );

    // Register as the focused editor so global shortcuts (format etc.) can
    // act on the right buffer.
    setActiveEditor(editor, tabRef.current);
    setActiveSaver(stableSaver);
    editor.onDidFocusEditorWidget(() => {
      setActiveEditor(editor, tabRef.current);
      setActiveSaver(stableSaver);
    });

    editor.onDidChangeCursorPosition((e) => {
      setCursor(e.position.lineNumber, e.position.column);
      saveCursorForTab(tab.id, e.position.lineNumber, e.position.column);
    });

    editor.onDidDispose(() => {
      clearActiveEditor(editor);
      clearActiveSaver(stableSaver);
    });
  }, [applyContent, setCursor, saveCursorForTab, tab.id]);

  // Reactively push every editor setting to Monaco when settings change.
  // We pass the full options each time — Monaco diffs internally and only
  // applies actual changes, so this is cheap.
  useEffect(() => {
    if (!editorRef.current || !editorSettings) return;
    editorRef.current.updateOptions(buildEditorOptions(editorSettings));
    applyModelOptions(editorRef.current, editorSettings);
  }, [editorSettings]);

  const save = useCallback(async () => {
    const editor = editorRef.current;
    if (!editor || !tab.path) return;
    // Format-on-save: prefer an external formatter (rustfmt / ruff / shfmt /
    // prettier / etc.) configured in the Formatters modal. Falling back to
    // Monaco's built-in formatDocument covers JS/TS/JSON/CSS/HTML out of the
    // box for users who haven't set anything up.
    // Read format-on-save freshly from the store at save time rather than via
    // a render-driven ref — guards against any subscription staleness where
    // a toggle-off hasn't propagated to this closure's view of settings.
    const liveSettings = useSettings.getState().settings?.editor;
    if (liveSettings?.format_on_save !== false) {
      const lang = tab.language || 'plaintext';
      const source = editor.getValue();
      let formatted = null;

      // Resolution order:
      //   1. Backend `formatter_format` — covers custom user formatters AND
      //      installed/detected built-ins (rustfmt, ruff, shfmt, gofmt, etc.).
      //      A custom formatter for a Prettier-handled language takes
      //      precedence over the bundle here.
      //   2. Bundled Prettier — runs in-worker for JS/TS/CSS/HTML/MD/YAML
      //      when nothing in (1) matched.
      //   3. Monaco's built-in formatDocument as the final fallback.
      try {
        const res = await invoke('formatter_format', {
          req: { language: lang, source, file_path: tab.path },
        });
        if (res?.formatted !== undefined) formatted = res.formatted;
      } catch (err) {
        const msg = String(err).toLowerCase();
        if (!msg.includes('no formatter configured')) {
          toast.error(`Formatter failed: ${err}`);
        }
      }

      if (formatted === null && isPrettierLanguage(lang)) {
        try {
          formatted = await formatWithPrettier(lang, source, {
            filepath: tab.path,
            tabWidth: editorSettings?.tab_size ?? 4,
            useTabs: !(editorSettings?.insert_spaces !== false),
          });
        } catch (err) {
          // Real syntax errors should be surfaced; silent "no parser" is
          // already filtered out by isPrettierLanguage gating.
          toast.error(`Prettier: ${err.message ?? err}`);
        }
      }

      if (formatted !== null && formatted !== source) {
        applyFormattedContent(editor, formatted);
      } else if (formatted === null) {
        // No external formatter handled it — let Monaco's built-in providers
        // try (covers JSON/CSS/HTML/TS via the Monaco language workers, which
        // is useful as a fallback if Prettier loading fails for any reason).
        try { await editor.getAction('editor.action.formatDocument')?.run(); } catch {}
      }
    }
    const value = editor.getValue();
    try {
      await writeTextFile(tab.path, value);
      originalRef.current = value;
      setDirty(tab.id, false);
      toast.success(`Saved ${tab.title}`);
      try {
        await invoke('buffer_external_change', { bufferId: 0, path: tab.path });
      } catch {}
    } catch (err) {
      toast.error(`Save failed: ${err}`);
    }
  }, [tab.id, tab.path, tab.title, setDirty]);

  // Keep saveRef pointing at the latest save() closure so stableSaver always
  // calls into the up-to-date logic.
  saveRef.current = save;

  const handleChange = useCallback(
    (value) => {
      const dirty = (value ?? '') !== originalRef.current;
      setDirty(tab.id, dirty);
    },
    [tab.id, setDirty]
  );

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        Failed to open: {error}
      </div>
    );
  }

  return (
    <Suspense fallback={<Fallback />}>
      <div className="relative h-full w-full">
        <MonacoReact
          height="100%"
          theme="rustic-dark"
          language={tab.language || 'plaintext'}
          defaultValue=""
          onChange={handleChange}
          onMount={handleMount}
          options={DEFAULT_EDITOR_OPTIONS}
          loading={<Fallback />}
        />
        {loading && (
          <div className="absolute inset-0 z-10 bg-[#1e1e1e]">
            <Fallback />
          </div>
        )}
      </div>
    </Suspense>
  );
}
