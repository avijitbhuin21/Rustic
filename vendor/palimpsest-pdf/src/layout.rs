//! Reading order: text runs grouped into lines, and lines into blocks.

use crate::doc::PageState;
use crate::geom::Rect;
use crate::interp::{RawKind, RawText};
use crate::model::{json_num, json_nums, json_rect, json_str, ObjId};

/// How the lines of a block are aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
    Justified,
}

impl Align {
    /// A stable lower-case name, used in JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Right => "right",
            Align::Center => "center",
            Align::Justified => "justified",
        }
    }
}

/// One line of text: runs that share a baseline and are not separated by a column gap.
/// Runs painted over each other are never one line; each is a line of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    /// The id of the line's first run. Pass it to [`crate::Document::replace_line`].
    pub id: ObjId,
    /// The runs of the line in reading order.
    pub runs: Vec<ObjId>,
    /// The line's text, with a space wherever the runs leave a word gap.
    pub text: String,
    pub bbox: Rect,
    /// Start of the baseline in page space.
    pub origin: (f64, f64),
    /// Baseline direction in degrees, counter-clockwise from the x axis.
    pub angle: f64,
    /// The largest font size on the line, in page-space points.
    pub size: f64,
}

/// A paragraph, list item, heading or table cell: consecutive lines that wrap together.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    /// The id of the block's first line. Pass it to [`crate::Document::replace_block`].
    pub id: ObjId,
    pub bbox: Rect,
    pub align: Align,
    /// The block's text as it flows: wrapped lines are joined by a space (or by nothing
    /// after a trailing space or hyphen), and only hard breaks appear as `\n`. Edit
    /// this string and pass it to [`crate::Document::replace_block`].
    pub text: String,
    pub lines: Vec<TextLine>,
}

/// The text of a page in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLayout {
    pub page: u32,
    pub blocks: Vec<TextBlock>,
}

impl PageLayout {
    /// Serialises the layout as JSON.
    pub fn to_json(&self) -> String {
        let mut out = format!("{{\"page\":{},\"blocks\":[", self.page);
        for (i, b) in self.blocks.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"id\":{},\"align\":\"{}\",\"bbox\":",
                b.id,
                b.align.as_str()
            ));
            json_rect(&mut out, &b.bbox);
            out.push_str(",\"text\":");
            json_str(&mut out, &b.text);
            out.push_str(",\"lines\":[");
            for (j, l) in b.lines.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str(&format!("{{\"id\":{},\"bbox\":", l.id));
                json_rect(&mut out, &l.bbox);
                out.push_str(",\"origin\":");
                json_nums(&mut out, &[l.origin.0, l.origin.1]);
                out.push_str(",\"angle\":");
                json_num(&mut out, l.angle);
                out.push_str(",\"size\":");
                json_num(&mut out, l.size);
                out.push_str(",\"runs\":");
                json_nums(
                    &mut out,
                    &l.runs.iter().map(|r| f64::from(*r)).collect::<Vec<_>>(),
                );
                out.push_str(",\"text\":");
                json_str(&mut out, &l.text);
                out.push('}');
            }
            out.push_str("]}");
        }
        out.push_str("]}");
        out
    }
}

/// Marks an [`Item`] that is a word gap rather than a glyph.
pub(crate) const GAP: u32 = u32::MAX;

/// One glyph or word gap of a line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Item {
    /// Index into the page's interpreted items.
    pub run: u32,
    /// Glyph index within the run, or [`GAP`].
    pub glyph: u32,
    /// Extent along the baseline, in page-space points from the line's frame origin.
    pub t0: f64,
    pub t1: f64,
    /// Byte range of this item's text in [`LineGeom::text`].
    pub text: (u32, u32),
}

/// A run's placement within its line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunGeom {
    pub index: u32,
    /// Position of the run's text-space origin along the baseline.
    pub t: f64,
    /// Page-space points per text-space unit along the baseline.
    pub s: f64,
    pub t0: f64,
    pub t1: f64,
    /// The extent that decides which line the run belongs to: what its glyphs cover,
    /// or what they covered before a text edit (see `RawText::reserve`).
    pub g0: f64,
    pub g1: f64,
    /// Baseline offset across the line direction.
    pub n: f64,
    /// Font size in page-space points.
    pub size: f64,
    /// The text-space y unit vector in the line's frame: across and along the baseline.
    pub vn: f64,
    pub vu: f64,
}

#[derive(Debug)]
pub(crate) struct LineGeom {
    pub runs: Vec<RunGeom>,
    pub items: Vec<Item>,
    pub text: String,
    /// Unit vector along the baseline.
    pub u: (f64, f64),
    pub n: f64,
    pub t0: f64,
    pub t1: f64,
    pub size: f64,
    pub stream: u32,
    pub bucket: i32,
    pub bbox: Rect,
}

#[derive(Debug)]
pub(crate) struct BlockGeom {
    pub lines: Vec<usize>,
    pub align: Align,
    /// Right edge of the widest line, along the baseline.
    pub t1: f64,
}

#[derive(Debug, Default)]
pub(crate) struct Layout {
    pub lines: Vec<LineGeom>,
    pub blocks: Vec<BlockGeom>,
}

impl LineGeom {
    /// The text of one item.
    pub fn item_text(&self, item: &Item) -> &str {
        self.text
            .get(item.text.0 as usize..item.text.1 as usize)
            .unwrap_or("")
    }

    /// The point on the baseline at distance `t` along it.
    pub fn point(&self, t: f64) -> (f64, f64) {
        (
            self.u.0 * t - self.u.1 * self.n,
            self.u.1 * t + self.u.0 * self.n,
        )
    }
}

/// A run measured in its line's frame, before lines are formed.
struct Cand {
    geom: RunGeom,
    bucket: i32,
    stream: u32,
    u: (f64, f64),
}

/// Measures a text run in the frame of its baseline direction.
fn measure(index: usize, run: &RawText, stream: u32, state: &PageState) -> Option<Cand> {
    let item = state.interp.items.get(index)?;
    let m = run.tm.then(&item.ctm).then(&state.to_page);
    let (ax, ay) = m.apply_vec(1.0, 0.0);
    let s = ax.hypot(ay);
    let (bx, by) = m.apply_vec(0.0, 1.0);
    let size = run.size.abs() * bx.hypot(by);
    if !(s > 1e-9 && size > 1e-6 && s.is_finite() && size.is_finite())
        || run.font.vertical
        || run.render == 3
    {
        return None;
    }
    // A negative size or horizontal scale flips the direction glyphs advance in.
    let flip = if run.size * run.th < 0.0 { -1.0 } else { 1.0 };
    let u = (ax / s * flip, ay / s * flip);
    let (ox, oy) = m.apply(0.0, run.rise);
    let (mut x0, mut x1) = (f64::MAX, f64::MIN);
    for g in &run.glyphs {
        x0 = x0.min(g.x.min(g.x + f64::from(g.w)));
        x1 = x1.max(g.x.max(g.x + f64::from(g.w)));
    }
    let t = ox * u.0 + oy * u.1;
    let s = s * flip;
    let (a, b) = (t + x0 * s, t + x1 * s);
    let (t0, t1) = (a.min(b), a.max(b));
    let reserved = |x: Option<f64>, own: f64| {
        x.map(|x| t + x * s)
            .filter(|v| v.is_finite())
            .unwrap_or(own)
    };
    let (g0, g1) = (reserved(run.reserve.0, t0), reserved(run.reserve.1, t1));
    let (g0, g1) = if g0 <= g1 { (g0, g1) } else { (t0, t1) };
    Some(Cand {
        geom: RunGeom {
            index: index as u32,
            t,
            s,
            t0,
            t1,
            g0,
            g1,
            n: oy * u.0 - ox * u.1,
            size,
            vn: by * u.0 - bx * u.1,
            vu: bx * u.0 + by * u.1,
        },
        bucket: (u.1.atan2(u.0).to_degrees() * 2.0).round() as i32,
        stream,
        u,
    })
}

/// How two consecutive lines of a block join into running text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Join {
    /// A soft break that reads as a space.
    Space,
    /// A soft break with nothing between: after a space or a hyphenated word part.
    Nothing,
    /// A hard break, kept when the block is re-wrapped.
    Break,
}

/// Decides how `prev` joins the line after it in a block aligned as `align`.
pub(crate) fn join(prev: &LineGeom, next: &LineGeom, align: Align) -> Join {
    if matches!(align, Align::Center | Align::Right) {
        return Join::Break;
    }
    let mut tail = prev.text.chars().rev();
    match (tail.next(), tail.next(), next.text.chars().next()) {
        (Some(c), _, _) if c.is_whitespace() => Join::Nothing,
        (Some('-' | '\u{2010}'), Some(a), Some(b)) if a.is_alphabetic() && b.is_lowercase() => {
            Join::Nothing
        }
        _ => Join::Space,
    }
}

/// The stretches of the baseline that a run's visible glyphs cover, merged and in order.
fn ink(state: &PageState, c: &Cand) -> Vec<(f64, f64)> {
    let g = &c.geom;
    let Some(RawKind::Text(run)) = state.interp.items.get(g.index as usize).map(|i| &i.kind) else {
        return Vec::new();
    };
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for (k, glyph) in run.glyphs.iter().enumerate() {
        let end = run
            .glyphs
            .get(k + 1)
            .map_or(run.text.len(), |n| n.at as usize);
        let text = run
            .text
            .get(glyph.at as usize..end.max(glyph.at as usize))
            .unwrap_or("");
        if !text.is_empty() && text.chars().all(char::is_whitespace) {
            continue;
        }
        let (a, b) = (
            g.t + glyph.x * g.s,
            g.t + (glyph.x + f64::from(glyph.w)) * g.s,
        );
        if (b - a).abs() > 1e-9 {
            spans.push((a.min(b), a.max(b)));
        }
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for s in spans {
        match merged.last_mut() {
            Some(last) if s.0 <= last.1 => last.1 = last.1.max(s.1),
            _ => merged.push(s),
        }
    }
    merged
}

/// The length of baseline that two lists of merged, ordered spans both cover.
fn shared(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let (mut i, mut j, mut sum) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        sum += (a[i].1.min(b[j].1) - a[i].0.max(b[j].0)).max(0.0);
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    sum
}

/// Runs of one line that do not paint over each other.
struct Track {
    /// `(index into the candidates, furthest end of the members up to this one)`.
    members: Vec<(usize, f64)>,
    /// The largest member.
    lead: usize,
    reach: f64,
}

/// Separates runs that share a baseline into lines whose runs do not paint over each
/// other: a caption set across a heading, a subscript under a superscript. Read as one
/// line, the glyphs of such runs interleave into text that nobody wrote. `cands` is
/// sorted along the baseline; so is each line returned.
fn split_overlaps<'a>(state: &PageState, cands: &'a [Cand]) -> Vec<Vec<&'a Cand>> {
    let mut reach = f64::MIN;
    let clear = cands.iter().all(|c| {
        let ok = c.geom.t0 >= reach - 1e-6;
        reach = reach.max(c.geom.t1);
        ok
    });
    if clear {
        return vec![cands.iter().collect()];
    }
    let inks: Vec<Vec<(f64, f64)>> = cands.iter().map(|c| ink(state, c)).collect();
    let covered: Vec<f64> = inks
        .iter()
        .map(|spans| spans.iter().map(|s| s.1 - s.0).sum())
        .collect();
    // Kerning and italic overhang overlap by a fraction of a glyph. More than half of
    // the shorter run, or more than half an em, is one text set on top of another.
    let collide = |a: usize, b: usize| {
        let em = cands[a].geom.size.max(cands[b].geom.size);
        shared(&inks[a], &inks[b]) > 0.5 * covered[a].min(covered[b]).min(em)
    };
    let mut tracks: Vec<Track> = Vec::new();
    // Bounds the work on a page built to overlap everything with everything.
    let mut budget = 200_000u32;
    for (k, c) in cands.iter().enumerate() {
        let g = &c.geom;
        // `(track, (not the same style, distance))`: a run continues text of its own
        // baseline and size where it can, and otherwise joins the largest text.
        let mut best: Option<(usize, (bool, f64))> = None;
        for (ti, track) in tracks.iter().enumerate() {
            let mut hit = false;
            for &(m, upto) in track.members.iter().rev() {
                if hit || budget == 0 || upto <= g.t0 + 1e-6 {
                    break;
                }
                budget -= 1;
                hit = cands[m].geom.t1 > g.t0 + 1e-6 && collide(m, k);
            }
            if hit {
                continue;
            }
            let lead = &cands[track.lead].geom;
            let big = lead.size.max(g.size);
            let twin =
                (lead.n - g.n).abs() <= 0.05 * big && (lead.size - g.size).abs() <= 0.05 * big;
            let key = if twin {
                (false, (g.t0 - track.reach).abs())
            } else {
                (true, -lead.size)
            };
            if best.is_none_or(|(_, b)| key < b) {
                best = Some((ti, key));
            }
        }
        match best {
            Some((ti, _)) => {
                let track = &mut tracks[ti];
                track.reach = track.reach.max(g.t1);
                track.members.push((k, track.reach));
                if g.size > cands[track.lead].geom.size {
                    track.lead = k;
                }
            }
            None => tracks.push(Track {
                members: vec![(k, g.t1)],
                lead: k,
                reach: g.t1,
            }),
        }
    }
    tracks
        .into_iter()
        .map(|t| t.members.into_iter().map(|(k, _)| &cands[k]).collect())
        .collect()
}

/// Builds one line from runs sorted along the baseline.
fn make_line(state: &PageState, cands: &[&Cand]) -> Option<LineGeom> {
    let lead = cands
        .iter()
        .max_by(|a, b| a.geom.size.total_cmp(&b.geom.size))?;
    let mut line = LineGeom {
        runs: cands.iter().map(|c| c.geom).collect(),
        items: Vec::new(),
        text: String::new(),
        u: lead.u,
        n: lead.geom.n,
        t0: f64::MAX,
        t1: f64::MIN,
        size: lead.geom.size,
        stream: lead.stream,
        bucket: lead.bucket,
        bbox: Rect::new(0.0, 0.0, 0.0, 0.0),
    };
    let mut bbox: Option<Rect> = None;
    // `(sort key, run slot, glyph index, start, end)`. Glyphs are read in the order they
    // stand on the baseline, whichever run paints them; within a run the key never
    // decreases, so marks and kerned pairs keep their logical order.
    let mut glyphs: Vec<(f64, u32, u32, f64, f64)> = Vec::new();
    for (slot, c) in cands.iter().enumerate() {
        let g = &c.geom;
        line.t0 = line.t0.min(g.t0);
        line.t1 = line.t1.max(g.t1);
        if let Some(o) = state.objects.objects.get(g.index as usize) {
            bbox = Some(bbox.map_or(o.bbox, |b| b.union(&o.bbox)));
        }
        let Some(RawKind::Text(run)) = state.interp.items.get(g.index as usize).map(|i| &i.kind)
        else {
            continue;
        };
        let mut key = f64::MIN;
        for (k, glyph) in run.glyphs.iter().enumerate() {
            let (a, b) = (
                g.t + glyph.x * g.s,
                g.t + (glyph.x + f64::from(glyph.w)) * g.s,
            );
            key = key.max(a.min(b));
            glyphs.push((key, slot as u32, k as u32, a.min(b), a.max(b)));
        }
    }
    glyphs.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, slot, k, t0, t1) in glyphs {
        let g = &cands[slot as usize].geom;
        let Some(RawKind::Text(run)) = state.interp.items.get(g.index as usize).map(|i| &i.kind)
        else {
            continue;
        };
        let glyph = &run.glyphs[k as usize];
        let end = run
            .glyphs
            .get(k as usize + 1)
            .map_or(run.text.len(), |n| n.at as usize);
        let mut text = run
            .text
            .get(glyph.at as usize..end.max(glyph.at as usize))
            .unwrap_or("");
        if glyph.gap {
            text = text.strip_suffix(' ').unwrap_or(text);
        }
        if let Some(prev) = line.items.last().copied() {
            let blank = |s: &str| s.chars().all(char::is_whitespace);
            let wide = t0 - prev.t1 > 0.18 * g.size.max(1e-6) * run.th.abs().min(1.0);
            if wide && !blank(line.item_text(&prev)) && !blank(text) {
                let at = line.text.len() as u32;
                line.text.push(' ');
                line.items.push(Item {
                    run: slot,
                    glyph: GAP,
                    t0: prev.t1,
                    t1: t0,
                    text: (at, at + 1),
                });
            }
        }
        let at = line.text.len() as u32;
        line.text.push_str(text);
        line.items.push(Item {
            run: slot,
            glyph: k,
            t0,
            t1,
            text: (at, line.text.len() as u32),
        });
    }
    line.bbox = bbox?;
    (!line.text.trim().is_empty()).then_some(line)
}

/// Groups the text runs of a page into lines, in reading order.
fn build_lines(state: &PageState) -> Vec<LineGeom> {
    let mut cands: Vec<Cand> = Vec::new();
    for (i, item) in state.interp.items.iter().enumerate() {
        if let RawKind::Text(run) = &item.kind {
            cands.extend(measure(i, run, item.stream, state));
        }
    }
    cands.sort_by(|a, b| {
        (a.stream, a.bucket)
            .cmp(&(b.stream, b.bucket))
            .then(b.geom.n.total_cmp(&a.geom.n))
            .then(a.geom.t0.total_cmp(&b.geom.t0))
    });
    // Pass 1: runs that share a baseline. Super- and subscripts may sit up to half a size
    // away from the baseline of the largest run seen so far.
    let mut clusters: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < cands.len() {
        let group = (cands[i].stream, cands[i].bucket);
        let (mut base, mut size) = (cands[i].geom.n, cands[i].geom.size);
        let mut j = i + 1;
        while j < cands.len() && (cands[j].stream, cands[j].bucket) == group {
            let c = &cands[j].geom;
            let big = size.max(c.size);
            let tol = if (c.size - size).abs() > 0.15 * big {
                0.5 * big
            } else {
                0.25 * big
            };
            if base - c.n > tol {
                break;
            }
            if c.size > 1.15 * size {
                (base, size) = (c.n, c.size);
            }
            j += 1;
        }
        cands[i..j].sort_by(|a, b| a.geom.t0.total_cmp(&b.geom.t0));
        clusters.push((i, j));
        i = j;
    }
    // Where text starts after a clear gap, or at the left of its baseline. A column of a
    // table shows up as the same start on several baselines.
    let gap_at = |k: usize, reach: f64| {
        (cands[k].geom.g0 - reach) / cands[k].geom.size.max(cands[k - 1].geom.size).max(1e-6)
    };
    let mut edges: Vec<(u32, i32, f64)> = Vec::new();
    for &(i, j) in &clusters {
        let mut reach = f64::MIN;
        for (k, c) in cands.iter().enumerate().take(j).skip(i) {
            if k == i || gap_at(k, reach) >= 1.0 {
                edges.push((c.stream, c.bucket, c.geom.g0));
            }
            reach = reach.max(c.geom.g1);
        }
    }
    edges.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
    let aligned = |c: &Cand| {
        let lo = edges.partition_point(|e| (e.0, e.1, e.2) < (c.stream, c.bucket, c.geom.g0 - 1.5));
        edges[lo..]
            .iter()
            .take_while(|e| (e.0, e.1) == (c.stream, c.bucket) && e.2 <= c.geom.g0 + 1.5)
            .count()
            >= 3
    };
    // Pass 2: a gap of two font sizes, or of one where a column starts, separates cells.
    let mut lines = Vec::new();
    for &(i, j) in &clusters {
        let mut start = i;
        let mut reach = cands[i].geom.g1;
        for k in i + 1..=j {
            let split =
                k == j || gap_at(k, reach) > 2.0 || (gap_at(k, reach) >= 1.0 && aligned(&cands[k]));
            if split {
                for runs in split_overlaps(state, &cands[start..k]) {
                    lines.extend(make_line(state, &runs));
                }
                start = k;
            }
            if k < j {
                reach = if split {
                    cands[k].geom.g1
                } else {
                    reach.max(cands[k].geom.g1)
                };
            }
        }
    }
    lines
}

/// Width of a line's first word, up to its first space.
fn first_word(line: &LineGeom) -> f64 {
    let stop = line
        .items
        .iter()
        .find(|i| line.item_text(i).chars().all(char::is_whitespace));
    stop.map_or(line.t1, |i| i.t0) - line.t0
}

/// Detects how a group of consecutive lines is aligned.
fn alignment(lines: &[LineGeom], group: &[usize]) -> Align {
    if group.len() < 2 {
        return Align::Left;
    }
    let size = lines[group[0]].size;
    let tol = (0.12 * size).max(0.75);
    let spread = |values: &[f64]| {
        let (lo, hi) = values
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        hi - lo
    };
    let lefts: Vec<f64> = group.iter().map(|i| lines[*i].t0).collect();
    let rights: Vec<f64> = group.iter().map(|i| lines[*i].t1).collect();
    let centers: Vec<f64> = lefts
        .iter()
        .zip(&rights)
        .map(|(a, b)| (a + b) / 2.0)
        .collect();
    // A first-line indent and a short last line do not change the alignment.
    let left = spread(&lefts[1..]) <= tol && lefts[0] >= lefts[1] - tol;
    let full_right = spread(&rights) <= tol;
    let body_right = group.len() >= 3
        && spread(&rights[..rights.len() - 1]) <= tol
        && rights[rights.len() - 1] <= rights[0] + tol;
    if left && (full_right || body_right) {
        Align::Justified
    } else if left {
        Align::Left
    } else if full_right {
        Align::Right
    } else if spread(&centers) <= tol {
        Align::Center
    } else {
        Align::Left
    }
}

/// True when a line starts like a list item: a bullet, a checkbox, `1.`, `(a)`, `ii)`.
fn starts_item(text: &str) -> bool {
    let t = text.trim_start();
    let mut chars = t.chars();
    match chars.next() {
        Some(
            '\u{2022}' | '\u{25E6}' | '\u{25AA}' | '\u{25CF}' | '\u{2023}' | '\u{2043}'
            | '\u{00B7}' | '\u{2013}' | '\u{2014}' | '[',
        ) => true,
        Some('-' | '*') => chars.next().is_some_and(char::is_whitespace),
        Some(first) => {
            let body = t.strip_prefix('(').unwrap_or(t);
            let label: String = body.chars().take_while(|c| c.is_alphanumeric()).collect();
            let rest = &body[label.len()..];
            let closed = rest.starts_with(['.', ')']) && rest[1..].starts_with(char::is_whitespace);
            let short = !label.is_empty() && label.chars().count() <= 3;
            let numeric = label.chars().all(|c| c.is_ascii_digit())
                || label.chars().count() == 1
                || t.starts_with('(');
            closed && short && numeric && (first == '(' || first.is_alphanumeric())
        }
        None => false,
    }
}

/// Groups lines into blocks.
fn build_blocks(lines: &[LineGeom]) -> Vec<BlockGeom> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        // `(group, gap, steal)`: steal means the group's last line belongs with this one.
        let mut best: Option<(usize, f64, bool)> = None;
        for (gi, group) in groups.iter().enumerate().rev().take(48) {
            let last = &lines[group[group.len() - 1]];
            if (last.stream, last.bucket) != (line.stream, line.bucket) {
                break;
            }
            let gap = last.n - line.n;
            let size = last.size.max(line.size);
            let overlap = last.t1.min(line.t1) - last.t0.max(line.t0);
            let narrow = (last.t1 - last.t0).min(line.t1 - line.t0);
            // Lines of one block share a left edge (give or take an indent), a right edge or a centre.
            let edge = (last.t0 - line.t0).abs() <= 3.0 * size
                || (last.t1 - line.t1).abs() <= 0.25 * size
                || ((last.t0 + last.t1) - (line.t0 + line.t1)).abs() <= 0.5 * size;
            let near = edge
                && gap > 0.3 * size
                && gap <= 2.0 * size
                && (last.size - line.size).abs() <= 0.12 * size
                && overlap >= 0.5 * narrow;
            if !near || best.is_some_and(|(_, g, _)| g <= gap) {
                continue;
            }
            let lead = (group.len() >= 2).then(|| lines[group[group.len() - 2]].n - last.n);
            match lead {
                Some(l) if (gap - l).abs() > 0.2 * l => {
                    // A heading followed by a paragraph: the pair below is spaced tighter.
                    if group.len() == 2 && gap < 0.8 * l {
                        best = Some((gi, gap, true));
                    }
                }
                _ => best = Some((gi, gap, false)),
            }
        }
        match best {
            Some((gi, _, false)) => groups[gi].push(li),
            Some((gi, _, true)) => {
                let stolen = groups[gi].pop();
                groups.push(stolen.into_iter().chain([li]).collect());
            }
            None => groups.push(vec![li]),
        }
    }
    let mut blocks = Vec::new();
    for group in groups {
        let align = alignment(lines, &group);
        let right = group.iter().map(|i| lines[*i].t1).fold(f64::MIN, f64::max);
        let mut parts: Vec<Vec<usize>> = vec![Vec::new()];
        for (k, li) in group.iter().enumerate() {
            if k > 0 && matches!(align, Align::Left | Align::Justified) {
                // The next line's first word would have fitted: the break is a hard one.
                let prev = &lines[group[k - 1]];
                let fitted = prev.t1 + 0.25 * prev.size + first_word(&lines[*li]) <= right - 0.5;
                if fitted || starts_item(&lines[*li].text) {
                    parts.push(Vec::new());
                }
            }
            if let Some(part) = parts.last_mut() {
                part.push(*li);
            }
        }
        for part in parts.into_iter().filter(|p| !p.is_empty()) {
            let t1 = part.iter().map(|i| lines[*i].t1).fold(f64::MIN, f64::max);
            let align = if part.len() >= 2 { align } else { Align::Left };
            blocks.push(BlockGeom {
                lines: part,
                align,
                t1,
            });
        }
    }
    blocks.sort_by(|a, b| {
        let (x, y) = (&lines[a.lines[0]], &lines[b.lines[0]]);
        (x.stream, x.bucket)
            .cmp(&(y.stream, y.bucket))
            .then(y.n.total_cmp(&x.n))
            .then(x.t0.total_cmp(&y.t0))
    });
    blocks
}

impl Layout {
    /// Computes the lines and blocks of an interpreted page.
    pub fn build(state: &PageState) -> Layout {
        let lines = build_lines(state);
        let blocks = build_blocks(&lines);
        Layout { lines, blocks }
    }

    /// The line that contains the run with object index `index`.
    pub fn line_of_run(&self, index: usize) -> Option<usize> {
        self.lines
            .iter()
            .position(|l| l.runs.iter().any(|r| r.index as usize == index))
    }

    /// Converts the layout to its public form.
    pub fn to_public(&self, state: &PageState) -> PageLayout {
        let id = |index: u32| {
            state
                .objects
                .objects
                .get(index as usize)
                .map_or(0, |o| o.id)
        };
        let public_line = |l: &LineGeom| TextLine {
            id: l.runs.first().map_or(0, |r| id(r.index)),
            runs: l.runs.iter().map(|r| id(r.index)).collect(),
            text: l.text.clone(),
            bbox: l.bbox,
            origin: l.point(l.t0),
            angle: l.u.1.atan2(l.u.0).to_degrees(),
            size: l.size,
        };
        let blocks = self
            .blocks
            .iter()
            .map(|b| {
                let lines: Vec<TextLine> = b
                    .lines
                    .iter()
                    .map(|i| public_line(&self.lines[*i]))
                    .collect();
                let bbox = lines
                    .iter()
                    .skip(1)
                    .fold(lines[0].bbox, |acc, l| acc.union(&l.bbox));
                let mut text = String::new();
                for (k, li) in b.lines.iter().enumerate() {
                    text.push_str(&self.lines[*li].text);
                    if let Some(next) = b.lines.get(k + 1) {
                        match join(&self.lines[*li], &self.lines[*next], b.align) {
                            Join::Space => text.push(' '),
                            Join::Nothing => {}
                            Join::Break => text.push('\n'),
                        }
                    }
                }
                TextBlock {
                    id: lines[0].id,
                    bbox,
                    align: b.align,
                    text,
                    lines,
                }
            })
            .collect();
        PageLayout {
            page: state.objects.page,
            blocks,
        }
    }
}

impl crate::Document {
    /// Returns the text of a page grouped into blocks and lines, in reading order.
    pub fn page_layout(&mut self, page: u32) -> crate::Result<PageLayout> {
        let state = self.state(page)?;
        Ok(state.layout().to_public(state))
    }
}
