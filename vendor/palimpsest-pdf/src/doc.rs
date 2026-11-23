//! The document: loading, page geometry, the cached page model and stable object ids.

use std::collections::{HashMap, HashSet, VecDeque};

use lopdf::{Document as Pdf, Object, ObjectId};

use crate::content::{lex, K};
use crate::geom::{Matrix, Rect};
use crate::interp::{interpret, FontCache, Interp, Limits, Raw, RawKind, StreamKey};
use crate::model::{
    GlyphBox, ImageInfo, ObjId, ObjectKind, PageObject, PageObjects, PageText, TextRun,
};
use crate::util::{get, get_dict, get_rect, num, stream_data};
use crate::{Error, PageSummary, Result};

/// Document-level facts the host should know before offering edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocInfo {
    pub pages: usize,
    /// PDF version from the header, for example `1.7`.
    pub version: String,
    /// The file was encrypted; it is saved without encryption.
    pub was_encrypted: bool,
    /// The file carries a digital signature, which any edit invalidates.
    pub signed: bool,
    /// The file is a tagged PDF. Edits keep marked-content sequences in place, so the
    /// structure tree stays consistent.
    pub tagged: bool,
}

impl DocInfo {
    /// Serialises the info as JSON.
    pub fn to_json(&self) -> String {
        let mut out = format!("{{\"pages\":{},\"version\":", self.pages);
        crate::model::json_str(&mut out, &self.version);
        out.push_str(&format!(
            ",\"was_encrypted\":{},\"signed\":{},\"tagged\":{}}}",
            self.was_encrypted, self.signed, self.tagged
        ));
        out
    }
}

/// Ties a stable object id to the operation that paints the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub id: ObjId,
    pub key: StreamKey,
    pub start: u32,
}

/// The interpreted form of one page.
pub(crate) struct PageState {
    pub interp: Interp,
    /// Maps default user space to page space.
    pub to_page: Matrix,
    pub objects: PageObjects,
    layout: std::sync::OnceLock<crate::layout::Layout>,
}

impl PageState {
    /// The page's lines and blocks, computed on first use.
    pub(crate) fn layout(&self) -> &crate::layout::Layout {
        self.layout
            .get_or_init(|| crate::layout::Layout::build(self))
    }
}

/// One undoable step: objects and anchors as they were before it.
pub(crate) struct Step {
    pub label: &'static str,
    pub page: u32,
    pub objects: Vec<(ObjectId, Option<Object>)>,
    pub anchors: Vec<(u32, Option<Vec<Anchor>>)>,
}

/// A PDF document loaded into memory.
pub struct Document {
    pub(crate) pdf: Pdf,
    pub(crate) pages: Vec<ObjectId>,
    pub(crate) fonts: FontCache,
    pub(crate) limits: Limits,
    pub(crate) cache: HashMap<u32, PageState>,
    pub(crate) anchors: HashMap<u32, Vec<Anchor>>,
    /// The next unused object id of each page. Ids are per page, so that a freshly
    /// loaded document numbers a page the same way whichever pages were read before it.
    pub(crate) next_ids: HashMap<u32, ObjId>,
    /// Content streams created by the engine, which it may rewrite in place.
    pub(crate) owned: HashSet<ObjectId>,
    /// Streams the engine stored uncompressed; they are compressed on save.
    pub(crate) plain: HashSet<ObjectId>,
    pub(crate) undo: Vec<Step>,
    pub(crate) redo: Vec<Step>,
    pub(crate) history_limit: usize,
    pub(crate) was_encrypted: bool,
    /// Fonts the host registered for characters that embedded fonts lack.
    pub(crate) fallbacks: Vec<crate::embed::Fallback>,
}

/// Runs `f`, turning a panic inside a dependency into an error where unwinding is enabled.
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_) => Err(Error::Parse(format!("{what} failed on malformed data"))),
    }
}

impl Document {
    /// Parses a PDF from its raw bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::open(bytes, None)
    }

    /// Parses a PDF, decrypting it with `password` when it is encrypted.
    pub fn open(bytes: &[u8], password: Option<&str>) -> Result<Self> {
        let encrypted_hint = bytes
            .windows(8)
            .rev()
            .take(1 << 20)
            .any(|w| w == b"/Encrypt");
        let pdf = guarded("parsing", || {
            let options = lopdf::LoadOptions {
                password: password.map(str::to_string),
                ..Default::default()
            };
            Pdf::load_mem_with_options(bytes, options).map_err(|e| match e {
                lopdf::Error::InvalidPassword => Error::WrongPassword,
                other => Error::Parse(other.to_string()),
            })
        })?;
        if pdf.trailer.get(b"Encrypt").is_ok() {
            return Err(if password.is_some() {
                Error::WrongPassword
            } else {
                Error::PasswordRequired
            });
        }
        let pages = guarded("reading the page tree", || {
            Ok(pdf.get_pages().into_values().collect())
        })?;
        Ok(Document {
            was_encrypted: pdf.encryption_state.is_some()
                || (encrypted_hint && pdf.was_encrypted()),
            pdf,
            pages,
            fonts: FontCache::default(),
            limits: Limits::default(),
            cache: HashMap::new(),
            anchors: HashMap::new(),
            next_ids: HashMap::new(),
            owned: HashSet::new(),
            plain: HashSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            history_limit: 200,
            fallbacks: Vec::new(),
        })
    }

    /// Replaces the resource budgets used when interpreting pages.
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
        self.cache.clear();
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Reports encryption, signatures and tagging.
    pub fn info(&self) -> DocInfo {
        let catalog = self.pdf.catalog().ok();
        let acro = catalog.and_then(|c| get_dict(&self.pdf, c, b"AcroForm"));
        let sig_flags = acro
            .and_then(|a| get(&self.pdf, a, b"SigFlags"))
            .and_then(num)
            .unwrap_or(0.0) as u32;
        let signed = sig_flags & 1 != 0
            || catalog
                .is_some_and(|c| get_dict(&self.pdf, c, b"Perms").is_some_and(|p| !p.is_empty()))
            || self.pdf.objects.values().any(|o| match o {
                Object::Dictionary(d) => d.has(b"ByteRange") && d.has(b"Contents"),
                _ => false,
            });
        let marked = catalog
            .and_then(|c| get_dict(&self.pdf, c, b"MarkInfo"))
            .and_then(|m| get(&self.pdf, m, b"Marked"))
            .is_some_and(|m| m.as_bool().unwrap_or(false));
        DocInfo {
            pages: self.pages.len(),
            version: self.pdf.version.clone(),
            was_encrypted: self.was_encrypted,
            signed,
            tagged: marked || catalog.is_some_and(|c| c.has(b"StructTreeRoot")),
        }
    }

    /// Resolves a 1-based page number.
    pub(crate) fn page_id(&self, page: u32) -> Result<ObjectId> {
        let index = (page as usize)
            .checked_sub(1)
            .ok_or(Error::NoSuchPage(page))?;
        self.pages
            .get(index)
            .copied()
            .ok_or(Error::NoSuchPage(page))
    }

    /// Looks up a page attribute, following `Parent` links for inheritable ones.
    pub(crate) fn inherited(&self, page_id: ObjectId, key: &[u8]) -> Option<&Object> {
        inherited(&self.pdf, page_id, key)
    }

    /// The decoded content of a page, all `Contents` streams joined by newlines.
    pub(crate) fn page_content(&self, page_id: ObjectId) -> Vec<u8> {
        let mut out = Vec::new();
        for id in self.pdf.get_page_contents(page_id) {
            if let Some(Object::Stream(stream)) = self.pdf.objects.get(&id) {
                let room = self.limits.max_stream_bytes.saturating_sub(out.len());
                if let Some(data) = stream_data(stream, room) {
                    out.extend_from_slice(&data);
                    out.push(b'\n');
                }
            }
        }
        out
    }

    /// Visible page size, rotation and the matrix from default user space to page space.
    pub(crate) fn page_geometry(&self, page_id: ObjectId) -> (f64, f64, u16, Matrix) {
        let rect = |key: &[u8]| {
            let mut dict = self.pdf.get_dictionary(page_id).ok()?;
            for _ in 0..64 {
                if let Some(r) = get_rect(&self.pdf, dict, key) {
                    return Some(r);
                }
                dict = get_dict(&self.pdf, dict, b"Parent")?;
            }
            None
        };
        let media =
            rect(b"MediaBox").filter(|r| r.is_finite() && r.width() > 0.0 && r.height() > 0.0);
        let media = media.unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
        let crop = rect(b"CropBox")
            .map(|c| c.intersect(&media))
            .filter(|c| c.width() > 0.0 && c.height() > 0.0);
        let view = crop.unwrap_or(media);
        let turns = self
            .inherited(page_id, b"Rotate")
            .and_then(num)
            .unwrap_or(0.0);
        let rotate = ((turns / 90.0).round() as i64).rem_euclid(4) as u16 * 90;
        let (w, h) = (view.width(), view.height());
        let origin = Matrix::translate(-view.x0, -view.y0);
        let (turn, pw, ph) = match rotate {
            90 => (Matrix([0.0, -1.0, 1.0, 0.0, 0.0, w]), h, w),
            180 => (Matrix([-1.0, 0.0, 0.0, -1.0, w, h]), w, h),
            270 => (Matrix([0.0, 1.0, -1.0, 0.0, h, 0.0]), h, w),
            _ => (Matrix::IDENTITY, w, h),
        };
        (pw, ph, rotate, origin.then(&turn))
    }

    /// Summarises the content stream of every page, in page order.
    pub fn inspect(&self) -> Result<Vec<PageSummary>> {
        let mut out = Vec::with_capacity(self.pages.len());
        for (index, page_id) in self.pages.iter().enumerate() {
            let content = self.page_content(*page_id);
            let lexed = lex(&content, self.limits.max_ops);
            let count = |f: fn(K) -> bool| lexed.ops.iter().filter(|op| f(op.k)).count();
            out.push(PageSummary {
                number: index as u32 + 1,
                operations: lexed.ops.len(),
                text_show_ops: count(|k| matches!(k, K::Tj | K::TJ | K::Quote | K::DQuote)),
                xobject_paint_ops: count(|k| k == K::Do),
            });
        }
        Ok(out)
    }
}

/// Looks up a page attribute, following `Parent` links for inheritable ones.
pub(crate) fn inherited<'a>(pdf: &'a Pdf, page_id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut dict = pdf.get_dictionary(page_id).ok()?;
    for _ in 0..64 {
        if let Some(value) = get(pdf, dict, key) {
            return Some(value);
        }
        dict = get_dict(pdf, dict, b"Parent")?;
    }
    None
}

/// Gives every item an id, reusing the ids of anchors that still point at the same operation.
fn assign_ids(old: Option<&Vec<Anchor>>, interp: &Interp, next_id: &mut ObjId) -> Vec<Anchor> {
    let mut pool: HashMap<(StreamKey, u32), VecDeque<ObjId>> = HashMap::new();
    for a in old.into_iter().flatten() {
        pool.entry((a.key, a.start)).or_default().push_back(a.id);
    }
    let mut out = Vec::with_capacity(interp.items.len());
    for item in &interp.items {
        let key = interp
            .streams
            .get(item.stream as usize)
            .map_or(StreamKey::Page, |s| s.key);
        let id = pool
            .get_mut(&(key, item.start))
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| {
                let id = *next_id;
                *next_id = next_id.saturating_add(1);
                id
            });
        out.push(Anchor {
            id,
            key,
            start: item.start,
        });
    }
    out
}

/// Builds the public object for one interpreted item.
fn build_object(item: &Raw, id: ObjId, z: u32, to_page: &Matrix) -> PageObject {
    let corners = |m: &Matrix, x0: f64, y0: f64, x1: f64, y1: f64| {
        [
            m.apply(x0, y0),
            m.apply(x1, y0),
            m.apply(x1, y1),
            m.apply(x0, y1),
        ]
    };
    match &item.kind {
        RawKind::Image(image) => {
            let m = item.ctm.then(to_page);
            PageObject {
                id,
                z,
                bbox: Rect::quad_bbox(&m, 0.0, 0.0, 1.0, 1.0),
                quad: corners(&m, 0.0, 0.0, 1.0, 1.0),
                in_form: item.depth > 0,
                kind: ObjectKind::Image(ImageInfo {
                    pixel_width: image.width,
                    pixel_height: image.height,
                    inline: image.xobject.is_none(),
                }),
            }
        }
        RawKind::Text(run) => {
            let m = run.tm.then(&item.ctm).then(to_page);
            let (lo, hi) = (
                run.font.descent * run.size + run.rise,
                run.font.ascent * run.size + run.rise,
            );
            let (mut x0, mut x1) = (f64::MAX, f64::MIN);
            let mut glyphs = Vec::with_capacity(run.glyphs.len());
            for g in &run.glyphs {
                let (a, b) = (g.x, g.x + f64::from(g.w));
                x0 = x0.min(a.min(b));
                x1 = x1.max(a.max(b));
                glyphs.push(GlyphBox {
                    bbox: Rect::quad_bbox(&m, a, lo, b, hi),
                    offset: g.at,
                });
            }
            if x0 > x1 {
                (x0, x1) = (0.0, 0.0);
            }
            let (ux, uy) = m.apply_vec(1.0, 0.0);
            let (vx, vy) = m.apply_vec(0.0, 1.0);
            PageObject {
                id,
                z,
                bbox: Rect::quad_bbox(&m, x0, lo, x1, hi),
                quad: corners(&m, x0, lo, x1, hi),
                in_form: item.depth > 0,
                kind: ObjectKind::Text(TextRun {
                    text: run.text.clone(),
                    font: run.font.name.clone(),
                    size: (run.size * vx.hypot(vy)).abs(),
                    color: run.fill,
                    bold: run.font.bold,
                    italic: run.font.italic,
                    invisible: run.render == 3,
                    origin: m.apply(0.0, run.rise),
                    angle: uy.atan2(ux).to_degrees(),
                    unmapped: run.unmapped,
                    glyphs,
                }),
            }
        }
    }
}

/// Decides what kind of text a page carries.
fn classify(interp: &Interp, objects: &[PageObject], width: f64, height: f64) -> PageText {
    let mut visible = false;
    let mut hidden = false;
    let mut covered = 0.0f64;
    for o in objects {
        match &o.kind {
            ObjectKind::Text(t) if t.invisible => hidden = true,
            ObjectKind::Text(_) => visible = true,
            ObjectKind::Image(_) => covered = covered.max(o.bbox.width() * o.bbox.height()),
        }
    }
    let scanned = covered >= 0.5 * width * height;
    if visible {
        PageText::Text
    } else if hidden {
        PageText::HiddenText
    } else if scanned {
        PageText::Scanned
    } else if interp.path_ops >= 400 {
        PageText::Outlined
    } else {
        PageText::Empty
    }
}

impl Document {
    /// Interprets a page on first use and caches the result.
    pub(crate) fn state(&mut self, page: u32) -> Result<&PageState> {
        let page_id = self.page_id(page)?;
        if !self.cache.contains_key(&page) {
            let content = self.page_content(page_id);
            let (width, height, rotate, to_page) = self.page_geometry(page_id);
            let resources =
                inherited(&self.pdf, page_id, b"Resources").and_then(crate::util::as_dict);
            let interp = interpret(&self.pdf, &mut self.fonts, content, resources, self.limits);
            let anchors = assign_ids(
                self.anchors.get(&page),
                &interp,
                self.next_ids.entry(page).or_insert(1),
            );
            let objects: Vec<PageObject> = interp
                .items
                .iter()
                .zip(&anchors)
                .enumerate()
                .map(|(z, (item, a))| build_object(item, a.id, z as u32, &to_page))
                .collect();
            let mut warnings = Vec::new();
            for name in &interp.unsupported_fonts {
                warnings.push(format!(
                    "font {name}: predefined CMap is not built in; its text may be misread"
                ));
            }
            if interp.truncated {
                warnings
                    .push("page exceeded a resource limit; some objects are missing".to_string());
            }
            let objects = PageObjects {
                page,
                width,
                height,
                rotate,
                text: classify(&interp, &objects, width, height),
                objects,
                truncated: interp.truncated,
                warnings,
            };
            self.anchors.insert(page, anchors);
            self.cache.insert(
                page,
                PageState {
                    interp,
                    to_page,
                    objects,
                    layout: std::sync::OnceLock::new(),
                },
            );
        }
        self.cache.get(&page).ok_or(Error::NoSuchPage(page))
    }

    /// Returns the text runs and images of a 1-based page, in paint order.
    pub fn page_objects(&mut self, page: u32) -> Result<&PageObjects> {
        self.state(page).map(|s| &s.objects)
    }

    /// Finds the index of an object on a page.
    pub(crate) fn locate(&mut self, page: u32, id: ObjId) -> Result<usize> {
        let state = self.state(page)?;
        state
            .objects
            .objects
            .iter()
            .position(|o| o.id == id)
            .ok_or(Error::NoSuchObject { page, id })
    }
}
