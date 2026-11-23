import React, { forwardRef, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  PptxEditor,
  Icon,
  Popover,
  LayoutGallery,
  usePptxEditor,
  useEditorState,
  useTranslation,
  createDocumentRefresh,
} from '@pptx-editor.dev/react';
import {
  onEditorEvent,
  focusCurrentSlideOrCanvas,
  applyLocalFonts,
  canQueryLocalFonts,
} from '@pptx-editor.dev/core/editor';
import { bundledFonts } from './pptx-fonts';
import '@pptx-editor.dev/react/styles.css';
import './pptx-preview.css';

export { createDocumentRefresh, onEditorEvent };

// Matches the collapse transition in pptx-preview.css.
const COLLAPSE_MS = 240;

// Material Symbols "warning"; the editor's icon set has none.
const WARNING_ICON = [
  'm40-120 440-760 440 760H40Zm138-80h604L480-720 178-200Zm302-40q17 0 28.5-11.5T520-280q0-17-11.5-28.5T480-320q-17 0-28.5 11.5T440-280q0 17 11.5 28.5T480-240Zm-40-120h80v-200h-80v200Zm40-100Z',
];

/** Animates its child open and closed by height; closed content cannot be focused. */
function Collapse({ open, className, children }) {
  return (
    <div
      className={`rustic-pptx-collapse ${className}`}
      data-open={open ? 'true' : 'false'}
      inert={!open}
    >
      <div className="rustic-pptx-collapse__body">{children}</div>
    </div>
  );
}

/** Button that opens the layout gallery and adds a slide with the picked layout. */
function AddWithLayout() {
  const editor = usePptxEditor();
  const { t } = useTranslation();
  const viewing = useEditorState((s) => s.mode === 'view');
  const wrap = useRef(null);
  const [anchor, setAnchor] = useState(null);
  return (
    <span ref={wrap} className="rustic-pptx-contents">
      <PptxEditor.SlideRail.Action
        label={t('rail.chooseLayout')}
        icon={<Icon name="note_add" />}
        active={anchor ? true : undefined}
        disabled={viewing}
        disabledReason={t('disabled.viewing')}
        onSelect={() => setAnchor((shown) => (shown ? null : wrap.current?.querySelector('button') ?? null))}
      />
      {anchor && (
        <Popover anchor={anchor} onClose={() => setAnchor(null)} label={t('rail.chooseLayout')}>
          <LayoutGallery
            onChange={(layoutId) => {
              setAnchor(null);
              editor?.exec({ type: 'addSlide', layoutId });
            }}
          />
        </Popover>
      )}
    </span>
  );
}

/** The rail's slide actions, shown in the control row instead of beside the thumbnails. */
function SlideActions({ target }) {
  // The parts read the rail's action context, so they stay the rail's React
  // children and only their DOM moves, through a portal.
  if (!target) return null;
  const Rail = PptxEditor.SlideRail;
  return createPortal(
    <>
      <Rail.AddSlide />
      <AddWithLayout />
      <Rail.Duplicate />
      <Rail.Delete />
      <span className="rustic-pptx-sep" aria-hidden="true" />
      <Rail.MoveEarlier />
      <Rail.MoveLater />
    </>,
    target,
  );
}

/** Warning icon in the control row; a click lists the fonts drawn with a stand-in. */
function SubstitutedFonts() {
  const editor = usePptxEditor();
  const { t } = useTranslation();
  const substituted = useEditorState((s) => s.substitutedFonts);
  const status = useEditorState((s) => s.status);
  const wrap = useRef(null);
  const [anchor, setAnchor] = useState(null);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState(null);
  // A stand-in with the same letter widths keeps the layout, so it is not reported.
  const missing = (substituted ?? []).filter((font) => !font.metricCompatible);
  const none = missing.length === 0;

  useEffect(() => setOutcome(null), [status]);
  useEffect(() => {
    if (none) setAnchor(null);
  }, [none]);

  if (none) return null;

  const useLocal = async () => {
    if (!editor) return;
    setBusy(true);
    try {
      setOutcome(t(await applyLocalFonts(editor, missing.map((font) => font.family))));
    } catch {
      setOutcome(null);
    } finally {
      setBusy(false);
    }
  };

  const label = missing.length === 1 ? '1 font substituted' : `${missing.length} fonts substituted`;
  return (
    <div ref={wrap} className="pptx-zoom rustic-pptx-pill">
      <PptxEditor.StatusBar.Action
        label={label}
        icon={<Icon paths={WARNING_ICON} className="pptx-icon rustic-pptx-warn" />}
        active={anchor ? true : undefined}
        onSelect={() => setAnchor((shown) => (shown ? null : wrap.current?.querySelector('button') ?? null))}
      />
      {anchor && (
        <Popover
          anchor={anchor}
          onClose={() => setAnchor(null)}
          align="end"
          role="dialog"
          label={label}
          className="rustic-pptx-fonts"
        >
          <p className="rustic-pptx-fonts__title">{label}</p>
          <p className="rustic-pptx-fonts__hint">
            The editor has no font files for these, so their text is drawn in a stand-in and may wrap differently.
          </p>
          <ul className="rustic-pptx-fonts__list">
            {missing.map((font) => (
              <li key={font.family}>
                <span>{font.family}</span>
                <span className="rustic-pptx-fonts__substitute">{font.substitute}</span>
              </li>
            ))}
          </ul>
          {canQueryLocalFonts() && (
            <button
              type="button"
              className="rustic-pptx-fonts__action"
              disabled={busy || status === 'loading'}
              onClick={useLocal}
            >
              {t('fonts.useLocal')}
            </button>
          )}
          {outcome && <p className="rustic-pptx-fonts__hint">{outcome}</p>}
        </Popover>
      )}
    </div>
  );
}

/** Slide, collapsible speaker notes, one control row, and the slide strip along the bottom. */
function Workspace() {
  const editor = usePptxEditor();
  const { t } = useTranslation();
  const view = useEditorState((s) => s.presentationView ?? 'normal');
  const sorter = view === 'slideSorter';
  const master = view === 'slideMaster';
  const [railOpen, setRailOpen] = useState(true);
  const [notesOpen, setNotesOpen] = useState(false);
  const [notesField, setNotesField] = useState(false);
  const [actionsEl, setActionsEl] = useState(null);
  const box = useRef(null);
  const lastView = useRef(view);

  // The notes field stays mounted until the closing animation has finished.
  useEffect(() => {
    if (notesOpen) {
      setNotesField(true);
      return undefined;
    }
    const id = setTimeout(() => setNotesField(false), COLLAPSE_MS);
    return () => clearTimeout(id);
  }, [notesOpen]);

  // A view switch unmounts the focused part; put the focus back on the slide.
  useEffect(() => {
    if (lastView.current === view) return;
    lastView.current = view;
    const active = box.current?.ownerDocument.activeElement;
    if (active && active !== active.ownerDocument.body && active.isConnected) return;
    focusCurrentSlideOrCanvas(box.current, editor);
  }, [view, editor]);

  return (
    <div ref={box} className="pptx-default__views rustic-pptx-views">
      <div className="pptx-default__main" data-view={view}>
        {sorter ? (
          <>
            <PptxEditor.SlideSorter />
            <PptxEditor.EditNotice />
          </>
        ) : (
          <>
            <PptxEditor.Content />
            <PptxEditor.FindBar />
            <PptxEditor.EditNotice />
            <PptxEditor.Loading overlay />
          </>
        )}
      </div>
      <PptxEditor.OpenFailure />

      {!sorter && (
        <Collapse open={notesOpen} className="rustic-pptx-notes">
          <PptxEditor.NotesPane open={notesOpen || notesField} onOpenChange={setNotesOpen} />
        </Collapse>
      )}

      <div className="rustic-pptx-dock">
        <PptxEditor.StatusBar preset={false} className="rustic-pptx-bar">
          {!sorter && (
            <div className="pptx-zoom rustic-pptx-pill">
              <span ref={setActionsEl} className="rustic-pptx-contents" />
              <span className="rustic-pptx-sep" aria-hidden="true" />
              <PptxEditor.StatusBar.Action
                label={railOpen ? t('rail.collapse') : t('rail.expand')}
                icon={<Icon name="keyboard_arrow_down" className="pptx-icon rustic-pptx-chevron" />}
                className={railOpen ? undefined : 'is-closed'}
                onSelect={() => setRailOpen((open) => !open)}
              />
            </div>
          )}
          <span className="pptx-statusbar__spacer" />
          <SubstitutedFonts />
          {!sorter && !master && (
            <div className="pptx-zoom rustic-pptx-pill">
              <PptxEditor.StatusBar.Action
                label={notesOpen ? t('notes.collapse') : t('notes.expand')}
                icon={<Icon name="edit_note" />}
                active={notesOpen}
                onSelect={() => setNotesOpen((open) => !open)}
              />
            </div>
          )}
          <PptxEditor.StatusBar.Views />
          <PptxEditor.StatusBar.Zoom />
        </PptxEditor.StatusBar>
      </div>

      {!sorter && (
        <Collapse open={railOpen} className="rustic-pptx-strip">
          <PptxEditor.SlideRail orientation="horizontal" toggle={false} preset={false}>
            <SlideActions target={actionsEl} />
          </PptxEditor.SlideRail>
        </Collapse>
      )}
    </div>
  );
}

/** The PPTX editor composed from its parts, with Rustic's bottom layout. */
export const PptxWorkspace = forwardRef(function PptxWorkspace({ bytes, colorMode, onReady }, ref) {
  return (
    <PptxEditor.Root ref={ref} document={bytes} colorMode={colorMode} fonts={bundledFonts} onReady={onReady}>
      <PptxEditor.Viewport className="pptx-default rustic-pptx">
        <div className="pptx-default__dock">
          <PptxEditor.Toolbar />
        </div>
        <Workspace />
        <PptxEditor.Slideshow />
      </PptxEditor.Viewport>
    </PptxEditor.Root>
  );
});
