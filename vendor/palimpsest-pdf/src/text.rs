//! Text replacement: typesetting new text into an existing line or block.
//!
//! Old and new text are compared; unchanged glyphs keep their original codes and,
//! where the flow allows it, their exact positions. Every glyph is painted from the
//! place of the run it belongs to, so it inherits that run's font, colour and clip.

use std::collections::HashMap;
use std::sync::Arc;

use lopdf::Document as Pdf;

use crate::content::{put_hex, put_name, put_num, put_nums, K};
use crate::doc::{Document, PageState};
use crate::edit::{Fate, Splice};
use crate::embed::Fallback;
use crate::interp::{RawKind, RawText, StreamKey};
use crate::layout::{join, Align, Join, Layout, LineGeom, RunGeom, GAP};
use crate::model::ObjId;
use crate::{Error, Result};

/// What to do when replaced text needs more lines than the block had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    /// Add lines below the block, with the block's line spacing.
    #[default]
    Grow,
    /// Condense the text horizontally, down to [`TextOptions::min_scale`], so that it
    /// fits the original lines. Grows when even that is not enough.
    Shrink,
    /// Leave the document unchanged and return [`Error::Overflow`].
    Report,
}

/// Options for [`Document::replace_line`] and [`Document::replace_block`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextOptions {
    /// Overrides the detected alignment.
    pub align: Option<Align>,
    pub overflow: Overflow,
    /// The narrowest horizontal scale [`Overflow::Shrink`] may use, in `0..=1`.
    pub min_scale: f64,
    /// Write text that needs shaping or right-to-left layout even when it cannot be
    /// shaped: one glyph per character, in logical order, which drops conjuncts and
    /// reorders nothing. Off by default: such an edit returns [`Error::Unsupported`]
    /// and leaves the document unchanged.
    pub allow_unshaped: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        TextOptions {
            align: None,
            overflow: Overflow::Grow,
            min_scale: 0.8,
            allow_unshaped: false,
        }
    }
}

/// The outcome of a text replacement.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextEdit {
    /// Characters that no available font can show. They were left out.
    pub missing: Vec<char>,
    /// Characters the original font lacks that were set in a fallback font.
    pub substituted: Vec<char>,
    /// Names of the fallback fonts used.
    pub fallback_fonts: Vec<String>,
    pub lines_before: usize,
    pub lines_after: usize,
    /// Horizontal scale applied by [`Overflow::Shrink`]; 1 when the text was not condensed.
    pub scale: f64,
    /// The block grew into the space of other text.
    pub overlaps: bool,
    /// Part of the new text falls outside a clipping path around the original text
    /// (a table cell, for example) and is hidden.
    pub clipped: bool,
    /// The new text contains characters from a script that needs shaping or
    /// right-to-left layout, which was not applied.
    pub needs_shaping: bool,
}

impl TextEdit {
    /// Serialises the outcome as JSON.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"missing\":");
        crate::model::json_str(&mut out, &self.missing.iter().collect::<String>());
        out.push_str(",\"substituted\":");
        crate::model::json_str(&mut out, &self.substituted.iter().collect::<String>());
        out.push_str(",\"fallback_fonts\":[");
        for (i, name) in self.fallback_fonts.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            crate::model::json_str(&mut out, name);
        }
        out.push_str(&format!(
            "],\"lines_before\":{},\"lines_after\":{},\"scale\":",
            self.lines_before, self.lines_after
        ));
        crate::model::json_num(&mut out, self.scale);
        out.push_str(&format!(
            ",\"overlaps\":{},\"clipped\":{},\"needs_shaping\":{}}}",
            self.overlaps, self.clipped, self.needs_shaping
        ));
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Glyph,
    Space,
    Break,
}

/// Marks a [`Unit`] that is not an item of an original line.
const NO_ITEM: u32 = u32::MAX;

/// One glyph, space or hard break of the text being flowed.
#[derive(Clone, Copy)]
struct Unit {
    kind: Kind,
    /// Index of the original line within the edited scope.
    ol: u32,
    /// Item index on that line, or [`NO_ITEM`] for new text and line joins.
    item: u32,
    /// Original extent along the baseline; meaningful for kept units.
    t0: f64,
    t1: f64,
    /// Original distance from its start to the start of the item after it on the line.
    /// `None` for the last item of a line, a line join and typed-in text.
    pitch: Option<f64>,
    /// The run that paints it: layout line and run slot.
    run: (u32, u32),
    /// Advance in page points at scale 1, without kerning.
    natural: f64,
    glyph: Option<Glyph>,
    /// A hyphen: the line may break after it.
    break_after: bool,
    /// False for text that was typed in.
    kept: bool,
    /// Starts a run that a positioning operator placed at least one font size after
    /// the text before it: a table cell, a tab stop. On a line set from its left edge
    /// it stays at `t0` until the text before it comes within a space of it.
    hold: bool,
}

#[derive(Clone, Copy)]
struct Glyph {
    /// Character code, or the glyph id in the fallback font's own numbering.
    code: u32,
    len: u8,
    /// Width for a font size of one.
    w0: f64,
    word_space: bool,
    fallback: Option<u32>,
    /// Shaping offset in em.
    off: (f32, f32),
}

/// A glyph placed relative to the run that paints it.
struct Put {
    /// Pen position in the run's text space.
    x: f64,
    /// Rise added to the run's own, in text-space units.
    dy: f64,
    glyph: Glyph,
}

/// A run to rewrite and the glyphs it paints afterwards.
struct RunPlan {
    key: StreamKey,
    start: u32,
    end: u32,
    show: K,
    size: f64,
    th: f64,
    tc: f64,
    tw: f64,
    rise: f64,
    advance: f64,
    followed: bool,
    /// Where the run began and ended for line grouping before the edit, in its text
    /// space. `None` when the text is re-wrapped.
    reserve: Option<(f64, f64)>,
    font_name: Arc<[u8]>,
    puts: Vec<Put>,
}

struct Plan {
    runs: Vec<RunPlan>,
    scale: f64,
    /// Fallback glyphs to embed: `(fallback index, glyph id, text it stands for)`.
    embed: Vec<(u32, u16, String)>,
    edit: TextEdit,
}

/// True for characters whose script needs shaping or right-to-left layout.
fn needs_shaping(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0FFF | 0x1000..=0x109F | 0x1780..=0x17FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Looks up the run behind a layout line and run slot.
fn run_at<'a>(
    state: &'a PageState,
    layout: &'a Layout,
    at: (u32, u32),
) -> Option<(&'a RunGeom, &'a RawText)> {
    let geom = layout.lines.get(at.0 as usize)?.runs.get(at.1 as usize)?;
    match &state.interp.items.get(geom.index as usize)?.kind {
        RawKind::Text(run) => Some((geom, run)),
        RawKind::Image(_) => None,
    }
}

/// The advance of a space in a run's style, in page points.
fn space_width(geom: &RunGeom, run: &RawText) -> f64 {
    let glyph = run
        .font
        .encode(' ')
        .map(|code| run.font.width(code))
        .filter(|w| *w > 0.0);
    let em = glyph.map_or(0.3 * run.size.abs(), |w| (w * run.size + run.tc).abs());
    em * run.th.abs() * geom.s
}

/// The glyph for `c` in a run's own font, when the font can really paint it.
fn own_glyph(pdf: &Pdf, run: &RawText, c: char) -> Option<Glyph> {
    let code = run
        .font
        .encode(c)
        .filter(|code| run.font.can_show(pdf, *code, c))?;
    let len = run.font.code_len(code);
    Some(Glyph {
        code,
        len: len as u8,
        w0: run.font.width(code),
        word_space: run.font.is_word_space(code, len),
        fallback: None,
        off: (0.0, 0.0),
    })
}

/// Advance of a glyph along the baseline in page points, at horizontal scale 1.
fn advance(geom: &RunGeom, run: &RawText, g: &Glyph) -> f64 {
    let spacing = run.tc + if g.word_space { run.tw } else { 0.0 };
    ((g.w0 * run.size + spacing) * run.th * geom.s).abs()
}

/// Advance from unit `i` to the next unit on the same output line.
fn step(units: &[Unit], i: usize, justified: bool) -> f64 {
    let u = &units[i];
    match u.pitch.filter(|_| u.kept) {
        // Kept text keeps the distance to what stood after it, whatever stands there
        // now: that distance carries kerning, and before a run that was positioned on
        // its own (a table column, a `Td` after a change of word spacing) it is not
        // the glyph's advance at all. Spaces of a justified line are re-spread, unless
        // the gap is wide enough to be a tab.
        Some(pitch) if u.kind != Kind::Space || !justified || pitch > 2.5 * u.natural => pitch,
        _ => u.natural,
    }
}

/// Width of `units[a..b]` set on one line, without trailing spaces.
fn measure(units: &[Unit], a: usize, b: usize, justified: bool) -> f64 {
    let Some(last) = (a..b).rev().find(|i| units[*i].kind == Kind::Glyph) else {
        return 0.0;
    };
    (a..last).map(|i| step(units, i, justified)).sum::<f64>() + units[last].natural
}

/// A stretch of a text comparison: old units `a..b` against new text `c..d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seg {
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    /// The stretch reads the same in both.
    same: bool,
}

/// The largest table [`diff`] fills; a larger comparison is one change from end to end.
const DIFF_CELLS: usize = 8_000_000;

/// Compares the texts of old units with new text. Returns stretches that alternate
/// between equal and changed and together cover both; `c..d` are byte offsets in `new`.
fn diff(old: &[&str], new: &str) -> Vec<Seg> {
    let mut at: Vec<usize> = new.char_indices().map(|(i, _)| i).collect();
    at.push(new.len());
    let (m, n) = (old.len(), at.len() - 1);
    if m == 0 && n == 0 {
        return Vec::new();
    }
    let whole = Seg {
        a: 0,
        b: m,
        c: 0,
        d: new.len(),
        same: false,
    };
    if m == 0 || n == 0 || m > 60_000 || (m + 1).saturating_mul(n + 1) > DIFF_CELLS {
        return vec![whole];
    }
    let lens: Vec<usize> = old.iter().map(|s| s.chars().count()).collect();
    let fits = |i: usize, j: usize| j + lens[i] <= n && new[at[j]..at[j + lens[i]]] == *old[i];
    // `lcs[i * w + j]`: the most units of `old[i..]` found in order in `new` from char `j`.
    let w = n + 1;
    let mut lcs = vec![0u16; (m + 1) * w];
    for i in (0..m).rev() {
        for j in (0..=n).rev() {
            let mut best = lcs[(i + 1) * w + j];
            if j < n {
                best = best.max(lcs[i * w + j + 1]);
            }
            if fits(i, j) {
                best = best.max(1 + lcs[(i + 1) * w + j + lens[i]]);
            }
            lcs[i * w + j] = best;
        }
    }
    // Segments in units and chars: `(a, b, c, d, same)`.
    let mut segs: Vec<Seg> = Vec::new();
    let mut push = |a: usize, c: usize, da: usize, dc: usize, same: bool| match segs.last_mut() {
        Some(last) if last.same == same => {
            last.b += da;
            last.d += dc;
        }
        _ => segs.push(Seg {
            a,
            b: a + da,
            c,
            d: c + dc,
            same,
        }),
    };
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        let here = lcs[i * w + j];
        if i < m && fits(i, j) && here == 1 + lcs[(i + 1) * w + j + lens[i]] {
            push(i, j, 1, lens[i], true);
            j += lens[i];
            i += 1;
        } else if i < m && (j == n || here == lcs[(i + 1) * w + j]) {
            push(i, j, 1, 0, false);
            i += 1;
        } else {
            push(i, j, 0, 1, false);
            j += 1;
        }
    }
    // A match that is small beside the changes on both sides of it is a coincidence: a
    // letter of a rewritten phrase that happens to recur. Keeping it would leave one
    // glyph, and whatever is typed after it, in the style of a run whose text is gone.
    // Whole words and the spaces between them are held to a weaker test, because a
    // space is often all that separates two corrections in neighbouring runs.
    let chars: Vec<char> = new.chars().collect();
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let whole = |s: &Seg| {
        let inner = (
            word(chars.get(s.c).copied()),
            word(chars[..s.d].last().copied()),
        );
        let before = word(chars[..s.c].last().copied())
            || word(old[..s.a].last().and_then(|t| t.chars().last()));
        let after =
            word(chars.get(s.d).copied()) || word(old.get(s.b).and_then(|t| t.chars().next()));
        s.c == s.d || !(inner.0 && before || inner.1 && after)
    };
    let size = |s: &Seg| (s.b - s.a).max(s.d - s.c);
    loop {
        let weak = (1..segs.len().saturating_sub(1))
            .filter(|&k| segs[k].same)
            .filter(|&k| {
                let around = size(&segs[k - 1]).min(size(&segs[k + 1]));
                if whole(&segs[k]) {
                    2 * size(&segs[k]) < around
                } else {
                    size(&segs[k]) <= around
                }
            })
            .min_by_key(|&k| size(&segs[k]));
        let Some(k) = weak else {
            break;
        };
        segs[k - 1].b = segs[k + 1].b;
        segs[k - 1].d = segs[k + 1].d;
        segs.drain(k..=k + 1);
    }
    for s in &mut segs {
        (s.c, s.d) = (at[s.c], at[s.d]);
    }
    segs
}

/// The box text is flowed into, in the frame of the block's baseline direction.
struct Frame {
    align: Align,
    /// Left edge, right edge and baseline of each original line. For centred and
    /// right-aligned text the right edge is where the visible text ended as
    /// [`measure`] sees it, so that old and new text are anchored the same way.
    t0: Vec<f64>,
    t1: Vec<f64>,
    n: Vec<f64>,
    right: f64,
    leading: f64,
    /// False for a single-line replacement, which never wraps.
    wrap: bool,
}

impl Frame {
    fn at(&self, values: &[f64], k: usize) -> f64 {
        values.get(k).or(values.last()).copied().unwrap_or(0.0)
    }

    /// Baseline of output line `k`; lines past the original ones continue downwards.
    fn baseline(&self, k: usize) -> f64 {
        self.at(&self.n, k) - k.saturating_sub(self.n.len().saturating_sub(1)) as f64 * self.leading
    }

    /// Where output line `k` of width `width` starts.
    fn origin(&self, k: usize, width: f64) -> f64 {
        match self.align {
            Align::Left | Align::Justified => self.at(&self.t0, k),
            Align::Right => self.at(&self.t1, k) - width,
            Align::Center => (self.at(&self.t0, k) + self.at(&self.t1, k)) / 2.0 - width / 2.0,
        }
    }

    /// The width available to output line `k`.
    fn room(&self, k: usize) -> f64 {
        let left = match self.align {
            Align::Left | Align::Justified => self.at(&self.t0, k),
            Align::Right | Align::Center => self.t0.iter().copied().fold(f64::MAX, f64::min),
        };
        (self.right - left).max(0.0)
    }
}

/// The text of a scope as units, with the typed-in part already merged.
struct Flow<'a> {
    pdf: &'a Pdf,
    state: &'a PageState,
    layout: &'a Layout,
    fallbacks: &'a [Fallback],
    /// The layout lines being replaced.
    scope: Vec<usize>,
    units: Vec<Unit>,
    /// The first and the last change lie within `units[fresh.0..fresh.1]`.
    fresh: (usize, usize),
    /// The original line the change starts on.
    change_line: usize,
    /// Width of each original line as [`measure`] sees it.
    orig_width: Vec<f64>,
    embed: Vec<(u32, u16, String)>,
    edit: TextEdit,
}

impl<'a> Flow<'a> {
    /// Reads the lines of a scope and merges `new_text` into them.
    fn new(
        pdf: &'a Pdf,
        state: &'a PageState,
        layout: &'a Layout,
        fallbacks: &'a [Fallback],
        lines: &[usize],
        align: Align,
        new_text: &str,
    ) -> Result<Flow<'a>> {
        let mut old: Vec<Unit> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut text = String::new();
        for (ol, &li) in lines.iter().enumerate() {
            let line = &layout.lines[li];
            // How far the runs read so far reach as lines are formed, and the size of
            // the last.
            let (mut reach, mut before) = (f64::MIN, 0.0f64);
            for (ii, item) in line.items.iter().enumerate() {
                let Some((geom, run)) = run_at(state, layout, (li as u32, item.run)) else {
                    continue;
                };
                let hold = item.glyph == 0
                    && run.placed
                    && reach > f64::MIN
                    && geom.g0 - reach >= geom.size.max(before);
                if item.glyph != GAP {
                    (reach, before) = (reach.max(geom.g1), geom.size);
                }
                let s = line.item_text(item);
                let glyph = (item.glyph != GAP)
                    .then(|| run.glyphs.get(item.glyph as usize))
                    .flatten()
                    .map(|g| Glyph {
                        code: g.code,
                        len: g.len,
                        w0: run.font.width(g.code),
                        word_space: run.font.is_word_space(g.code, usize::from(g.len)),
                        fallback: None,
                        off: (0.0, 0.0),
                    });
                let blank = s.chars().all(char::is_whitespace);
                old.push(Unit {
                    kind: if blank { Kind::Space } else { Kind::Glyph },
                    ol: ol as u32,
                    item: ii as u32,
                    t0: item.t0,
                    t1: item.t1,
                    pitch: line.items.get(ii + 1).map(|next| next.t0 - item.t0),
                    run: (li as u32, item.run),
                    natural: glyph
                        .as_ref()
                        .map_or_else(|| space_width(geom, run), |g| advance(geom, run, g)),
                    glyph,
                    break_after: s.ends_with(['-', '\u{2010}']),
                    kept: true,
                    hold,
                });
                spans.push((text.len(), text.len() + s.len()));
                text.push_str(s);
            }
            let (Some(&next), Some(last)) = (lines.get(ol + 1), old.last().copied()) else {
                continue;
            };
            let (kind, s) = match join(line, &layout.lines[next], align) {
                Join::Space => (Kind::Space, " "),
                Join::Break => (Kind::Break, "\n"),
                Join::Nothing => continue,
            };
            let space = run_at(state, layout, last.run).and_then(|(g, r)| {
                own_glyph(pdf, r, ' ').map(|glyph| (advance(g, r, &glyph), glyph))
            });
            let natural = space.map_or_else(
                || run_at(state, layout, last.run).map_or(0.0, |(g, r)| space_width(g, r)),
                |s| s.0,
            );
            // The space is painted only if the join ends up inside a line.
            let glyph = space.map(|s| s.1).filter(|_| kind == Kind::Space);
            old.push(Unit {
                kind,
                item: NO_ITEM,
                t0: last.t1,
                pitch: None,
                natural,
                glyph,
                break_after: false,
                hold: false,
                ..last
            });
            spans.push((text.len(), text.len() + 1));
            text.push_str(s);
        }
        let justified = align == Align::Justified;
        let mut orig_width = vec![0.0; lines.len()];
        let mut a = 0;
        while a < old.len() {
            let b = (a..old.len())
                .find(|i| old[*i].ol != old[a].ol)
                .unwrap_or(old.len());
            orig_width[old[a].ol as usize] = measure(&old, a, b, justified);
            a = b;
        }

        let of = |i: usize| &text[spans[i].0..spans[i].1];
        // Spaces that end the text are invisible; text given without them keeps them.
        let mut tail = old.len();
        if !new_text.ends_with(char::is_whitespace) {
            while tail > 0 && old[tail - 1].kind == Kind::Space {
                tail -= 1;
            }
        }
        // Everything before the first and after the last difference is kept as it is.
        let (mut p, mut pos) = (0, 0);
        while p < tail && new_text[pos..].starts_with(of(p)) {
            pos += of(p).len();
            p += 1;
        }
        let (mut q, mut end) = (tail, new_text.len());
        while q > p && new_text[pos..end].ends_with(of(q - 1)) {
            end -= of(q - 1).len();
            q -= 1;
        }
        // Between them, only what really differs is typed in again, so that a run the
        // change does not reach keeps its own glyphs, font, size and colour.
        let middle: Vec<&str> = (p..q).map(of).collect();
        let segs = diff(&middle, &new_text[pos..end]);
        let change_line = old.get(p).or(old.last()).map_or(0, |u| u.ol as usize);
        let mut flow = Flow {
            pdf,
            state,
            layout,
            fallbacks,
            scope: lines.to_vec(),
            units: old[..p].to_vec(),
            fresh: (p, p),
            change_line,
            orig_width,
            embed: Vec::new(),
            edit: TextEdit {
                scale: 1.0,
                lines_before: lines.len(),
                ..TextEdit::default()
            },
        };
        let glyphy = |u: &&Unit| u.kind == Kind::Glyph;
        for seg in &segs {
            let (a, b) = (p + seg.a, p + seg.b);
            if seg.same {
                flow.units.extend_from_slice(&old[a..b]);
                continue;
            }
            // New text takes the style of the run it replaces text of, or follows.
            let style = old[a..b]
                .iter()
                .find(glyphy)
                .or_else(|| old[..a].iter().rev().find(glyphy))
                .or_else(|| old[b..].iter().find(glyphy));
            let style = *style.ok_or_else(|| {
                Error::Unsupported("the text has no glyphs to take a style from".into())
            })?;
            let first = flow.units.len();
            flow.typeset(
                &new_text[pos + seg.c..pos + seg.d],
                Unit {
                    ol: old.get(a).or(old.last()).map_or(0, |u| u.ol),
                    item: NO_ITEM,
                    kept: false,
                    pitch: None,
                    hold: false,
                    ..style
                },
            );
            // Text typed over the start of a held run starts where the run did.
            if let (Some(u), Some(o)) = (flow.units.get_mut(first), old[a..b].first()) {
                (u.hold, u.t0) = (o.hold, o.t0);
            }
            flow.fresh.1 = flow.units.len();
        }
        flow.units.extend_from_slice(&old[q..]);
        Ok(flow)
    }
}

/// A line of the result: `units[a..b]` on output line `k`.
struct OutLine {
    a: usize,
    b: usize,
    k: usize,
    /// The line ends its paragraph, so it is not justified.
    last: bool,
    /// An original line taken over whole; `same` when it also stays where it was.
    adopted: bool,
    same: bool,
}

impl Flow<'_> {
    /// Turns typed-in text into units styled like `proto`.
    fn typeset(&mut self, text: &str, proto: Unit) {
        let Some((geom, run)) = run_at(self.state, self.layout, proto.run) else {
            return;
        };
        let (pdf, fallbacks) = (self.pdf, self.fallbacks);
        let (state, layout) = (self.state, self.layout);
        let own = |c: char| own_glyph(pdf, run, c);
        // Another run of the same text may use a font that has the character.
        let scope: Vec<(u32, u32)> = self
            .scope
            .iter()
            .flat_map(|li| (0..layout.lines[*li].runs.len()).map(|s| (*li as u32, s as u32)))
            .collect();
        let elsewhere = |c: char| {
            let found = scope
                .iter()
                .filter_map(|at| run_at(state, layout, *at).map(|(g, r)| (*at, g, r)));
            let mut found: Vec<_> = found
                .filter(|(_, g, _)| (g.size - geom.size).abs() <= 0.05 * geom.size)
                .collect();
            found.sort_by_key(|(_, _, r)| {
                (
                    r.font.bold != run.font.bold,
                    r.font.italic != run.font.italic,
                )
            });
            found.into_iter().find_map(|(at, g, r)| {
                own_glyph(pdf, r, c).map(|glyph| (at, advance(g, r, &glyph), glyph))
            })
        };
        // With shaping available, complex scripts are set in a fallback font, whose
        // layout tables are intact; an embedded subset has usually lost them.
        let shaped = |c: char| cfg!(feature = "shaping") && needs_shaping(c);
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            i += 1;
            if c == '\n' {
                self.units.push(Unit {
                    kind: Kind::Break,
                    natural: 0.0,
                    glyph: None,
                    break_after: false,
                    ..proto
                });
                continue;
            }
            if c.is_whitespace() {
                let glyph = own(' ');
                let natural = glyph
                    .as_ref()
                    .map_or_else(|| space_width(geom, run), |g| advance(geom, run, g));
                self.units.push(Unit {
                    kind: Kind::Space,
                    natural,
                    glyph,
                    break_after: false,
                    ..proto
                });
                continue;
            }
            self.edit.needs_shaping |=
                needs_shaping(c) && (!cfg!(feature = "shaping") || crate::embed::is_rtl(c));
            let mine = own(c);
            let other = if mine.is_none() { elsewhere(c) } else { None };
            let wants = |c: char| {
                !c.is_whitespace() && (shaped(c) || (own(c).is_none() && elsewhere(c).is_none()))
            };
            let pick = if (mine.is_none() && other.is_none()) || shaped(c) {
                crate::embed::pick(fallbacks, &run.font, c)
            } else {
                None
            };
            let hyphen = matches!(c, '-' | '\u{2010}');
            match (pick, mine, other) {
                (Some(fi), _, _) => {
                    let face = &fallbacks[fi];
                    let end = (i..chars.len())
                        .find(|j| !(wants(chars[*j]) && face.has(chars[*j])))
                        .unwrap_or(chars.len());
                    let segment: String = chars[i - 1..end].iter().collect();
                    for c in segment
                        .chars()
                        .filter(|c| !self.edit.substituted.contains(c))
                        .collect::<Vec<_>>()
                    {
                        self.edit.substituted.push(c);
                    }
                    if !self.edit.fallback_fonts.contains(&face.name) {
                        self.edit.fallback_fonts.push(face.name.clone());
                    }
                    for g in face.shape(&segment) {
                        // The code is the index of this glyph's entry in `embed`; the real
                        // glyph id is assigned when the subset is written.
                        let code = self.embed.len() as u32;
                        let glyph = Glyph {
                            code,
                            len: 2,
                            w0: g.advance,
                            word_space: false,
                            fallback: Some(fi as u32),
                            off: g.offset,
                        };
                        let natural = advance(geom, run, &glyph);
                        self.units.push(Unit {
                            kind: Kind::Glyph,
                            natural,
                            glyph: Some(glyph),
                            break_after: g.text.ends_with('-'),
                            ..proto
                        });
                        self.embed.push((fi as u32, g.gid, g.text));
                    }
                    i = end;
                }
                (None, Some(glyph), _) => {
                    let natural = advance(geom, run, &glyph);
                    self.units.push(Unit {
                        kind: Kind::Glyph,
                        natural,
                        glyph: Some(glyph),
                        break_after: hyphen,
                        ..proto
                    });
                }
                (None, None, Some((at, natural, glyph))) => {
                    self.units.push(Unit {
                        kind: Kind::Glyph,
                        run: at,
                        natural,
                        glyph: Some(glyph),
                        break_after: hyphen,
                        ..proto
                    });
                }
                (None, None, None) => {
                    if !self.edit.missing.contains(&c) {
                        self.edit.missing.push(c);
                    }
                }
            }
        }
    }

    /// True when unit `i` is kept text that began an original line.
    fn starts_line(&self, i: usize) -> bool {
        let u = &self.units[i];
        u.kept && u.item == 0 && i >= self.fresh.1
    }

    /// Breaks the units into lines. Lines before `from` stay as they are; with
    /// `resync`, the original breaks are taken up again once the text lines up with them.
    fn wrap(&self, f: &Frame, scale: f64, from: usize, resync: bool) -> Vec<OutLine> {
        let (units, n) = (&self.units, self.units.len());
        let justified = f.align == Align::Justified;
        let mut out = Vec::new();
        let mut i = 0;
        for ol in 0..from {
            let a = i;
            while i < n && units[i].ol as usize == ol {
                i += 1;
            }
            out.push(OutLine {
                a,
                b: i,
                k: ol,
                last: false,
                adopted: true,
                same: true,
            });
        }
        let mut k = from;
        while i < n {
            if resync && self.starts_line(i) {
                while i < n {
                    let (a, ol) = (i, units[i].ol);
                    while i < n && units[i].ol == ol {
                        i += 1;
                    }
                    let last = i >= n || units[i - 1].kind == Kind::Break;
                    out.push(OutLine {
                        a,
                        b: i,
                        k,
                        last,
                        adopted: true,
                        same: k == ol as usize,
                    });
                    k += 1;
                }
                break;
            }
            let a = i;
            let avail = if f.wrap {
                f.room(k) + 0.05
            } else {
                f64::INFINITY
            };
            let (mut width, mut slack, mut brk, mut hard) = (0.0, 0.0, None, false);
            while i < n {
                let u = &units[i];
                if u.kind == Kind::Break {
                    hard = true;
                    i += 1;
                    break;
                }
                if u.kind == Kind::Glyph && i > a {
                    // The original break stands unless the line became shorter than it was.
                    let keep = resync
                        && self.starts_line(i)
                        && measure(units, a, i, justified) * scale
                            >= self.at_least(units[i - 1].ol);
                    if keep || width + u.natural * scale > avail + slack {
                        i = if keep { i } else { brk.unwrap_or(i) };
                        break;
                    }
                }
                width += step(units, i, justified) * scale;
                if u.kind == Kind::Space {
                    brk = Some(i + 1);
                    // Spaces of a justified line may close up by a third, as TeX sets them.
                    slack += if justified {
                        u.natural * scale / 3.0
                    } else {
                        0.0
                    };
                } else if u.break_after {
                    brk = Some(i + 1);
                }
                i += 1;
            }
            out.push(OutLine {
                a,
                b: i,
                k,
                last: hard || i >= n,
                adopted: false,
                same: false,
            });
            k += 1;
        }
        out
    }

    /// The width below which an original line counts as having become shorter.
    fn at_least(&self, ol: u32) -> f64 {
        let width = self.orig_width.get(ol as usize).copied().unwrap_or(0.0);
        width - 0.25 * self.units.first().map_or(0.0, |u| u.natural.max(1.0))
    }
}

impl Flow<'_> {
    /// Places every glyph and collects, for each run that changes, what it paints afterwards.
    fn place(
        &mut self,
        f: &Frame,
        lines: &[OutLine],
        scale: f64,
        scope: &[usize],
    ) -> Result<Vec<RunPlan>> {
        let (state, layout) = (self.state, self.layout);
        let justified = f.align == Align::Justified;
        let to_user = state.to_page.invert();
        let mut puts: HashMap<(u32, u32), Vec<Put>> = HashMap::new();
        let mut same = vec![false; scope.len()];
        // `(baseline, start, end)` of lines that reach into space the block did not occupy.
        let mut spill: Vec<(f64, f64, f64)> = Vec::new();
        for line in lines {
            let units = &self.units[line.a..line.b];
            if line.same {
                units
                    .first()
                    .into_iter()
                    .for_each(|u| same[u.ol as usize] = true);
                continue;
            }
            let baseline = f.baseline(line.k);
            let inner = units
                .iter()
                .rposition(|u| u.kind == Kind::Glyph)
                .unwrap_or(0);
            let mut t: Vec<f64> = units.iter().map(|u| u.t0).collect();
            if !line.adopted {
                let width = measure(&self.units, line.a, line.b, justified) * scale;
                let origin = f.origin(line.k, width);
                let spaces = units[..inner]
                    .iter()
                    .filter(|u| u.kind == Kind::Space)
                    .count();
                let widest = units
                    .iter()
                    .filter(|u| u.kind == Kind::Space)
                    .map(|u| u.natural)
                    .fold(0.0, f64::max);
                let mut gap = if justified && f.wrap && !line.last && spaces > 0 {
                    (f.right - origin - width) / spaces as f64
                } else {
                    0.0
                };
                if gap > 3.0 * widest * scale {
                    gap = 0.0;
                }
                let mut x = origin;
                // Runs across a column gap keep their place on a line that is set from
                // its left edge and not re-wrapped.
                let holds = !f.wrap && matches!(f.align, Align::Left | Align::Justified);
                let mut ink = f64::MIN;
                for (j, i) in (line.a..line.b).enumerate() {
                    if units[j].hold && holds {
                        let space = run_at(state, layout, units[j].run)
                            .map_or(0.0, |(g, r)| space_width(g, r));
                        x = if ink > f64::MIN {
                            units[j].t0.max(ink + space * scale)
                        } else {
                            units[j].t0
                        };
                    }
                    t[j] = x;
                    if units[j].kind == Kind::Glyph {
                        ink = x + units[j].natural * scale;
                    }
                    x += step(&self.units, i, justified) * scale
                        + if units[j].kind == Kind::Space && j < inner {
                            gap
                        } else {
                            0.0
                        };
                }
                if origin + width > f.right + 1.0 && line.k < scope.len() {
                    spill.push((baseline, origin.max(f.right), origin + width));
                }
            }
            if line.k >= scope.len() {
                let glyphs = || units.iter().zip(&t).filter(|(u, _)| u.kind == Kind::Glyph);
                let start = glyphs().map(|(_, t)| *t).fold(f64::MAX, f64::min);
                let end = glyphs()
                    .map(|(u, t)| t + u.natural * scale)
                    .fold(f64::MIN, f64::max);
                spill.push((baseline, start, end));
            }
            for (j, (u, t)) in units.iter().zip(&t).enumerate() {
                // A line join is a space only when it lands inside a line.
                if u.kept && u.item == NO_ITEM && j > inner {
                    continue;
                }
                let (Some(mut glyph), Some((geom, run))) = (u.glyph, run_at(state, layout, u.run))
                else {
                    continue;
                };
                if geom.vn.abs() < 1e-9 || geom.s.abs() < 1e-12 {
                    return Err(Error::Unsupported("the text matrix is degenerate".into()));
                }
                let from = &layout.lines[u.run.0 as usize];
                let shift = baseline - from.n;
                let dy = if shift.abs() < 1e-7 {
                    0.0
                } else {
                    shift / geom.vn
                };
                let x = (t - geom.t - dy * geom.vu) / geom.s;
                let moved = !u.kept || dy != 0.0 || (t - u.t0).abs() > 0.01;
                let clip = state
                    .interp
                    .items
                    .get(geom.index as usize)
                    .and_then(|i| i.clip.as_ref());
                if let (true, Some(clip), Some(to_user)) = (moved, clip, to_user) {
                    let n = geom.n + shift;
                    for end in [*t, t + u.natural * scale] {
                        let (px, py) =
                            (from.u.0 * end - from.u.1 * n, from.u.1 * end + from.u.0 * n);
                        let (ux, uy) = to_user.apply(px, py);
                        let r = &clip.rect;
                        self.edit.clipped |= ux < r.x0 - 0.5
                            || ux > r.x1 + 0.5
                            || uy < r.y0 - 0.5
                            || uy > r.y1 + 0.5;
                    }
                }
                let off = (
                    f64::from(glyph.off.0) * run.size * run.th * scale,
                    f64::from(glyph.off.1) * run.size,
                );
                glyph.off = (0.0, 0.0);
                puts.entry(u.run).or_default().push(Put {
                    x: x + off.0,
                    dy: dy + off.1,
                    glyph,
                });
            }
        }
        let anchor = &layout.lines[scope[0]];
        for (i, other) in layout.lines.iter().enumerate() {
            let peer = !scope.contains(&i)
                && (other.stream, other.bucket) == (anchor.stream, anchor.bucket);
            let hit = |(n, a, b): &(f64, f64, f64)| {
                (other.n - n).abs() < 0.9 * anchor.size && other.t1 > *a && other.t0 < *b
            };
            self.edit.overlaps |= peer && spill.iter().any(hit);
        }
        let mut plans = Vec::new();
        for (_, &li) in scope.iter().enumerate().filter(|(ol, _)| !same[*ol]) {
            for (slot, geom) in layout.lines[li].runs.iter().enumerate() {
                let Some(item) = state.interp.items.get(geom.index as usize) else {
                    continue;
                };
                let (RawKind::Text(run), Some(stream)) =
                    (&item.kind, state.interp.streams.get(item.stream as usize))
                else {
                    continue;
                };
                let list = puts.remove(&(li as u32, slot as u32)).unwrap_or_default();
                let untouched = scale == 1.0
                    && list.len() == run.glyphs.len()
                    && list.iter().zip(&run.glyphs).all(|(p, g)| {
                        p.dy == 0.0
                            && p.glyph.fallback.is_none()
                            && p.glyph.code == g.code
                            && (p.x - g.x).abs() <= 1e-6 * (1.0 + g.x.abs())
                    });
                if untouched {
                    continue;
                }
                plans.push(RunPlan {
                    key: stream.key,
                    start: item.start,
                    end: item.end,
                    show: run.show,
                    size: run.size,
                    th: run.th,
                    tc: run.tc,
                    tw: run.tw,
                    rise: run.rise,
                    advance: run.advance,
                    followed: run.followed,
                    reserve: (!f.wrap && geom.s.abs() > 1e-12)
                        .then(|| ((geom.g0 - geom.t) / geom.s, (geom.g1 - geom.t) / geom.s)),
                    font_name: run.font_name.clone(),
                    puts: list,
                });
            }
        }
        Ok(plans)
    }
}

/// The right-hand limit for a line that stands alone: the next text on its baseline,
/// or the right edge of the page's text.
fn limit(layout: &Layout, li: usize) -> f64 {
    let line = &layout.lines[li];
    let (mut edge, mut wall) = (line.t1, f64::INFINITY);
    for (i, other) in layout.lines.iter().enumerate() {
        if i == li || (other.stream, other.bucket) != (line.stream, line.bucket) {
            continue;
        }
        edge = edge.max(other.t1);
        if (other.n - line.n).abs() < 0.8 * line.size.max(other.size) && other.t0 >= line.t1 - 0.5 {
            wall = wall.min(other.t0 - line.size);
        }
    }
    wall.min(edge).max(line.t1)
}

/// Works out how to replace the text of the line or block that object `index` belongs to.
fn plan(
    pdf: &Pdf,
    state: &PageState,
    fallbacks: &[Fallback],
    index: usize,
    text: &str,
    options: &TextOptions,
    block: bool,
) -> Result<Plan> {
    let layout = state.layout();
    let not_text = || Error::Unsupported("the object is not editable text".into());
    let li = layout.line_of_run(index).ok_or_else(not_text)?;
    let owner = layout
        .blocks
        .iter()
        .find(|b| b.lines.contains(&li))
        .ok_or_else(not_text)?;
    let scope: Vec<usize> = if block { owner.lines.clone() } else { vec![li] };
    let text = if block {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.replace(['\r', '\n'], " ")
    };
    let align = options.align.unwrap_or(if block || owner.lines.len() > 1 {
        owner.align
    } else {
        Align::Left
    });
    let align = if !block && align == Align::Justified {
        Align::Left
    } else {
        align
    };
    let mut flow = Flow::new(pdf, state, layout, fallbacks, &scope, align, &text)?;
    if flow.edit.needs_shaping && !options.allow_unshaped {
        return Err(Error::Unsupported(
            "the new text needs shaping or right-to-left layout, which is not available; \
             TextOptions::allow_unshaped (--unshaped) writes it unshaped"
                .into(),
        ));
    }
    let of = |get: fn(&LineGeom) -> f64| {
        scope
            .iter()
            .map(|i| get(&layout.lines[*i]))
            .collect::<Vec<f64>>()
    };
    let (t0, mut t1, n) = (of(|l| l.t0), of(|l| l.t1), of(|l| l.n));
    if matches!(align, Align::Right | Align::Center) {
        // A line's own extent includes the spaces that end it; the new text is
        // measured without them.
        for (end, (start, width)) in t1.iter_mut().zip(t0.iter().zip(&flow.orig_width)) {
            *end = start + width;
        }
    }
    let size = layout.lines[scope[0]].size;
    let spacing = if n.len() > 1 {
        (n[0] - n[n.len() - 1]) / (n.len() - 1) as f64
    } else {
        0.0
    };
    let frame = Frame {
        align,
        right: if owner.lines.len() > 1 {
            owner.t1
        } else {
            limit(layout, li)
        },
        leading: if spacing > 0.2 * size {
            spacing
        } else {
            1.2 * size
        },
        wrap: block,
        t0,
        t1,
        n,
    };
    let justified = align == Align::Justified;
    let min_scale = options.min_scale.clamp(0.1, 1.0);
    let mut scale = 1.0;
    let mut lines = flow.wrap(&frame, 1.0, flow.change_line, true);
    let width = measure(&flow.units, 0, flow.units.len(), justified);
    let over = if block {
        lines.len() > scope.len()
    } else {
        width > frame.room(0) + 0.5
    };
    match (over, options.overflow) {
        (true, Overflow::Report) => {
            let (needed, available) = if block {
                (lines.len() as f64, scope.len() as f64)
            } else {
                (width, frame.room(0))
            };
            return Err(Error::Overflow { needed, available });
        }
        (true, Overflow::Shrink) if block => {
            let fits = |s: f64| flow.wrap(&frame, s, 0, false).len() <= scope.len();
            if fits(min_scale) {
                let (mut lo, mut hi) = (min_scale, 1.0);
                for _ in 0..10 {
                    let mid = (lo + hi) / 2.0;
                    if fits(mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                scale = lo;
                lines = flow.wrap(&frame, scale, 0, false);
            }
        }
        (true, Overflow::Shrink) => scale = (frame.room(0) / width).clamp(min_scale, 1.0),
        _ => {}
    }
    let runs = flow.place(&frame, &lines, scale, &scope)?;
    flow.edit.lines_after = lines.len();
    flow.edit.scale = scale;
    Ok(Plan {
        runs,
        scale,
        embed: flow.embed,
        edit: flow.edit,
    })
}

/// Writes a glyph-less `TJ` that moves the pen by `distance` text-space units.
fn put_shift(out: &mut Vec<u8>, distance: f64, unit: f64) {
    if unit.abs() > 1e-12 && distance.is_finite() && distance.abs() > 1e-9 {
        out.push(b'[');
        put_num(out, -distance * 1000.0 / unit);
        out.extend_from_slice(b"] TJ ");
    }
}

/// Writes the operations that replace a run's show operation. Returns the bytes and
/// the offset of the first show among them, or `None` when the run paints nothing.
fn emit(
    run: &RunPlan,
    scale: f64,
    fonts: &HashMap<u32, Vec<u8>>,
    gids: &[u16],
) -> (Vec<u8>, Option<usize>) {
    let mut out = vec![b' '];
    match run.show {
        K::Quote => out.extend_from_slice(b"T* "),
        K::DQuote => {
            put_nums(&mut out, &[run.tw]);
            out.extend_from_slice(b"Tw ");
            put_nums(&mut out, &[run.tc]);
            out.extend_from_slice(b"Tc T* ");
        }
        _ => {}
    }
    let th = run.th * scale;
    let unit = run.size * th;
    if run.puts.is_empty() || unit.abs() < 1e-12 {
        if run.followed {
            put_shift(&mut out, run.advance, run.size * run.th);
        }
        return (out, None);
    }
    if scale != 1.0 {
        put_nums(&mut out, &[th * 100.0]);
        out.extend_from_slice(b"Tz ");
    }
    let set_font = |out: &mut Vec<u8>, name: &[u8]| {
        put_name(out, name);
        out.push(b' ');
        put_nums(out, &[run.size]);
        out.extend_from_slice(b"Tf ");
    };
    let close = |out: &mut Vec<u8>, hex: &mut Vec<u8>, open: &mut bool| {
        if !hex.is_empty() {
            put_hex(out, hex);
            hex.clear();
        }
        if std::mem::take(open) {
            out.extend_from_slice(b"] TJ ");
        }
    };
    let (mut pen, mut dy, mut fallback) = (0.0f64, 0.0f64, None::<u32>);
    let (mut first, mut open, mut hex) = (None, false, Vec::new());
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * (1.0 + a.abs().max(b.abs()));
    for (k, p) in run.puts.iter().enumerate() {
        if p.dy != dy || p.glyph.fallback != fallback {
            close(&mut out, &mut hex, &mut open);
            if p.dy != dy {
                dy = p.dy;
                put_nums(&mut out, &[run.rise + dy]);
                out.extend_from_slice(b"Ts ");
            }
            if p.glyph.fallback != fallback {
                fallback = p.glyph.fallback;
                let name = fallback
                    .and_then(|i| fonts.get(&i))
                    .map_or(&run.font_name[..], Vec::as_slice);
                set_font(&mut out, name);
            }
        }
        if !open {
            first.get_or_insert(out.len());
            out.push(b'[');
            open = true;
        }
        let shift = p.x - pen;
        if let Some((at, _)) = run.reserve.filter(|r| k == 0 && !near(r.0, p.x)) {
            // Two numbers in a row: the pen between them is where the run began before
            // the edit, which is what forms lines (`RawText::reserve`).
            put_num(&mut out, -at * 1000.0 / unit);
            out.push(b' ');
            put_num(&mut out, -(p.x - at) * 1000.0 / unit);
        } else if shift.abs() > 1e-9 * (1.0 + pen.abs()) {
            if !hex.is_empty() {
                put_hex(&mut out, &hex);
                hex.clear();
            }
            put_num(&mut out, -shift * 1000.0 / unit);
        }
        let code = match p.glyph.fallback {
            Some(_) => gids.get(p.glyph.code as usize).map_or(0, |g| u32::from(*g)),
            None => p.glyph.code,
        };
        hex.extend_from_slice(&code.to_be_bytes()[4 - usize::from(p.glyph.len.clamp(1, 4))..]);
        let spacing = run.tc + if p.glyph.word_space { run.tw } else { 0.0 };
        pen = p.x + (p.glyph.w0 * run.size + spacing) * th;
    }
    if let (Some((_, at)), Some(p)) = (run.reserve, run.puts.last()) {
        if !near(at, p.x + p.glyph.w0 * run.size * th) {
            // The same at the end; the second number takes the pen on to where a
            // following show expects it.
            if !hex.is_empty() {
                put_hex(&mut out, &hex);
                hex.clear();
            }
            let end = if run.followed { run.advance } else { at };
            put_num(&mut out, -(at - pen) * 1000.0 / unit);
            out.push(b' ');
            put_num(&mut out, -(end - at) * 1000.0 / unit);
            pen = end;
        }
    }
    close(&mut out, &mut hex, &mut open);
    if fallback.is_some() {
        set_font(&mut out, &run.font_name);
    }
    if dy != 0.0 {
        put_nums(&mut out, &[run.rise]);
        out.extend_from_slice(b"Ts ");
    }
    if scale != 1.0 {
        put_nums(&mut out, &[run.th * 100.0]);
        out.extend_from_slice(b"Tz ");
    }
    if run.followed {
        put_shift(&mut out, run.advance - pen, run.size * run.th);
    }
    (out, first)
}

impl Document {
    /// Replaces the text of the line that object `id` belongs to. The line is not
    /// wrapped: text after the change moves along the baseline.
    ///
    /// Glyphs that did not change keep their codes and spacing. Characters the line's
    /// font cannot show are set in a fallback font when one was registered with
    /// [`Document::add_fallback_font`], and are otherwise left out and reported.
    pub fn replace_line(
        &mut self,
        page: u32,
        id: ObjId,
        text: &str,
        options: &TextOptions,
    ) -> Result<TextEdit> {
        self.replace(page, id, text, options, false)
    }

    /// Replaces the text of the block (paragraph, list item, table cell) that object
    /// `id` belongs to and re-wraps it inside the block's own width. Lines the change
    /// does not reach are not rewritten. `text` uses `\n` for hard line breaks; see
    /// [`crate::TextBlock::text`].
    pub fn replace_block(
        &mut self,
        page: u32,
        id: ObjId,
        text: &str,
        options: &TextOptions,
    ) -> Result<TextEdit> {
        self.replace(page, id, text, options, true)
    }

    fn replace(
        &mut self,
        page: u32,
        id: ObjId,
        text: &str,
        options: &TextOptions,
        block: bool,
    ) -> Result<TextEdit> {
        let index = self.locate(page, id)?;
        self.state(page)?;
        let state = self.cache.get(&page).ok_or(Error::NoSuchPage(page))?;
        let plan = plan(
            &self.pdf,
            state,
            &self.fallbacks,
            index,
            text,
            options,
            block,
        )?;
        if plan.runs.is_empty() {
            return Ok(plan.edit);
        }
        let mut step = self.begin("replace text", page);
        let outcome = (|| {
            let key = plan.runs[0].key;
            let mut gids = Vec::with_capacity(plan.embed.len());
            let mut fonts = HashMap::new();
            for (fi, gid, text) in &plan.embed {
                let face = self
                    .fallbacks
                    .get_mut(*fi as usize)
                    .ok_or_else(|| Error::Unsupported("unknown fallback font".into()))?;
                gids.push(face.subset_gid(*gid, text));
                fonts.entry(*fi).or_insert_with(Vec::new);
            }
            for (fi, name) in &mut fonts {
                let font = self.embed_fallback(&mut step, *fi as usize)?;
                *name = self.add_resource(&mut step, page, key, b"Font", "PlF", font)?;
            }
            let splices = plan.runs.iter().map(|run| {
                let (bytes, first) = emit(run, plan.scale, &fonts, &gids);
                Splice {
                    start: run.start,
                    end: run.end,
                    bytes,
                    fate: first.map_or(Fate::Remove, |at| Fate::Keep(at as u32)),
                }
            });
            self.splice(&mut step, page, key, splices.collect())
        })();
        match outcome {
            Ok(()) => {
                self.commit(step);
                Ok(plan.edit)
            }
            Err(e) => {
                self.rollback(step);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::geom::Rect;

    const PARA: &str = "The quick brown fox jumps over the lazy dog and keeps running through the quiet forest until it reaches the river.";

    /// `(id, text, bbox)` of every text run on page 1.
    fn runs(doc: &mut Document) -> Vec<(ObjId, String, Rect)> {
        let page = doc.page_objects(1).unwrap();
        page.objects
            .iter()
            .filter_map(|o| o.as_text().map(|t| (o.id, t.text.clone(), o.bbox)))
            .collect()
    }

    fn block_texts(doc: &mut Document) -> Vec<String> {
        doc.page_layout(1)
            .unwrap()
            .blocks
            .into_iter()
            .map(|b| b.text)
            .collect()
    }

    #[test]
    fn groups_runs_into_lines_and_blocks() {
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        let layout = doc.page_layout(1).unwrap();
        let texts: Vec<&str> = layout.blocks.iter().map(|b| b.text.as_str()).collect();
        // Two stacked lines of which the first is the longer read as one wrapped block.
        assert_eq!(
            texts,
            [PARA, "Right column second line", "Next block stays."]
        );
        assert_eq!(layout.blocks[0].lines.len(), 3);
        assert_eq!(layout.blocks[0].align, Align::Left);
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        assert_eq!(
            block_texts(&mut doc),
            ["Alpha Beta", "Gamma Delta", "In a form"]
        );
    }

    #[test]
    fn fixing_a_typo_leaves_the_rest_of_the_line_in_place() {
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        let before = runs(&mut doc);
        let glyphs = |doc: &mut Document, id: ObjId| {
            let page = doc.page_objects(1).unwrap();
            page.objects
                .iter()
                .find(|o| o.id == id)
                .unwrap()
                .as_text()
                .unwrap()
                .glyphs
                .clone()
        };
        let (id, old) = (before[1].0, glyphs(&mut doc, before[1].0));
        let edit = doc
            .replace_line(
                1,
                id,
                "keeps running through the quite forest until it",
                &TextOptions::default(),
            )
            .unwrap();
        assert_eq!(
            (edit.lines_before, edit.lines_after, edit.missing.len()),
            (1, 1, 0)
        );
        let after = runs(&mut doc);
        assert_eq!(
            after[1].1,
            "keeps running through the quite forest until it"
        );
        assert_eq!(after[1].0, id);
        // Up to "qui" every glyph is where it was; "te" swapped widths, so the tail is too.
        let new = glyphs(&mut doc, id);
        assert_eq!(new.len(), old.len());
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            let same = (a.bbox.x0 - b.bbox.x0).abs() < 1e-6;
            assert!(
                same || i == 30,
                "glyph {i} moved from {} to {}",
                a.bbox.x0,
                b.bbox.x0
            );
        }
        for i in [0, 2, 3, 4, 5] {
            assert_eq!(before[i], after[i]);
        }
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(block_texts(&mut again)[0], PARA.replace("quiet", "quite"));
        assert_eq!(doc.undo(), Some(1));
        assert_eq!(runs(&mut doc), before);
    }

    #[test]
    fn longer_text_pushes_the_tail_and_reports_what_it_cannot_show() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let before = runs(&mut doc);
        let edit = doc
            .replace_line(
                1,
                before[0].0,
                "Alpha \u{3A9}and Beta",
                &TextOptions::default(),
            )
            .unwrap();
        assert_eq!(edit.missing, ['\u{3A9}']);
        assert_eq!(
            edit.to_json(),
            "{\"missing\":\"\u{3A9}\",\"substituted\":\"\",\"fallback_fonts\":[],\"lines_before\":1,\"lines_after\":1,\"scale\":1,\"overlaps\":false,\"clipped\":false,\"needs_shaping\":false}"
        );
        assert_eq!(doc.info().to_json(), "{\"pages\":1,\"version\":\"1.5\",\"was_encrypted\":false,\"signed\":false,\"tagged\":false}");
        let after = runs(&mut doc);
        assert_eq!(
            block_texts(&mut doc),
            ["Alpha and Beta", "Gamma Delta", "In a form"]
        );
        // "and " in Helvetica 12pt: (556 + 556 + 556 + 278) / 1000 * 12.
        let beta = |list: &[(ObjId, String, Rect)]| {
            list.iter().find(|r| r.1.ends_with("Beta")).unwrap().2.x1
        };
        assert!((beta(&after) - beta(&before) - 23.352).abs() < 1e-6);
        // The lines below start where they did, although they are positioned relative to this one.
        for text in ["Gamma", "Delta", "In a form"] {
            let find =
                |list: &[(ObjId, String, Rect)]| list.iter().find(|r| r.1 == text).unwrap().clone();
            assert_eq!(find(&before), find(&after));
        }
    }

    #[test]
    fn reflows_a_block_without_touching_its_neighbours() {
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        let before = runs(&mut doc);
        let longer = PARA.replace("lazy dog", "lazy dog, the sleeping cat, two startled hens");
        let edit = doc
            .replace_block(1, before[0].0, &longer, &TextOptions::default())
            .unwrap();
        assert_eq!(
            (edit.lines_before, edit.lines_after, edit.overlaps),
            (3, 4, false)
        );
        let layout = doc.page_layout(1).unwrap();
        assert_eq!(layout.blocks[0].text, longer);
        assert_eq!(layout.blocks[0].lines.len(), 4);
        // No line's ink is wider than the block was (a trailing space may hang past the
        // edge, as it does in word processors), and the new line continues the leading.
        let ink = |doc: &mut Document| {
            let page = doc.page_objects(1).unwrap();
            let runs = page
                .objects
                .iter()
                .filter_map(|o| o.as_text())
                .filter(|t| t.origin.0 < 100.0 && t.origin.1 > 650.0);
            runs.map(|t| {
                t.glyphs[..t.text.trim_end().len().min(t.glyphs.len())]
                    .iter()
                    .map(|g| g.bbox.x1)
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max)
        };
        let width = before[..3].iter().map(|r| r.2.x1).fold(0.0, f64::max);
        assert!(ink(&mut doc) <= width + 0.06, "{} > {width}", ink(&mut doc));
        assert!((layout.blocks[0].lines[3].origin.1 - (700.0 - 3.0 * 14.0)).abs() < 1e-6);
        let after = runs(&mut doc);
        for text in ["Next block stays.", "Right column", "second line"] {
            let find =
                |list: &[(ObjId, String, Rect)]| list.iter().find(|r| r.1 == text).unwrap().clone();
            assert_eq!(find(&before), find(&after));
        }
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(block_texts(&mut again)[0], longer);
        // Deleting the insertion again pulls the text back up into three lines.
        let id = doc.page_layout(1).unwrap().blocks[0].id;
        let edit = doc
            .replace_block(1, id, PARA, &TextOptions::default())
            .unwrap();
        assert_eq!(edit.lines_after, 3);
        // Runs are now split where the text was re-wrapped, but every line is back in place.
        let lines = doc.page_layout(1).unwrap().blocks[0].lines.clone();
        let got: Vec<(&str, f64, f64)> = lines
            .iter()
            .map(|l| (l.text.trim_end(), l.origin.0, l.origin.1))
            .collect();
        let want: Vec<(&str, f64, f64)> = before[..3]
            .iter()
            .enumerate()
            .map(|(i, r)| (r.1.as_str(), 72.0, 700.0 - 14.0 * i as f64))
            .collect();
        assert_eq!(got.len(), 3);
        for (g, w) in got.iter().zip(&want) {
            assert!(
                g.0 == w.0 && (g.1 - w.1).abs() < 1e-6 && (g.2 - w.2).abs() < 1e-6,
                "{g:?} != {w:?}"
            );
        }
    }

    #[test]
    fn applies_the_overflow_policy() {
        let longer = PARA.replace(
            "river",
            "river that runs past the old mill, under the stone bridge, across the wide plain and out to the sea beyond the distant hills",
        );
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        let before = doc.save().unwrap();
        let id = runs(&mut doc)[0].0;
        let report = TextOptions {
            overflow: Overflow::Report,
            ..TextOptions::default()
        };
        assert!(
            matches!(doc.replace_block(1, id, &longer, &report), Err(Error::Overflow { needed, available }) if needed > available)
        );
        assert!(doc.history().is_empty());
        assert_eq!(doc.save().unwrap(), before);
        // Growing five lines down reaches the block below.
        let edit = doc
            .replace_block(1, id, &longer, &TextOptions::default())
            .unwrap();
        assert!(edit.lines_after >= 5 && edit.overlaps, "{edit:?}");
        doc.undo();
        let shrink = TextOptions {
            overflow: Overflow::Shrink,
            min_scale: 0.5,
            ..TextOptions::default()
        };
        let edit = doc.replace_block(1, id, &longer, &shrink).unwrap();
        assert_eq!((edit.lines_after, edit.overlaps), (3, false));
        assert!(edit.scale < 1.0 && edit.scale >= 0.5, "{}", edit.scale);
        assert_eq!(block_texts(&mut doc)[0], longer);
        let width = Document::from_bytes(&before)
            .unwrap()
            .page_objects(1)
            .unwrap()
            .objects[..3]
            .iter()
            .map(|o| o.bbox.x1)
            .fold(0.0, f64::max);
        assert!(doc.page_layout(1).unwrap().blocks[0].bbox.x1 <= width + 4.0);
    }

    /// `(text, font, size, colour)` of every text run on page 1.
    fn styles(doc: &mut Document) -> Vec<(String, String, f64, Option<[f32; 3]>)> {
        let page = doc.page_objects(1).unwrap();
        let runs = page.objects.iter().filter_map(|o| o.as_text());
        runs.map(|t| (t.text.clone(), t.font.clone(), t.size, t.color))
            .collect()
    }

    #[test]
    fn a_change_in_one_run_leaves_the_style_of_the_others() {
        let mut doc = Document::from_bytes(&fixtures::mixed_fonts()).unwrap();
        let before = styles(&mut doc);
        let boxes = runs(&mut doc);
        let look = |list: &[(String, String, f64, Option<[f32; 3]>)]| {
            list.iter()
                .map(|r| (r.1.clone(), r.2, r.3))
                .collect::<Vec<_>>()
        };
        let texts = |list: &[(String, String, f64, Option<[f32; 3]>)]| {
            list.iter().map(|r| r.0.clone()).collect::<Vec<_>>()
        };
        assert_eq!(
            look(&before),
            [
                ("Times-Roman".to_string(), 12.0, Some([0.0, 0.0, 0.0])),
                ("Times-Bold".to_string(), 12.0, Some([0.0, 0.0, 0.0])),
                ("Times-Roman".to_string(), 12.0, Some([0.0, 0.0, 0.0])),
                ("Times-Bold".to_string(), 12.0, Some([0.0, 0.0, 1.0])),
                ("Times-Roman".to_string(), 10.0, Some([0.0, 0.0, 0.0])),
            ]
        );
        let id = boxes[0].0;
        // One digit changes. The text comes without the line's trailing space, as a
        // host that trims its input sends it.
        let edit = doc
            .replace_line(
                1,
                id,
                "6. * Radha V et al., 2015, Indian J Hum Genet. 20:37-42.",
                &TextOptions::default(),
            )
            .unwrap();
        assert!(edit.missing.is_empty() && edit.substituted.is_empty());
        let after = styles(&mut doc);
        assert_eq!(look(&after), look(&before));
        assert_eq!(
            texts(&after),
            [
                "6. ",
                "* ",
                "Radha V et al., 2015, ",
                "Indian J Hum Genet.",
                " 20:37-42. "
            ]
        );
        // The digits of Times are equally wide: no run moved, and four were not rewritten.
        assert_eq!(runs(&mut doc)[3..], boxes[3..]);
        // Two changes in two runs, each typed in the style of the run it falls in.
        doc.replace_line(
            1,
            id,
            "6. * Radha V et al., 2015, Indian J Med Genet. 21:37-42. ",
            &TextOptions::default(),
        )
        .unwrap();
        let after = styles(&mut doc);
        assert_eq!(look(&after), look(&before));
        assert_eq!(after[3].0, "Indian J Med Genet.");
        assert_eq!(after[4].0, " 21:37-42. ");
        // Text typed at the end of a run continues that run's style. Which of the two
        // spaces around it is the old one is not defined, so spaces are not compared.
        doc.replace_line(
            1,
            id,
            "6. * Radha V et al., 2015, Indian J Med Genet. Suppl. 21:37-42. ",
            &TextOptions::default(),
        )
        .unwrap();
        let after = styles(&mut doc);
        assert_eq!(look(&after), look(&before));
        assert_eq!(after[3].0.trim(), "Indian J Med Genet. Suppl.");
        assert_eq!(after[4].0.trim(), "21:37-42.");
        // Corrections on both sides of a run boundary stay on their own side of it.
        doc.replace_line(
            1,
            id,
            "6. * Radha V et al., 2015, Indian J Med Genetics 31:37-42. ",
            &TextOptions::default(),
        )
        .unwrap();
        let after = styles(&mut doc);
        assert_eq!(look(&after), look(&before));
        assert_eq!(after[3].0.trim(), "Indian J Med Genetics");
        assert_eq!(after[4].0.trim(), "31:37-42.");
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(styles(&mut again), after);
        // A line that is written anew has no run left to keep: it takes one style.
        doc.replace_line(1, id, "Withdrawn.", &TextOptions::default())
            .unwrap();
        let after = styles(&mut doc);
        assert_eq!(texts(&after).concat().trim_end(), "Withdrawn.");
        assert!(after
            .iter()
            .filter(|r| !r.0.trim().is_empty())
            .all(|r| r.1 == "Times-Roman"));
    }

    /// The left edge of every glyph of every text run on page 1, run by run.
    fn glyph_starts(doc: &mut Document) -> Vec<Vec<f64>> {
        let page = doc.page_objects(1).unwrap();
        let runs = page.objects.iter().filter_map(|o| o.as_text());
        runs.map(|t| t.glyphs.iter().map(|g| g.bbox.x0).collect())
            .collect()
    }

    /// Replaces the line of run `first` and checks that run `run` still starts where it
    /// did, that its glyphs after the first moved by `delta`, and that no other run
    /// moved except those in `tail`, which follow it and moved by `delta` too.
    fn assert_start_holds(
        doc: &mut Document,
        first: usize,
        run: usize,
        text: &str,
        delta: f64,
        tail: &[usize],
    ) {
        let (before, old) = (runs(doc), glyph_starts(doc));
        doc.replace_line(1, before[first].0, text, &TextOptions::default())
            .unwrap();
        let (after, new) = (runs(doc), glyph_starts(doc));
        assert_eq!(after.len(), before.len());
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            let shift = if tail.contains(&i) { delta } else { 0.0 };
            if i == run {
                assert!(
                    (a.2.x0 - b.2.x0).abs() < 1e-6,
                    "run {i} moved from {} to {}",
                    b.2.x0,
                    a.2.x0
                );
                assert!((a.2.x1 - b.2.x1 - delta).abs() < 1e-6);
                assert_eq!(new[i].len(), old[i].len());
                assert!((new[i][0] - old[i][0]).abs() < 1e-6);
                for (k, (o, n)) in old[i].iter().zip(&new[i]).enumerate().skip(1) {
                    assert!((n - o - delta).abs() < 1e-6, "glyph {k}: {o} -> {n}");
                }
            } else {
                assert_eq!(a.1, b.1);
                for (k, (o, n)) in old[i].iter().zip(&new[i]).enumerate() {
                    assert!(
                        (n - o - shift).abs() < 1e-6,
                        "run {i} glyph {k}: {o} -> {n}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_run_moved_to_its_column_keeps_its_start_when_its_first_letter_changes() {
        let mut doc = Document::from_bytes(&fixtures::positioned_runs()).unwrap();
        let before = runs(&mut doc);
        let texts: Vec<&str> = before.iter().map(|r| r.1.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Agenda ",
                "File No.",
                " ",
                "NICU: ",
                "Neonatal intensive care unit"
            ]
        );
        assert!((before[0].2.x1 - 116.7).abs() < 1e-6);
        assert!((before[1].2.x0 - 132.0).abs() < 1e-6);
        // Helvetica at 12pt: "F" is 611 units wide and "i" 222.
        assert_start_holds(&mut doc, 0, 1, "Agenda iile No. ", -4.668, &[2]);
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(runs(&mut again)[1].1, "iile No.");
        assert!((runs(&mut again)[1].2.x0 - 132.0).abs() < 1e-6);
        assert_eq!(doc.undo(), Some(1));
        assert_eq!(runs(&mut doc), before);
        // A later letter of the same run: only what follows it moves.
        doc.replace_line(1, before[0].0, "Agenda Fille No. ", &TextOptions::default())
            .unwrap();
        let after = runs(&mut doc);
        assert_eq!(after[0], before[0]);
        assert!((after[1].2.x0 - 132.0).abs() < 1e-6);
        assert!((after[1].2.x1 - before[1].2.x1 - 2.664).abs() < 1e-6);
        doc.undo();
        // Text typed before the run pushes it, by the width of what was typed ("X ").
        doc.replace_line(
            1,
            before[0].0,
            "Agenda X File No. ",
            &TextOptions::default(),
        )
        .unwrap();
        let after = runs(&mut doc);
        assert_eq!(after[0].2.x0, before[0].2.x0);
        let file = after.iter().find(|r| r.1.ends_with("File No.")).unwrap();
        assert!((file.2.x1 - before[1].2.x1 - 11.34).abs() < 1e-6);
    }

    #[test]
    fn a_run_after_a_word_spacing_change_keeps_its_start_when_its_first_letter_changes() {
        let mut doc = Document::from_bytes(&fixtures::positioned_runs()).unwrap();
        let before = runs(&mut doc);
        // The run starts where `Td` put it, 32pt short of where the widened space
        // before it left the pen.
        assert!((before[3].2.x1 - 64.588).abs() < 1e-6);
        assert!((before[4].2.x0 - 65.2).abs() < 1e-6);
        // Helvetica at 12pt: "N" is 722 units wide and "n" 556.
        assert_start_holds(
            &mut doc,
            3,
            4,
            "NICU: neonatal intensive care unit",
            -1.992,
            &[],
        );
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(runs(&mut again)[4].1, "neonatal intensive care unit");
        assert!((runs(&mut again)[4].2.x0 - 65.2).abs() < 1e-6);
        assert_eq!(doc.undo(), Some(1));
        assert_eq!(runs(&mut doc), before);
        // The last letter of the run before it: the run follows by the width difference
        // (":" is 333 units wide in Helvetica-Bold and "." 278), and no further.
        doc.replace_line(
            1,
            before[3].0,
            "NICU. Neonatal intensive care unit",
            &TextOptions::default(),
        )
        .unwrap();
        let after = runs(&mut doc);
        assert_eq!(after[3].2.x0, before[3].2.x0);
        assert!((after[4].2.x0 - before[4].2.x0 + 0.66).abs() < 1e-6);
    }

    /// `(id, runs, align of the block, lines in the block)` of every line on page 1.
    fn line_sets(doc: &mut Document) -> Vec<(ObjId, Vec<ObjId>, Align, usize)> {
        let blocks = doc.page_layout(1).unwrap().blocks;
        let lines = blocks.iter().flat_map(|b| {
            b.lines
                .iter()
                .map(|l| (l.id, l.runs.clone(), b.align, b.lines.len()))
        });
        lines.collect()
    }

    /// Left and right edge of the glyphs that are not spaces in the given runs.
    fn visible(doc: &mut Document, ids: &[ObjId]) -> (f64, f64) {
        let page = doc.page_objects(1).unwrap();
        let (mut x0, mut x1) = (f64::MAX, f64::MIN);
        for o in page.objects.iter().filter(|o| ids.contains(&o.id)) {
            let t = o.as_text().unwrap();
            for (k, g) in t.glyphs.iter().enumerate() {
                let end = t
                    .glyphs
                    .get(k + 1)
                    .map_or(t.text.len(), |n| n.offset as usize);
                if !t.text[g.offset as usize..end].trim().is_empty() {
                    (x0, x1) = (x0.min(g.bbox.x0), x1.max(g.bbox.x1));
                }
            }
        }
        (x0, x1)
    }

    /// Replaces the line whose text is `old` and returns `(visible edges before,
    /// visible edges after)`. Checks that every line of the page keeps its id, its runs,
    /// its block and the block's alignment, also after saving and loading again.
    fn edit_line(doc: &mut Document, old: &str, new: &str) -> ((f64, f64), (f64, f64)) {
        let sets = line_sets(doc);
        let blocks = doc.page_layout(1).unwrap().blocks;
        let line = blocks
            .iter()
            .flat_map(|b| &b.lines)
            .find(|l| l.text == old)
            .unwrap_or_else(|| panic!("no line {old:?}"))
            .clone();
        let before = visible(doc, &line.runs);
        doc.replace_line(1, line.id, new, &TextOptions::default())
            .unwrap();
        assert_eq!(line_sets(doc), sets, "after {new:?}");
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        assert_eq!(line_sets(&mut again), sets, "after {new:?} and a reload");
        let text = |doc: &mut Document| {
            let blocks = doc.page_layout(1).unwrap().blocks;
            let found = blocks
                .iter()
                .flat_map(|b| &b.lines)
                .find(|l| l.id == line.id);
            found.unwrap().text.clone()
        };
        assert_eq!(text(doc), new);
        assert_eq!(text(&mut again), new);
        (before, visible(doc, &line.runs))
    }

    #[test]
    fn a_right_aligned_line_keeps_the_right_edge_of_its_visible_text() {
        let mut doc = Document::from_bytes(&fixtures::aligned_headings()).unwrap();
        let sets = line_sets(&mut doc);
        assert_eq!(sets.len(), 6);
        assert!(sets[..4].iter().all(|s| (s.2, s.3) == (Align::Right, 2)));
        // The space that ends the line is a run of its own.
        let (old, new) = edit_line(&mut doc, "Vol. 15, No. 3, 2024 ", "Vol. 15, Nos. 3, 2024 ");
        assert!((old.0 - 356.0).abs() < 1e-6 && (old.1 - 500.0).abs() < 1e-6);
        assert!((new.1 - 500.0).abs() < 1e-6, "right edge at {}", new.1);
        assert!((new.0 - 348.8).abs() < 1e-6, "left edge at {}", new.0);
        let (_, new) = edit_line(&mut doc, "Vol. 15, Nos. 3, 2024 ", "Vol. 15, N. 3, 2024 ");
        assert!((new.1 - 500.0).abs() < 1e-6 && (new.0 - 363.2).abs() < 1e-6);
        // Spaces at both ends of the only run.
        let (old, new) = edit_line(&mut doc, "   HEADING ONE  ", "   HEADING ONES  ");
        assert!((old.0 - 406.4).abs() < 1e-6 && (old.1 - 485.6).abs() < 1e-6);
        assert!((new.1 - 485.6).abs() < 1e-6, "right edge at {}", new.1);
        assert!((new.0 - 399.2).abs() < 1e-6, "left edge at {}", new.0);
    }

    #[test]
    fn a_centred_line_keeps_the_centre_of_its_visible_text() {
        let mut doc = Document::from_bytes(&fixtures::aligned_headings()).unwrap();
        let sets = line_sets(&mut doc);
        assert!(sets[4..].iter().all(|s| (s.2, s.3) == (Align::Center, 2)));
        let (old, new) = edit_line(&mut doc, "GUIDANCE DOCUMENT ", "GUIDANCE DOCUMENTS ");
        assert!(((old.0 + old.1) / 2.0 - 300.0).abs() < 1e-6);
        assert!(((new.0 + new.1) / 2.0 - 300.0).abs() < 1e-6, "{new:?}");
        assert!((new.1 - new.0 - 129.6).abs() < 1e-6);
        let (_, new) = edit_line(&mut doc, "GUIDANCE DOCUMENTS ", "GUIDANCE DOCUMEN ");
        assert!(((new.0 + new.1) / 2.0 - 300.0).abs() < 1e-6, "{new:?}");
    }

    #[test]
    fn editing_a_cell_leaves_the_other_cells_of_the_row_in_place() {
        let mut doc = Document::from_bytes(&fixtures::table_rows()).unwrap();
        let before = runs(&mut doc);
        let at = |doc: &mut Document, text: &str| {
            let all = runs(doc);
            all.iter().find(|r| r.1 == text).unwrap().2
        };
        let sets = line_sets(&mut doc);
        // Each header row is a line of its first cells and a line of its last cell.
        assert_eq!(sets[0].1.len(), 2);
        assert_eq!(sets[1].1.len(), 1);
        let row = |sets: &[(ObjId, Vec<ObjId>, Align, usize)]| {
            let n = sets.len();
            (sets[n - 2].1.len(), sets[n - 1].1.len())
        };
        assert_eq!(row(&sets), (3, 1));
        // A longer first cell: the second cell was placed on its own across a column
        // gap and stays, although 6pt further right it would stand on a column edge.
        edit_line(&mut doc, "Agen File Name", "Agent File Name");
        assert!((at(&mut doc, "File Name").x0 - 111.0).abs() < 1e-6);
        edit_line(&mut doc, "Agent File Name", "Ag File Name");
        assert!((at(&mut doc, "File Name").x0 - 111.0).abs() < 1e-6);
        // A longer last cell comes within two sizes of the next line and stays apart.
        edit_line(&mut doc, "Ag File Name", "Ag File Names");
        assert!((at(&mut doc, "File Names").x1 - 171.0).abs() < 1e-6);
        assert!((at(&mut doc, "Firm").x0 - 186.0).abs() < 1e-6);
        // A cell in the middle changes: the cell after it stays.
        let old = "Susceptible Intermediate Resistant";
        edit_line(&mut doc, old, "Susceptible Intermediates Resistant");
        assert!((at(&mut doc, "Intermediates").x0 - 153.0).abs() < 1e-6);
        assert!((at(&mut doc, "Resistant").x0 - 240.0).abs() < 1e-6);
        edit_line(
            &mut doc,
            "Susceptible Intermediates Resistant",
            "Susceptible nntermed Resistant",
        );
        assert!((at(&mut doc, "Resistant").x0 - 240.0).abs() < 1e-6);
        // Text that reaches the next cell pushes it, a space ahead.
        edit_line(
            &mut doc,
            "Susceptible nntermed Resistant",
            "Susceptible Intermediate or so Resistant",
        );
        assert!((at(&mut doc, "Intermediate or so").x1 - 261.0).abs() < 1e-6);
        assert!((at(&mut doc, "Resistant").x0 - 267.0).abs() < 1e-6);
        // Undoing everything restores the page.
        while doc.undo().is_some() {}
        assert_eq!(runs(&mut doc), before);
    }

    #[test]
    fn refuses_text_that_needs_shaping_unless_asked_to_write_it_unshaped() {
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        let before = runs(&mut doc);
        let id = before[1].0;
        // Hebrew is right-to-left, which no build of the engine lays out.
        let text = "keeps \u{5E9}\u{5DC}\u{5D5}\u{5DD} running";
        let refused = doc.replace_line(1, id, text, &TextOptions::default());
        assert!(matches!(refused, Err(Error::Unsupported(_))), "{refused:?}");
        assert_eq!(runs(&mut doc), before);
        assert_eq!(doc.undo(), None);
        let options = TextOptions {
            allow_unshaped: true,
            ..TextOptions::default()
        };
        let edit = doc.replace_line(1, id, text, &options).unwrap();
        assert!(edit.needs_shaping);
        // Text in the scripts already on the line is not affected.
        doc.undo();
        doc.replace_line(1, id, "keeps running", &TextOptions::default())
            .unwrap();
    }

    #[test]
    fn a_typed_bullet_uses_the_code_the_font_has_a_width_for() {
        let mut doc = Document::from_bytes(&fixtures::bullet_item()).unwrap();
        let before = runs(&mut doc);
        assert_eq!(before[0].1, "\u{2022} Item");
        let edit = doc
            .replace_line(
                1,
                before[0].0,
                "\u{2022} \u{2022} Item",
                &TextOptions::default(),
            )
            .unwrap();
        assert!(edit.missing.is_empty(), "{edit:?}");
        let after = runs(&mut doc);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].1, "\u{2022} \u{2022} Item");
        // Two more glyphs of 5pt each, every one with its own place.
        assert!((after[0].2.x1 - before[0].2.x1 - 10.0).abs() < 1e-6);
        let starts = &glyph_starts(&mut doc)[0];
        assert!(starts.windows(2).all(|w| (w[1] - w[0] - 5.0).abs() < 1e-6));
    }

    #[test]
    fn compares_old_units_with_new_text() {
        let units =
            |s: &'static str| -> Vec<&'static str> { (0..s.len()).map(|i| &s[i..i + 1]).collect() };
        let parts = |old: &'static str, new: &str| -> Vec<(String, String, bool)> {
            diff(&units(old), new)
                .iter()
                .map(|s| (old[s.a..s.b].to_string(), new[s.c..s.d].to_string(), s.same))
                .collect()
        };
        let seg = |a: &str, b: &str, same: bool| (a.to_string(), b.to_string(), same);
        assert_eq!(
            parts("4, Genet. 20:37 ", "5, Genet. 21:37"),
            [
                seg("4", "5", false),
                seg(", Genet. 2", ", Genet. 2", true),
                seg("0", "1", false),
                seg(":37", ":37", true),
                seg(" ", "", false),
            ]
        );
        // A letter that recurs by chance in a rewritten phrase is not kept...
        assert_eq!(
            parts("XaaaaaaqbbbbbbY", "ZccccccqddddddW"),
            [seg("XaaaaaaqbbbbbbY", "ZccccccqddddddW", false)]
        );
        assert_eq!(
            parts("Aquick brown foxB", "Cslow green owlD"),
            [seg("Aquick brown foxB", "Cslow green owlD", false)]
        );
        // ...but the space between two short corrections is.
        assert_eq!(
            parts(". 20", "ics 31"),
            [
                seg(".", "ics", false),
                seg(" ", " ", true),
                seg("20", "31", false)
            ]
        );
        assert_eq!(parts("", "new"), [seg("", "new", false)]);
        assert_eq!(parts("old", ""), [seg("old", "", false)]);
        assert!(diff(&[], "").is_empty());
        // A unit may stand for several characters, as a ligature does.
        let segs = diff(&["o", "ffi", "ce"], "offices");
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].b, segs[0].d, segs[0].same), (3, 6, true));
    }

    #[test]
    fn runs_painted_over_each_other_are_separate_lines() {
        const HEADING: &str = "What caused the most years lived with disability, by sex, in 2016?";
        const CAPTION: &str = "*COPD is chronic obstructive pulmonary disease.";
        let mut doc = Document::from_bytes(&fixtures::overprinted()).unwrap();
        let lines = |doc: &mut Document| -> Vec<(ObjId, String)> {
            let layout = doc.page_layout(1).unwrap();
            let lines = layout.blocks.iter().flat_map(|b| &b.lines);
            lines.map(|l| (l.id, l.text.clone())).collect()
        };
        let mut texts: Vec<String> = lines(&mut doc).into_iter().map(|l| l.1).collect();
        texts.sort();
        // A superscript and a run that starts a hair early still belong to their line.
        assert_eq!(
            texts,
            [
                CAPTION,
                "E = mc2 holds",
                "Heavy",
                "Heavy",
                "Slightly joined",
                HEADING
            ]
        );
        // Every run is on exactly one line.
        let layout = doc.page_layout(1).unwrap();
        let mut ids: Vec<ObjId> = layout
            .blocks
            .iter()
            .flat_map(|b| &b.lines)
            .flat_map(|l| l.runs.clone())
            .collect();
        ids.sort_unstable();
        let mut all: Vec<ObjId> = runs(&mut doc).iter().map(|r| r.0).collect();
        all.sort_unstable();
        assert_eq!(ids, all);
        // Retyping the heading writes the heading, and leaves the caption alone.
        let before = runs(&mut doc);
        let id = lines(&mut doc).iter().find(|l| l.1 == HEADING).unwrap().0;
        let retyped = HEADING.replace("2016", "2017");
        doc.replace_line(1, id, &retyped, &TextOptions::default())
            .unwrap();
        let after = runs(&mut doc);
        assert_eq!(after.len(), before.len());
        for (b, a) in before.iter().zip(&after) {
            if b.1 == HEADING {
                assert_eq!(a.1, retyped);
            } else {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn refuses_fallback_fonts_that_may_not_be_embedded() {
        let mut doc = Document::from_bytes(&fixtures::simple()).unwrap();
        // Installable, preview and print, editable.
        for fs_type in [0x0000, 0x0004, 0x0008] {
            let added = doc.add_fallback_font(fixtures::tiny_font(fs_type));
            assert!(added.is_ok(), "{fs_type:#06x}: {added:?}");
        }
        // Restricted licence, no subsetting (alone and with editable), bitmaps only.
        for fs_type in [0x0002, 0x0100, 0x0108, 0x0200] {
            let added = doc.add_fallback_font(fixtures::tiny_font(fs_type));
            assert!(
                matches!(added, Err(Error::Unsupported(_))),
                "{fs_type:#06x}: {added:?}"
            );
        }
    }

    #[test]
    fn embeds_a_fallback_font_when_one_is_registered() {
        // The engine bundles no fonts; this test runs where a system font is at hand.
        let Ok(font) = std::fs::read("/System/Library/Fonts/Supplemental/Arial.ttf") else {
            return;
        };
        let mut doc = Document::from_bytes(&fixtures::paragraph()).unwrap();
        assert_eq!(doc.add_fallback_font(font).unwrap(), ["ArialMT"]);
        assert!(doc.add_fallback_font(b"not a font".to_vec()).is_err());
        let id = runs(&mut doc)
            .iter()
            .find(|r| r.1 == "Next block stays.")
            .unwrap()
            .0;
        let edit = doc
            .replace_line(
                1,
                id,
                "Next block costs 5\u{20AC} \u{2192} \u{3A9}.",
                &TextOptions::default(),
            )
            .unwrap();
        // Helvetica's standard encoding has none of the three; all come from Arial.
        assert_eq!(
            (edit.missing.len(), edit.substituted.as_slice()),
            (0, &['\u{20AC}', '\u{2192}', '\u{3A9}'][..])
        );
        assert_eq!(edit.fallback_fonts, ["ArialMT"]);
        let saved = doc.save().unwrap();
        let mut again = Document::from_bytes(&saved).unwrap();
        assert!(block_texts(&mut again)
            .contains(&"Next block costs 5\u{20AC} \u{2192} \u{3A9}.".to_string()));
        let fonts: Vec<String> = again
            .page_objects(1)
            .unwrap()
            .objects
            .iter()
            .filter_map(|o| o.as_text().map(|t| t.font.clone()))
            .collect();
        assert!(fonts.contains(&"ArialMT".to_string()));
        // Only the glyphs in use are embedded: the subset is a small fraction of the font.
        assert!(saved.len() < 20_000, "{}", saved.len());
        // Text in the embedded subset can be edited again with the glyphs it has.
        let id = again
            .page_layout(1)
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.text.starts_with("Next"))
            .unwrap()
            .id;
        let edit = again
            .replace_line(
                1,
                id,
                "Next \u{3A9}\u{2192} block.",
                &TextOptions::default(),
            )
            .unwrap();
        assert!(
            edit.missing.is_empty() && edit.substituted.is_empty(),
            "{edit:?}"
        );
    }
}
