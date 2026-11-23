//! Content-stream interpreter: tracks graphics and text state and records every
//! text run and image together with the bytes of the operation that painted it.

use std::collections::HashMap;
use std::sync::Arc;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::content::{decode_name, decode_str, lex, name_of, Lexed, Op, Tok, K};
use crate::font::{Font, GlyphNames};
use crate::geom::{Matrix, Rect};
use crate::util::{
    as_dict, get_dict, get_matrix, get_name, get_num, get_rect, resolve, stream_data,
};

/// Resource budgets for interpreting one page.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Operations executed per page, counting nested forms each time they are painted.
    pub max_ops: usize,
    /// Glyphs recorded per page.
    pub max_glyphs: usize,
    /// Decoded size of a single content stream, in bytes.
    pub max_stream_bytes: usize,
    /// Form XObject nesting depth.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_ops: 4_000_000,
            max_glyphs: 1_000_000,
            max_stream_bytes: 256 << 20,
            max_depth: 12,
        }
    }
}

/// A `TJ` adjustment wider than this, in 1/1000 em, reads as a word space.
pub(crate) const SPACE_GAP: f64 = 180.0;

/// Which stream an operation lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StreamKey {
    /// The page's own content, all `Contents` streams joined.
    Page,
    Form(ObjectId),
}

/// A decoded and tokenized content stream.
#[derive(Debug)]
pub(crate) struct StreamData {
    pub key: StreamKey,
    pub bytes: Vec<u8>,
    pub lexed: Lexed,
}

/// Fonts already decoded for a document.
#[derive(Debug, Default)]
pub(crate) struct FontCache {
    pub by_id: HashMap<ObjectId, Arc<Font>>,
    pub names: GlyphNames,
}

/// One glyph of a run, in text space relative to the run's start.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Glyph {
    pub x: f64,
    pub w: f32,
    pub code: u32,
    /// Byte offset of this glyph's text in the run's string.
    pub at: u32,
    /// Number of bytes the code occupies in the show string.
    pub len: u8,
    /// True when the run's text has a synthetic space after this glyph's text.
    pub gap: bool,
}

#[derive(Debug)]
pub(crate) struct RawText {
    pub show: K,
    /// Text matrix when the first glyph is painted.
    pub tm: Matrix,
    /// Text line matrix after the operation.
    pub tlm_after: Matrix,
    /// Horizontal displacement of the text matrix caused by the operation.
    pub advance: f64,
    pub font: Arc<Font>,
    pub font_name: Arc<[u8]>,
    pub size: f64,
    pub tc: f64,
    pub tw: f64,
    pub th: f64,
    pub rise: f64,
    pub render: u8,
    pub fill: Option<[f32; 3]>,
    pub glyphs: Vec<Glyph>,
    pub text: String,
    pub unmapped: u32,
    /// True when a later show in the same text object continues from this run's end.
    pub followed: bool,
    /// True when a positioning operator set the pen for this run, rather than the
    /// show before it.
    pub placed: bool,
    /// The stretch of the text-space x axis the run stands for when lines are formed,
    /// where it differs from what its glyphs cover: `(start, end)`. A text edit writes
    /// two numbers in a row at an end of the `TJ` array, the pen between them marking
    /// where the run began or ended before the edit, so that an edit of one cell does
    /// not regroup its row.
    pub reserve: (Option<f64>, Option<f64>),
}

#[derive(Debug)]
pub(crate) struct RawImage {
    pub xobject: Option<ObjectId>,
    pub width: u32,
    pub height: u32,
}

// Nearly every item on a page is a text run, so boxing the large variant would add an
// allocation per run and save almost nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum RawKind {
    Text(RawText),
    Image(RawImage),
}

/// A state-setting operation as `(stream index, start, end)`, replayed when an object is lifted.
pub(crate) type StateOp = (u32, u32, u32);

/// Indices into [`Raw::state`].
pub(crate) const STATE_GS: usize = 0;
pub(crate) const STATE_FILL_SPACE: usize = 1;
pub(crate) const STATE_FILL: usize = 2;
pub(crate) const STATE_STROKE_SPACE: usize = 3;
pub(crate) const STATE_STROKE: usize = 4;

/// One level of clipping in effect for an object.
#[derive(Debug)]
pub(crate) struct ClipNode {
    /// The `q` that opened the group this clip was set in, as its operation index and
    /// the CTM outside the group. `None` when the clip cannot be left from this stream.
    pub group: Option<(u32, Matrix)>,
    /// Everything painted is limited to this box, in default user space.
    pub rect: Rect,
    pub parent: Option<Arc<ClipNode>>,
}

/// A painted object and where its operation sits.
#[derive(Debug)]
pub(crate) struct Raw {
    /// Index into [`Interp::streams`].
    pub stream: u32,
    pub start: u32,
    pub end: u32,
    /// Maps the operation's user space to the page's default user space.
    pub ctm: Matrix,
    pub depth: u8,
    pub clip: Option<Arc<ClipNode>>,
    pub state: [Option<StateOp>; 5],
    pub kind: RawKind,
}

/// The result of interpreting a page.
#[derive(Debug, Default)]
pub(crate) struct Interp {
    pub streams: Vec<Arc<StreamData>>,
    pub items: Vec<Raw>,
    pub truncated: bool,
    pub show_ops: usize,
    pub path_ops: usize,
    /// Fonts whose encoding CMap is not built in, by name.
    pub unsupported_fonts: Vec<String>,
}

#[derive(Clone)]
struct GState {
    ctm: Matrix,
    fill: Option<[f32; 3]>,
    font: Option<Arc<Font>>,
    font_name: Arc<[u8]>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
    render: u8,
    clip: Option<Arc<ClipNode>>,
    ops: [Option<StateOp>; 5],
}

/// Reads the last `N` operands as numbers.
fn nums<const N: usize>(args: &[Tok]) -> Option<[f64; N]> {
    let tail = args.get(args.len().checked_sub(N)?..)?;
    let mut out = [0.0; N];
    for (slot, tok) in out.iter_mut().zip(tail) {
        match tok {
            Tok::Num(v) => *slot = *v,
            _ => return None,
        }
    }
    Some(out)
}

/// Converts fill-colour operands to RGB; `None` for patterns and unknown spaces.
fn fill_rgb(args: &[Tok]) -> Option<[f32; 3]> {
    if matches!(args.last(), Some(Tok::Name(_))) {
        return None;
    }
    let v: Vec<f32> = args
        .iter()
        .filter_map(|t| {
            if let Tok::Num(v) = t {
                Some(v.clamp(0.0, 1.0) as f32)
            } else {
                None
            }
        })
        .collect();
    match v.as_slice() {
        [g] => Some([*g, *g, *g]),
        [r, g, b] => Some([*r, *g, *b]),
        [c, m, y, k] => Some([
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        ]),
        _ => None,
    }
}

struct Machine<'a> {
    pdf: &'a Pdf,
    fonts: &'a mut FontCache,
    limits: Limits,
    budget: usize,
    glyph_budget: usize,
    fallback: Arc<Font>,
    form_stack: Vec<ObjectId>,
    buf: Vec<u8>,
    out: Interp,
}

/// Interprets a page's content with the given resources and initial matrix.
pub(crate) fn interpret(
    pdf: &Pdf,
    fonts: &mut FontCache,
    content: Vec<u8>,
    resources: Option<&Dictionary>,
    limits: Limits,
) -> Interp {
    let lexed = lex(&content, limits.max_ops);
    let mut m = Machine {
        pdf,
        fonts,
        limits,
        budget: limits.max_ops,
        glyph_budget: limits.max_glyphs,
        fallback: Arc::new(Font::fallback()),
        form_stack: Vec::new(),
        buf: Vec::new(),
        out: Interp::default(),
    };
    m.out.truncated = lexed.truncated;
    m.out.streams.push(Arc::new(StreamData {
        key: StreamKey::Page,
        bytes: content,
        lexed,
    }));
    let gs = GState {
        ctm: Matrix::IDENTITY,
        fill: Some([0.0; 3]),
        font: None,
        font_name: Arc::from(&b""[..]),
        size: 0.0,
        tc: 0.0,
        tw: 0.0,
        th: 1.0,
        tl: 0.0,
        rise: 0.0,
        render: 0,
        clip: None,
        ops: [None; 5],
    };
    m.run(0, resources, gs, 0);
    m.out
}

impl<'a> Machine<'a> {
    /// Resolves a font resource name, decoding the font on first use.
    fn font_for(&mut self, res: Option<&'a Dictionary>, name: &[u8]) -> Option<Arc<Font>> {
        let fonts = get_dict(self.pdf, res?, b"Font")?;
        let entry = fonts.get(name).ok()?;
        let id = entry.as_reference().ok();
        if let Some(font) = id.and_then(|id| self.fonts.by_id.get(&id)) {
            return Some(font.clone());
        }
        let dict = as_dict(resolve(self.pdf, entry))?;
        let font = Arc::new(Font::load(self.pdf, dict, &mut self.fonts.names));
        if font.unsupported_cmap && !self.out.unsupported_fonts.contains(&font.name) {
            self.out.unsupported_fonts.push(font.name.clone());
        }
        if let Some(id) = id {
            self.fonts.by_id.insert(id, font.clone());
        }
        Some(font)
    }

    /// Executes one stream. `res` is the resource dictionary in effect.
    fn run(&mut self, si: usize, res: Option<&'a Dictionary>, mut gs: GState, depth: usize) {
        let Some(sd) = self.out.streams.get(si).cloned() else {
            return;
        };
        let (src, lx) = (&sd.bytes, &sd.lexed);
        let mut stack: Vec<GState> = Vec::new();
        let mut overflow = 0usize;
        let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
        let mut last_show: Option<usize> = None;
        let mut moved = true;
        // The `q` operations currently open, with the CTM in effect outside each.
        let mut frames: Vec<(u32, Matrix)> = Vec::new();
        let mut path: Option<Rect> = None;
        let mut clip_pending = false;
        for (oi, op) in lx.ops.iter().enumerate() {
            if self.budget == 0 {
                self.out.truncated = true;
                return;
            }
            self.budget -= 1;
            let args = lx.args(op);
            let here = (si as u32, op.start, op.end);
            match op.k {
                K::Save => {
                    if stack.len() < 512 {
                        stack.push(gs.clone());
                        frames.push((oi as u32, gs.ctm));
                    } else {
                        overflow += 1;
                    }
                }
                K::Restore => {
                    if overflow > 0 {
                        overflow -= 1;
                    } else if let Some(prev) = stack.pop() {
                        gs = prev;
                        frames.pop();
                    }
                }
                K::Cm => {
                    if let Some(m) = nums::<6>(args).map(Matrix).filter(Matrix::is_finite) {
                        gs.ctm = m.then(&gs.ctm);
                    }
                }
                K::BT => {
                    tm = Matrix::IDENTITY;
                    tlm = Matrix::IDENTITY;
                    moved = true;
                }
                K::ET => moved = true,
                K::Tf => {
                    if let (Some(Tok::Name(sp)), Some([size])) = (args.first(), nums::<1>(args)) {
                        let name = decode_name(src, *sp);
                        gs.font = self.font_for(res, &name);
                        gs.font_name = Arc::from(name);
                        gs.size = size;
                    }
                }
                K::Td | K::TD => {
                    if let Some([tx, ty]) = nums::<2>(args) {
                        if op.k == K::TD {
                            gs.tl = -ty;
                        }
                        tlm = Matrix::translate(tx, ty).then(&tlm);
                        tm = tlm;
                        moved = true;
                    }
                }
                K::Tm => {
                    if let Some(m) = nums::<6>(args).map(Matrix).filter(Matrix::is_finite) {
                        tlm = m;
                        tm = m;
                        moved = true;
                    }
                }
                K::TStar => {
                    tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                    tm = tlm;
                    moved = true;
                }
                K::Tc => gs.tc = nums::<1>(args).map_or(gs.tc, |v| v[0]),
                K::Tw => gs.tw = nums::<1>(args).map_or(gs.tw, |v| v[0]),
                K::Tz => gs.th = nums::<1>(args).map_or(gs.th, |v| v[0] / 100.0),
                K::TL => gs.tl = nums::<1>(args).map_or(gs.tl, |v| v[0]),
                K::Ts => gs.rise = nums::<1>(args).map_or(gs.rise, |v| v[0]),
                K::Tr => {
                    gs.render = nums::<1>(args).map_or(gs.render, |v| v[0].clamp(0.0, 7.0) as u8)
                }
                K::Tj | K::TJ | K::Quote | K::DQuote => {
                    self.out.show_ops += 1;
                    if matches!(op.k, K::Quote | K::DQuote) {
                        if let (K::DQuote, Some(Tok::Num(aw)), Some(Tok::Num(ac))) =
                            (op.k, args.first(), args.get(1))
                        {
                            gs.tw = *aw;
                            gs.tc = *ac;
                        }
                        tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                        tm = tlm;
                        moved = true;
                    }
                    if !moved {
                        if let Some(RawKind::Text(prev)) = last_show
                            .and_then(|i| self.out.items.get_mut(i))
                            .map(|r| &mut r.kind)
                        {
                            prev.followed = true;
                        }
                    }
                    if let Some(index) =
                        self.show(&sd.bytes, si, op, args, &gs, &mut tm, tlm, depth)
                    {
                        if let Some(RawKind::Text(run)) =
                            self.out.items.get_mut(index).map(|r| &mut r.kind)
                        {
                            run.placed = moved;
                        }
                        last_show = Some(index);
                        moved = false;
                    }
                }
                K::Do => {
                    if let Some(Tok::Name(sp)) = args.last() {
                        let name = decode_name(src, *sp);
                        self.paint_xobject(si, op, name, res, &gs, depth);
                    }
                }
                K::Inline => {
                    let mut dims = [0u32; 2];
                    for pair in args.windows(2) {
                        if let (Tok::Name(k), Tok::Num(v)) = (pair[0], pair[1]) {
                            match name_of(src, k) {
                                b"W" | b"Width" => dims[0] = v.max(0.0) as u32,
                                b"H" | b"Height" => dims[1] = v.max(0.0) as u32,
                                _ => {}
                            }
                        }
                    }
                    let image = RawImage {
                        xobject: None,
                        width: dims[0],
                        height: dims[1],
                    };
                    self.push(si, op, &gs, depth, RawKind::Image(image));
                }
                K::FillColor | K::StrokeColor => {
                    let fill = op.k == K::FillColor;
                    if fill {
                        gs.fill = fill_rgb(args);
                    }
                    let (space, color) = if fill {
                        (STATE_FILL_SPACE, STATE_FILL)
                    } else {
                        (STATE_STROKE_SPACE, STATE_STROKE)
                    };
                    // `g`, `rg` and `k` select their own colour space.
                    if !matches!(name_of(src, op.name), b"sc" | b"scn" | b"SC" | b"SCN") {
                        gs.ops[space] = None;
                    }
                    gs.ops[color] = Some(here);
                }
                K::FillSpace => {
                    let pattern = matches!(args.last(), Some(Tok::Name(sp)) if name_of(src, *sp) == b"Pattern");
                    gs.fill = (!pattern).then_some([0.0; 3]);
                    gs.ops[STATE_FILL_SPACE] = Some(here);
                    gs.ops[STATE_FILL] = None;
                }
                K::StrokeSpace => {
                    gs.ops[STATE_STROKE_SPACE] = Some(here);
                    gs.ops[STATE_STROKE] = None;
                }
                K::Gs => gs.ops[STATE_GS] = Some(here),
                K::MoveTo | K::LineTo | K::Curve | K::Rect => {
                    self.out.path_ops += 1;
                    let mut add = |x: f64, y: f64| {
                        let (px, py) = gs.ctm.apply(x, y);
                        match &mut path {
                            Some(r) => r.add_point(px, py),
                            None => path = Some(Rect::new(px, py, px, py)),
                        }
                    };
                    if op.k == K::Rect {
                        if let Some([x, y, w, h]) = nums::<4>(args) {
                            for (dx, dy) in [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)] {
                                add(x + dx, y + dy);
                            }
                        }
                    } else {
                        for pair in args.chunks_exact(2) {
                            if let (Tok::Num(x), Tok::Num(y)) = (pair[0], pair[1]) {
                                add(x, y);
                            }
                        }
                    }
                }
                K::Clip => clip_pending = true,
                K::EndPath | K::Paint => {
                    if let (true, Some(area)) = (clip_pending, path.filter(Rect::is_finite)) {
                        let group = frames.last().copied();
                        let same_group = |n: &Arc<ClipNode>| {
                            n.group.map(|g| g.0) == group.map(|g| g.0) && n.group.is_some()
                        };
                        let node = match gs.clip.take() {
                            Some(top)
                                if same_group(&top) || (group.is_none() && top.group.is_none()) =>
                            {
                                ClipNode {
                                    group: top.group,
                                    rect: top.rect.intersect(&area),
                                    parent: top.parent.clone(),
                                }
                            }
                            Some(top) => ClipNode {
                                group,
                                rect: top.rect.intersect(&area),
                                parent: Some(top),
                            },
                            None => ClipNode {
                                group,
                                rect: area,
                                parent: None,
                            },
                        };
                        gs.clip = Some(Arc::new(node));
                    }
                    clip_pending = false;
                    path = None;
                }
                _ => {}
            }
        }
    }

    /// Records an item for the operation `op` of stream `si`.
    fn push(&mut self, si: usize, op: &Op, gs: &GState, depth: usize, kind: RawKind) -> usize {
        self.out.items.push(Raw {
            stream: si as u32,
            start: op.start,
            end: op.end,
            ctm: gs.ctm,
            depth: depth as u8,
            clip: gs.clip.clone(),
            state: gs.ops,
            kind,
        });
        self.out.items.len() - 1
    }
}

impl<'a> Machine<'a> {
    /// Lays out the glyphs of a show operation and advances the text matrix.
    #[allow(clippy::too_many_arguments)]
    fn show(
        &mut self,
        src: &[u8],
        si: usize,
        op: &Op,
        args: &[Tok],
        gs: &GState,
        tm: &mut Matrix,
        tlm: Matrix,
        depth: usize,
    ) -> Option<usize> {
        let font = gs.font.clone().unwrap_or_else(|| self.fallback.clone());
        let mut buf = std::mem::take(&mut self.buf);
        let mut glyphs = Vec::new();
        let mut text = String::new();
        let mut unmapped = 0u32;
        let mut x = 0.0f64;
        let scale_x = gs.size * gs.th;
        let (mut lead, mut trail) = (None, None);
        // Numbers in a row so far, the pen after the first of them, and whether they
        // put a synthetic space into the text.
        let (mut row, mut mark, mut spaced) = (0u32, 0.0f64, false);
        for tok in args {
            match *tok {
                Tok::Str(sp, hex) => {
                    if row >= 2 && glyphs.is_empty() {
                        lead = Some(mark);
                    }
                    (row, spaced) = (0, false);
                    decode_str(src, sp, hex, &mut buf);
                    let mut i = 0;
                    while i < buf.len() && self.glyph_budget > 0 {
                        self.glyph_budget -= 1;
                        let (code, len) = font.next_code(&buf, i);
                        i += len.max(1);
                        let w0 = font.width(code);
                        let at = text.len() as u32;
                        if !font.push_unicode(code, &mut text) {
                            unmapped += 1;
                        }
                        glyphs.push(Glyph {
                            x,
                            w: (w0 * scale_x) as f32,
                            code,
                            at,
                            len: len.min(4) as u8,
                            gap: false,
                        });
                        let spacing = if font.is_word_space(code, len) {
                            gs.tc + gs.tw
                        } else {
                            gs.tc
                        };
                        x += (w0 * gs.size + spacing) * gs.th;
                    }
                }
                Tok::Num(n) if op.k == K::TJ => {
                    x -= n / 1000.0 * scale_x;
                    row += 1;
                    if row == 1 {
                        mark = x;
                    }
                    // Producers such as TeX have no space glyph and kern the words apart.
                    if n < -SPACE_GAP && !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                        spaced = true;
                        if let Some(g) = glyphs.last_mut() {
                            g.gap = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if row >= 2 && !glyphs.is_empty() {
            trail = Some(mark);
            // The numbers mark an extent; they are not a word gap.
            if spaced {
                text.pop();
                if let Some(g) = glyphs.last_mut() {
                    g.gap = false;
                }
            }
        }
        self.buf = buf;
        if self.glyph_budget == 0 {
            self.out.truncated = true;
        }
        let start = *tm;
        if x.is_finite() {
            *tm = Matrix::translate(x, 0.0).then(tm);
        }
        if glyphs.is_empty() {
            return None;
        }
        let run = RawText {
            show: op.k,
            tm: start,
            tlm_after: tlm,
            advance: if x.is_finite() { x } else { 0.0 },
            font,
            font_name: gs.font_name.clone(),
            size: gs.size,
            tc: gs.tc,
            tw: gs.tw,
            th: gs.th,
            rise: gs.rise,
            render: gs.render,
            fill: gs.fill,
            glyphs,
            text,
            unmapped,
            followed: false,
            placed: false,
            reserve: (lead, trail),
        };
        Some(self.push(si, op, gs, depth, RawKind::Text(run)))
    }

    /// Handles `Do`: records an image, or descends into a form XObject.
    fn paint_xobject(
        &mut self,
        si: usize,
        op: &Op,
        name: Vec<u8>,
        res: Option<&'a Dictionary>,
        gs: &GState,
        depth: usize,
    ) {
        let pdf = self.pdf;
        let Some(entry) = res
            .and_then(|r| get_dict(pdf, r, b"XObject"))
            .and_then(|x| x.get(&name).ok())
        else {
            return;
        };
        let id = entry.as_reference().ok();
        let Object::Stream(stream) = resolve(pdf, entry) else {
            return;
        };
        match get_name(pdf, &stream.dict, b"Subtype") {
            Some(b"Image") => {
                let dim =
                    |key: &[u8]| get_num(pdf, &stream.dict, key).unwrap_or(0.0).max(0.0) as u32;
                let image = RawImage {
                    xobject: id,
                    width: dim(b"Width"),
                    height: dim(b"Height"),
                };
                self.push(si, op, gs, depth, RawKind::Image(image));
            }
            Some(b"Form") => {
                let Some(id) = id else { return };
                if depth >= self.limits.max_depth || self.form_stack.contains(&id) {
                    return;
                }
                let key = StreamKey::Form(id);
                let index = match self.out.streams.iter().position(|s| s.key == key) {
                    Some(index) => index,
                    None => {
                        let Some(bytes) = stream_data(stream, self.limits.max_stream_bytes) else {
                            return;
                        };
                        let lexed = lex(&bytes, self.limits.max_ops);
                        self.out.truncated |= lexed.truncated;
                        self.out
                            .streams
                            .push(Arc::new(StreamData { key, bytes, lexed }));
                        self.out.streams.len() - 1
                    }
                };
                let matrix = get_matrix(pdf, &stream.dict, b"Matrix").unwrap_or(Matrix::IDENTITY);
                let ctm = matrix.then(&gs.ctm);
                // A form's content is clipped to its BBox, and nothing inside can leave that clip.
                let bbox = get_rect(pdf, &stream.dict, b"BBox")
                    .map(|b| b.transform(&ctm))
                    .filter(Rect::is_finite);
                let outer = gs.clip.as_ref().map(|c| c.rect);
                let rect = match (bbox, outer) {
                    (Some(b), Some(o)) => Some(o.intersect(&b)),
                    (b, o) => b.or(o),
                };
                let clip = rect.map(|rect| {
                    Arc::new(ClipNode {
                        group: None,
                        rect,
                        parent: None,
                    })
                });
                let inner = GState {
                    ctm,
                    clip,
                    ..gs.clone()
                };
                let form_res = get_dict(pdf, &stream.dict, b"Resources").or(res);
                self.form_stack.push(id);
                self.run(index, form_res, inner, depth + 1);
                self.form_stack.pop();
            }
            _ => {}
        }
    }
}
