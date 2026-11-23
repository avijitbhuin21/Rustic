//! Edits: splicing content streams, undo and redo, and saving.

use std::collections::HashSet;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream};

use crate::content::{lex, put_matrix_op, put_name, put_num, put_nums, Tok, K};
use crate::doc::{guarded, Anchor, Document, PageState, Step};
use crate::geom::{Matrix, Rect};
use crate::interp::{RawKind, RawText, StreamKey};
use crate::model::ObjId;
use crate::util::deflate;
use crate::{Error, Result};

/// What happens to the object whose operation a splice replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fate {
    /// The object survives; its operation now starts this many bytes into the replacement.
    Keep(u32),
    /// The object is gone.
    Remove,
    /// The object survives elsewhere: its operation starts at this offset of the new stream.
    Absolute(u32),
}

/// Replaces `start..end` of a stream with `bytes`.
#[derive(Debug)]
pub(crate) struct Splice {
    pub start: u32,
    pub end: u32,
    pub bytes: Vec<u8>,
    pub fate: Fate,
}

/// Applies non-overlapping splices to `src`.
fn spliced(src: &[u8], splices: &[Splice]) -> Vec<u8> {
    let grow: usize = splices.iter().map(|s| s.bytes.len()).sum();
    let mut out = Vec::with_capacity(src.len() + grow);
    let mut at = 0usize;
    for s in splices {
        let (start, end) = (
            (s.start as usize).clamp(at, src.len()),
            (s.end as usize).clamp(at, src.len()),
        );
        out.extend_from_slice(&src[at..start]);
        out.extend_from_slice(&s.bytes);
        at = end.max(start);
    }
    out.extend_from_slice(&src[at..]);
    out
}

/// Moves the anchors of one stream to where their operations are after the splices.
fn shift_anchors(anchors: &mut Vec<Anchor>, key: StreamKey, splices: &[Splice]) {
    anchors.retain_mut(|a| {
        if a.key != key {
            return true;
        }
        let mut delta = 0i64;
        for s in splices {
            if s.end <= a.start {
                delta += s.bytes.len() as i64 - i64::from(s.end - s.start);
            } else if s.start <= a.start && a.start < s.end {
                return match s.fate {
                    Fate::Remove => false,
                    Fate::Absolute(at) => {
                        a.start = at;
                        true
                    }
                    Fate::Keep(offset) => {
                        a.start = (i64::from(s.start) + delta + i64::from(offset)).max(0) as u32;
                        true
                    }
                };
            } else {
                break;
            }
        }
        a.start = (i64::from(a.start) + delta).max(0) as u32;
        true
    });
}

impl Document {
    /// Starts an undoable step.
    pub(crate) fn begin(&self, label: &'static str, page: u32) -> Step {
        Step {
            label,
            page,
            objects: Vec::new(),
            anchors: Vec::new(),
        }
    }

    /// Remembers an object's current value before it is changed.
    pub(crate) fn record(&self, step: &mut Step, id: ObjectId) {
        if !step.objects.iter().any(|(seen, _)| *seen == id) {
            step.objects.push((id, self.pdf.objects.get(&id).cloned()));
        }
    }

    /// Adds a new object and remembers that it did not exist before.
    pub(crate) fn add_object(&mut self, step: &mut Step, object: Object) -> ObjectId {
        let id = self.pdf.add_object(object);
        step.objects.push((id, None));
        id
    }

    /// Finishes a step: drops caches and makes it undoable.
    pub(crate) fn commit(&mut self, step: Step) {
        self.cache.clear();
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > self.history_limit {
            let extra = self.undo.len() - self.history_limit;
            self.undo.drain(..extra);
        }
    }

    /// Rewrites one stream of a page and keeps object ids pointing at the right operations.
    pub(crate) fn splice(
        &mut self,
        step: &mut Step,
        page: u32,
        key: StreamKey,
        mut splices: Vec<Splice>,
    ) -> Result<()> {
        splices.sort_by_key(|s| (s.start, s.end));
        let state = self.state(page)?;
        let stream = state.interp.streams.iter().find(|s| s.key == key).cloned();
        let stream = stream.ok_or_else(|| {
            Error::Unsupported("the object's content stream is not available".into())
        })?;
        let bytes = spliced(&stream.bytes, &splices);
        match key {
            StreamKey::Page => {
                let page_id = self.page_id(page)?;
                let contents = self.pdf.get_page_contents(page_id);
                match contents.as_slice() {
                    [only] if self.owned.contains(only) => {
                        self.record(step, *only);
                        if let Some(Object::Stream(s)) = self.pdf.objects.get_mut(only) {
                            s.set_plain_content(bytes);
                        }
                        self.plain.insert(*only);
                    }
                    _ => {
                        let id = self.add_object(
                            step,
                            Object::Stream(Stream::new(Dictionary::new(), bytes)),
                        );
                        self.record(step, page_id);
                        let dict =
                            self.pdf
                                .get_dictionary_mut(page_id)
                                .map_err(|e| Error::Content {
                                    page,
                                    reason: e.to_string(),
                                })?;
                        dict.set("Contents", Object::Reference(id));
                        self.owned.insert(id);
                        self.plain.insert(id);
                    }
                }
            }
            StreamKey::Form(id) => {
                self.record(step, id);
                match self.pdf.objects.get_mut(&id) {
                    Some(Object::Stream(s)) => s.set_plain_content(bytes),
                    _ => {
                        return Err(Error::Unsupported(
                            "the form XObject is no longer a stream".into(),
                        ))
                    }
                }
                self.plain.insert(id);
            }
        }
        let pages: Vec<u32> = match key {
            StreamKey::Page => vec![page],
            StreamKey::Form(_) => self.anchors.keys().copied().collect(),
        };
        for p in pages {
            if !step.anchors.iter().any(|(seen, _)| *seen == p) {
                step.anchors.push((p, self.anchors.get(&p).cloned()));
            }
            if let Some(list) = self.anchors.get_mut(&p) {
                shift_anchors(list, key, &splices);
            }
        }
        // Later splices in the same step must see the new bytes.
        self.cache.clear();
        Ok(())
    }

    /// Swaps the document with the state stored in `step`, returning the inverse step.
    fn swap(&mut self, step: Step) -> Step {
        let mut inverse = Step {
            label: step.label,
            page: step.page,
            objects: Vec::new(),
            anchors: Vec::new(),
        };
        for (id, old) in step.objects.into_iter().rev() {
            let current = match old {
                Some(object) => self.pdf.objects.insert(id, object),
                None => self.pdf.objects.remove(&id),
            };
            inverse.objects.push((id, current));
        }
        for (page, old) in step.anchors.into_iter().rev() {
            let current = match old {
                Some(list) => self.anchors.insert(page, list),
                None => self.anchors.remove(&page),
            };
            inverse.anchors.push((page, current));
        }
        self.cache.clear();
        if step.label == "replace text" {
            // The step may have rewritten an embedded fallback font in place.
            self.fonts.by_id.clear();
        }
        self.fonts
            .by_id
            .retain(|id, _| self.pdf.objects.contains_key(id));
        inverse
    }

    /// Undoes a step that failed partway, without making it redoable.
    pub(crate) fn rollback(&mut self, step: Step) {
        self.swap(step);
    }

    /// Reverts the most recent edit. Returns the page it touched, or `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<u32> {
        let step = self.undo.pop()?;
        let page = step.page;
        let inverse = self.swap(step);
        self.redo.push(inverse);
        Some(page)
    }

    /// Re-applies the most recently undone edit. Returns the page it touched.
    pub fn redo(&mut self) -> Option<u32> {
        let step = self.redo.pop()?;
        let page = step.page;
        let inverse = self.swap(step);
        self.undo.push(inverse);
        Some(page)
    }

    /// The labels of the edits that can be undone, oldest first.
    pub fn history(&self) -> Vec<&'static str> {
        self.undo.iter().map(|s| s.label).collect()
    }

    /// Sets how many edits are kept for undo.
    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        let extra = self.undo.len().saturating_sub(limit);
        self.undo.drain(..extra);
    }
}

/// Collects every object reachable from the trailer.
fn reachable(pdf: &Pdf) -> HashSet<ObjectId> {
    fn walk(object: &Object, seen: &mut HashSet<ObjectId>, queue: &mut Vec<ObjectId>) {
        match object {
            Object::Reference(id) => {
                if seen.insert(*id) {
                    queue.push(*id);
                }
            }
            Object::Array(items) => items.iter().for_each(|o| walk(o, seen, queue)),
            Object::Dictionary(dict) => dict.iter().for_each(|(_, o)| walk(o, seen, queue)),
            Object::Stream(stream) => stream.dict.iter().for_each(|(_, o)| walk(o, seen, queue)),
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    let mut queue = Vec::new();
    pdf.trailer
        .iter()
        .for_each(|(_, o)| walk(o, &mut seen, &mut queue));
    while let Some(id) = queue.pop() {
        if let Some(object) = pdf.objects.get(&id) {
            walk(object, &mut seen, &mut queue);
        }
    }
    seen
}

impl Document {
    /// Writes the document as a complete new file, leaving out objects nothing refers to.
    ///
    /// The document stays usable afterwards, including undo. An encrypted input is
    /// written without encryption, and any digital signature no longer validates.
    pub fn save(&mut self) -> Result<Vec<u8>> {
        for id in std::mem::take(&mut self.plain) {
            if let Some(Object::Stream(s)) = self.pdf.objects.get_mut(&id) {
                if s.dict.has(b"Filter") {
                    continue;
                }
                let packed = deflate(&s.content);
                if !packed.is_empty() && packed.len() + 20 < s.content.len() {
                    s.set_content(packed);
                    s.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
                }
            }
        }
        let live = reachable(&self.pdf);
        let all = std::mem::take(&mut self.pdf.objects);
        let (keep, unused): (
            std::collections::BTreeMap<_, _>,
            std::collections::BTreeMap<_, _>,
        ) = all.into_iter().partition(|(id, _)| live.contains(id));
        let size_hint: usize = keep
            .values()
            .map(|o| {
                if let Object::Stream(s) = o {
                    s.content.len() + 64
                } else {
                    48
                }
            })
            .sum();
        self.pdf.objects = keep;
        // lopdf takes a fresh object id for the cross-reference stream on each save.
        let max_id = self.pdf.max_id;
        let pdf = &mut self.pdf;
        let written = guarded("writing", || {
            let mut out = Vec::with_capacity(size_hint);
            pdf.save_to(&mut out)
                .map_err(|e| Error::Save(e.to_string()))?;
            Ok(out)
        });
        self.pdf.max_id = max_id;
        // Unused objects stay in memory so that undo can bring their references back.
        self.pdf.objects.extend(unused);
        written
    }

    /// Looks up the stream, byte range and details of an object.
    fn target(&mut self, page: u32, id: ObjId) -> Result<Target> {
        let index = self.locate(page, id)?;
        let state = self.state(page)?;
        let item = state
            .interp
            .items
            .get(index)
            .ok_or(Error::NoSuchObject { page, id })?;
        let stream = state
            .interp
            .streams
            .get(item.stream as usize)
            .ok_or(Error::NoSuchObject { page, id })?;
        let op = stream
            .bytes
            .get(item.start as usize..item.end as usize)
            .unwrap_or(&[])
            .to_vec();
        let text = match &item.kind {
            RawKind::Text(t) => Some(TextTarget::of(t)),
            RawKind::Image(_) => None,
        };
        Ok(Target {
            index,
            key: stream.key,
            start: item.start,
            end: item.end,
            op,
            to_page: item.ctm.then(&state.to_page),
            text,
        })
    }

    /// Removes a text run or an image from the page.
    pub fn delete(&mut self, page: u32, id: ObjId) -> Result<()> {
        let t = self.target(page, id)?;
        let mut bytes = vec![b' '];
        if let Some(text) = &t.text {
            text.put_removal(&mut bytes);
        }
        let mut step = self.begin("delete", page);
        self.splice(
            &mut step,
            page,
            t.key,
            vec![Splice {
                start: t.start,
                end: t.end,
                bytes,
                fate: Fate::Remove,
            }],
        )?;
        self.commit(step);
        Ok(())
    }

    /// Applies an affine transform, given in page space, to a text run or an image.
    ///
    /// When the object would leave a clipping path that surrounds it, it is lifted out
    /// of that clip group and painted right after it; see [`Placement`].
    pub fn transform(&mut self, page: u32, id: ObjId, transform: &Matrix) -> Result<Placement> {
        let t = self.target(page, id)?;
        let singular = || Error::Unsupported("the object has no area to transform".into());
        if !transform.is_finite() || transform.invert().is_none() {
            return Err(Error::Unsupported("the transform is not invertible".into()));
        }
        // `local` acts in the operation's own user space and has the same effect on the page.
        let local = t
            .to_page
            .then(transform)
            .then(&t.to_page.invert().ok_or_else(singular)?);
        let lift = plan_lift(self.state(page)?, t.index, transform, &local);
        if let Lift::Out { at, bytes, offset } = lift {
            let mut removal = vec![b' '];
            if let Some(text) = &t.text {
                text.put_removal(&mut removal);
            }
            let shift = removal.len() as i64 - i64::from(t.end - t.start);
            let landed = (i64::from(at) + shift + offset as i64).max(0) as u32;
            let splices = vec![
                Splice {
                    start: t.start,
                    end: t.end,
                    bytes: removal,
                    fate: Fate::Absolute(landed),
                },
                Splice {
                    start: at,
                    end: at,
                    bytes,
                    fate: Fate::Keep(0),
                },
            ];
            let mut step = self.begin("transform", page);
            self.splice(&mut step, page, t.key, splices)?;
            self.commit(step);
            return Ok(Placement {
                lifted: true,
                clipped: false,
            });
        }
        let clipped = matches!(lift, Lift::Clipped);
        let mut bytes = Vec::with_capacity(t.op.len() + 96);
        let offset;
        match &t.text {
            None => {
                bytes.extend_from_slice(b" q ");
                put_matrix_op(&mut bytes, &local, "cm");
                offset = bytes.len();
                bytes.extend_from_slice(&t.op);
                bytes.extend_from_slice(b" Q ");
            }
            Some(text) => {
                bytes.push(b' ');
                let pure_move = transform.0[..4] == [1.0, 0.0, 0.0, 1.0];
                let to_text = text
                    .tm
                    .then(&t.to_page)
                    .invert()
                    .filter(|_| pure_move && (text.size * text.th).abs() > 1e-9);
                if let Some(to_text) = to_text {
                    // A plain move leaves the text matrix alone: the run is shifted along its
                    // baseline by a glyph-less TJ and across it by text rise, and both are
                    // undone afterwards. Later lines are then positioned exactly as before.
                    let (vx, vy) = to_text.apply_vec(transform.0[4], transform.0[5]);
                    text.put_line_move(&mut bytes);
                    text.put_advance(&mut bytes, vx);
                    if vy != 0.0 {
                        put_nums(&mut bytes, &[text.rise + vy]);
                        bytes.extend_from_slice(b"Ts ");
                    }
                    offset = bytes.len();
                    if matches!(text.show, K::Quote | K::DQuote) {
                        put_show(&mut bytes, &t.op, text.show)?;
                    } else {
                        bytes.extend_from_slice(&t.op);
                    }
                    bytes.push(b' ');
                    if vy != 0.0 {
                        put_nums(&mut bytes, &[text.rise]);
                        bytes.extend_from_slice(b"Ts ");
                    }
                    if text.followed {
                        text.put_advance(&mut bytes, -vx);
                    }
                } else {
                    if text.show == K::DQuote {
                        put_num(&mut bytes, text.tw);
                        bytes.extend_from_slice(b" Tw ");
                        put_num(&mut bytes, text.tc);
                        bytes.extend_from_slice(b" Tc ");
                    }
                    put_matrix_op(&mut bytes, &text.tm.then(&local), "Tm");
                    offset = bytes.len();
                    if matches!(text.show, K::Quote | K::DQuote) {
                        put_show(&mut bytes, &t.op, text.show)?;
                    } else {
                        bytes.extend_from_slice(&t.op);
                    }
                    bytes.push(b' ');
                    put_matrix_op(&mut bytes, &text.tlm_after, "Tm");
                    if text.followed {
                        // Put the text matrix back where the following show expects it.
                        let end = Matrix::translate(text.advance, 0.0).then(&text.tm);
                        let [a, b, _, _, e, f] = text.tlm_after.0;
                        let norm = a * a + b * b;
                        if norm > 1e-18 {
                            text.put_advance(
                                &mut bytes,
                                ((end.0[4] - e) * a + (end.0[5] - f) * b) / norm,
                            );
                        }
                    }
                }
            }
        }
        let mut step = self.begin("transform", page);
        let splice = Splice {
            start: t.start,
            end: t.end,
            bytes,
            fate: Fate::Keep(offset as u32),
        };
        self.splice(&mut step, page, t.key, vec![splice])?;
        self.commit(step);
        Ok(Placement {
            lifted: false,
            clipped,
        })
    }

    /// Moves an object by `(dx, dy)` points in page space.
    pub fn translate(&mut self, page: u32, id: ObjId, dx: f64, dy: f64) -> Result<Placement> {
        self.transform(page, id, &Matrix::translate(dx, dy))
    }

    /// Moves and scales an object so that its bounding box becomes `target`.
    pub fn fit(&mut self, page: u32, id: ObjId, target: Rect) -> Result<Placement> {
        let index = self.locate(page, id)?;
        let bbox = self.state(page)?.objects.objects[index].bbox;
        let m = Matrix::rect_to_rect(&bbox, &target).filter(|m| m.invert().is_some());
        self.transform(
            page,
            id,
            &m.ok_or_else(|| Error::Unsupported("the object or the target box is empty".into()))?,
        )
    }
}

/// How a transformed object ended up relative to the clipping paths around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    /// The object left a clipping path and was moved out of that clip group. It is now
    /// painted after the rest of the group, which can change what overlaps what.
    pub lifted: bool,
    /// The object now extends past a clipping path it could not be lifted out of (a
    /// crop, a form's bounding box, or a page-level clip), so part of it is hidden.
    pub clipped: bool,
}

/// Whether a transformed object has to leave its clip group.
enum Lift {
    No,
    Clipped,
    /// Insert `bytes` at stream offset `at`; the object's operation starts `offset` bytes in.
    Out {
        at: u32,
        bytes: Vec<u8>,
        offset: usize,
    },
}

/// Writes the string or array of a show operation followed by `Tj` or `TJ`.
fn put_show(out: &mut Vec<u8>, op: &[u8], show: K) -> Result<()> {
    let lexed = lex(op, 4);
    let unreadable = || Error::Unsupported("the text operation could not be read".into());
    if show == K::TJ {
        let name = lexed.ops.first().ok_or_else(unreadable)?.name;
        out.extend_from_slice(op.get(..name.0 as usize).unwrap_or(&[]).trim_ascii_end());
        out.extend_from_slice(b" TJ");
    } else {
        let string = lexed.toks.iter().find_map(|tok| {
            if let Tok::Str(sp, _) = tok {
                Some(*sp)
            } else {
                None
            }
        });
        let (a, b) = string.ok_or_else(unreadable)?;
        out.extend_from_slice(op.get(a as usize..b as usize).unwrap_or(&[]));
        out.extend_from_slice(b" Tj");
    }
    Ok(())
}

/// Decides whether the object at `index` must leave a clip group, and builds its new operation.
fn plan_lift(state: &PageState, index: usize, transform: &Matrix, local: &Matrix) -> Lift {
    let (Some(item), Some(object)) = (
        state.interp.items.get(index),
        state.objects.objects.get(index),
    ) else {
        return Lift::No;
    };
    let (Some(stream), Some(to_user)) = (
        state.interp.streams.get(item.stream as usize),
        state.to_page.invert(),
    ) else {
        return Lift::No;
    };
    // The part of the object that must be visible for it to count as shown: the whole
    // quad of an image, and for text the band from the baseline to about x-height. Font
    // boxes are taller than the ink, and producers such as Word clip cells tighter than that.
    let core = match &item.kind {
        RawKind::Text(t) => {
            let span = t.font.ascent - t.font.descent;
            let (lo, hi) = if span > 1e-6 {
                (
                    (-t.font.descent / span).clamp(0.0, 1.0),
                    ((0.45 - t.font.descent) / span).clamp(0.0, 1.0),
                )
            } else {
                (0.0, 1.0)
            };
            let mix = |a: (f64, f64), b: (f64, f64), t: f64| {
                (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
            };
            let q = object.quad;
            [
                mix(q[0], q[3], lo),
                mix(q[1], q[2], lo),
                mix(q[1], q[2], hi),
                mix(q[0], q[3], hi),
            ]
        }
        RawKind::Image(_) => object.quad,
    };
    let bound = |quad: &[(f64, f64); 4], m: &Matrix| {
        let (x, y) = m.apply(quad[0].0, quad[0].1);
        let mut r = Rect::new(x, y, x, y);
        quad[1..].iter().for_each(|p| {
            let (x, y) = m.apply(p.0, p.1);
            r.add_point(x, y);
        });
        r
    };
    let (now, next) = (
        bound(&object.quad, &to_user),
        bound(&object.quad, &transform.then(&to_user)),
    );
    let core_now = bound(&core, &to_user);
    // How far a box sticks out of a clip on each side.
    let over = |clip: &Rect, b: &Rect| {
        [
            clip.x0 - b.x0,
            clip.y0 - b.y0,
            b.x1 - clip.x1,
            b.y1 - clip.y1,
        ]
        .map(|v| v.max(0.0))
    };
    let mut exit = None;
    let mut node = item.clip.as_deref();
    while let Some(n) = node {
        let (was, will) = (over(&n.rect, &now), over(&n.rect, &next));
        if was.iter().zip(&will).any(|(a, b)| *b > *a + 0.5) {
            // The clip would hide more of the object than it does now. Leave the clip's
            // group, unless it cannot be left or it already hides part of the object.
            if n.group.is_none() || !n.rect.contains(&core_now, 0.5) {
                return Lift::Clipped;
            }
            exit = n.group;
        }
        node = n.parent.as_deref();
    }
    let Some((group, outer)) = exit else {
        return Lift::No;
    };
    let Some(outer_inv) = outer.invert() else {
        return Lift::Clipped;
    };
    let mut depth = 0usize;
    let mut at = stream.bytes.len() as u32;
    for op in stream.lexed.ops.iter().skip(group as usize) {
        match op.k {
            K::Save => depth += 1,
            K::Restore => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    at = op.end;
                    break;
                }
            }
            _ => {}
        }
    }
    let op = stream
        .bytes
        .get(item.start as usize..item.end as usize)
        .unwrap_or(&[]);
    let mut bytes = Vec::with_capacity(op.len() + 160);
    bytes.extend_from_slice(b" q ");
    put_matrix_op(&mut bytes, &local.then(&item.ctm).then(&outer_inv), "cm");
    for &(s, a, b) in item.state.iter().flatten() {
        if s == item.stream {
            bytes.extend_from_slice(stream.bytes.get(a as usize..b as usize).unwrap_or(&[]));
            bytes.push(b' ');
        }
    }
    let offset;
    match &item.kind {
        RawKind::Image(_) => {
            offset = bytes.len();
            bytes.extend_from_slice(op);
        }
        RawKind::Text(t) => {
            bytes.extend_from_slice(b"BT ");
            put_name(&mut bytes, &t.font_name);
            bytes.push(b' ');
            put_nums(&mut bytes, &[t.size]);
            bytes.extend_from_slice(b"Tf ");
            for (value, default, name) in [
                (t.tc, 0.0, "Tc"),
                (t.tw, 0.0, "Tw"),
                (t.th * 100.0, 100.0, "Tz"),
                (t.rise, 0.0, "Ts"),
            ] {
                if value != default {
                    put_nums(&mut bytes, &[value]);
                    bytes.extend_from_slice(name.as_bytes());
                    bytes.push(b' ');
                }
            }
            if t.render != 0 {
                bytes.extend_from_slice(format!("{} Tr ", t.render).as_bytes());
            }
            put_matrix_op(&mut bytes, &t.tm, "Tm");
            offset = bytes.len();
            if put_show(&mut bytes, op, t.show).is_err() {
                return Lift::Clipped;
            }
            bytes.extend_from_slice(b" ET");
        }
    }
    bytes.extend_from_slice(b" Q ");
    Lift::Out { at, bytes, offset }
}

/// Where an object's operation lives and what it needs to be rewritten.
struct Target {
    index: usize,
    key: StreamKey,
    start: u32,
    end: u32,
    op: Vec<u8>,
    /// Maps the operation's user space to page space.
    to_page: Matrix,
    text: Option<TextTarget>,
}

/// The text state a run was shown with.
struct TextTarget {
    show: K,
    tm: Matrix,
    tlm_after: Matrix,
    advance: f64,
    size: f64,
    th: f64,
    tc: f64,
    tw: f64,
    rise: f64,
    followed: bool,
}

impl TextTarget {
    /// Copies the state needed to rewrite a run's operation.
    fn of(t: &RawText) -> Self {
        TextTarget {
            show: t.show,
            tm: t.tm,
            tlm_after: t.tlm_after,
            advance: t.advance,
            size: t.size,
            th: t.th,
            tc: t.tc,
            tw: t.tw,
            rise: t.rise,
            followed: t.followed,
        }
    }

    /// Writes the line move and spacing that `'` and `"` perform before showing.
    fn put_line_move(&self, out: &mut Vec<u8>) {
        match self.show {
            K::Quote => out.extend_from_slice(b"T* "),
            K::DQuote => {
                put_num(out, self.tw);
                out.extend_from_slice(b" Tw ");
                put_num(out, self.tc);
                out.extend_from_slice(b" Tc T* ");
            }
            _ => {}
        }
    }

    /// Writes what must stay behind when the run's operation is taken out: the line
    /// move and spacing of `'` and `"`, and the advance a following show relies on.
    fn put_removal(&self, out: &mut Vec<u8>) {
        self.put_line_move(out);
        if self.followed {
            self.put_advance(out, self.advance);
        }
    }

    /// Writes a glyph-less `TJ` that advances the text matrix by `distance` text-space units.
    fn put_advance(&self, out: &mut Vec<u8>, distance: f64) {
        let unit = self.size * self.th;
        if unit.abs() > 1e-9 && distance.is_finite() && distance != 0.0 {
            out.push(b'[');
            put_num(out, -distance * 1000.0 / unit);
            out.extend_from_slice(b"] TJ ");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::PageObject;

    /// `(id, text or "<image>", bbox origin rounded to 0.01)` for every object on page 1.
    fn snapshot(doc: &mut Document) -> Vec<(ObjId, String, (i64, i64))> {
        let label = |o: &PageObject| {
            o.as_text()
                .map_or("<image>".to_string(), |t| t.text.clone())
        };
        let at = |o: &PageObject| {
            (
                (o.bbox.x0 * 100.0).round() as i64,
                (o.bbox.y0 * 100.0).round() as i64,
            )
        };
        doc.page_objects(1)
            .unwrap()
            .objects
            .iter()
            .map(|o| (o.id, label(o), at(o)))
            .collect()
    }

    fn id_of(doc: &mut Document, text: &str) -> ObjId {
        snapshot(doc).iter().find(|(_, t, _)| t == text).unwrap().0
    }

    #[test]
    fn reads_the_rich_fixture() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let labels: Vec<String> = snapshot(&mut doc).into_iter().map(|(_, t, _)| t).collect();
        assert_eq!(
            labels,
            [
                "<image>",
                "Alpha ",
                "Beta",
                "Gamma",
                "Delta",
                "<image>",
                "In a form",
                "<image>"
            ]
        );
        let page = doc.page_objects(1).unwrap();
        assert_eq!(page.objects[0].bbox, Rect::new(50.0, 600.0, 150.0, 680.0));
        assert!(page.objects[6].in_form && !page.objects[5].in_form);
        assert_eq!(page.objects[6].as_text().unwrap().origin, (305.0, 405.0));
        assert!(page.objects[7].as_image().unwrap().inline);
        assert_eq!(page.objects[7].bbox, Rect::new(400.0, 100.0, 420.0, 120.0));
    }

    #[test]
    fn deleting_a_run_leaves_its_neighbours_in_place() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let before = snapshot(&mut doc);
        for victim in ["Alpha ", "Gamma", "Delta"] {
            let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
            let id = id_of(&mut doc, victim);
            doc.delete(1, id).unwrap();
            let expected: Vec<_> = before
                .iter()
                .filter(|(_, t, _)| t != victim)
                .cloned()
                .collect();
            assert_eq!(snapshot(&mut doc), expected, "after deleting {victim:?}");
            // The same holds after a save and reload, apart from ids being reassigned.
            let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
            let strip = |v: Vec<(ObjId, String, (i64, i64))>| {
                v.into_iter().map(|(_, t, p)| (t, p)).collect::<Vec<_>>()
            };
            assert_eq!(
                strip(snapshot(&mut again)),
                strip(expected),
                "reloaded after deleting {victim:?}"
            );
        }
    }

    #[test]
    fn moving_objects_keeps_ids_and_everything_else() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let before = snapshot(&mut doc);
        let moves = [
            ("Beta", 1usize),
            ("Delta", 4),
            ("<image>", 0),
            ("In a form", 6),
            ("Alpha ", 1),
        ];
        let mut expected = before.clone();
        for (text, _) in moves {
            let index = expected.iter().position(|(_, t, _)| t == text).unwrap();
            doc.translate(1, expected[index].0, 12.5, -7.25).unwrap();
            expected[index].2 .0 += 1250;
            expected[index].2 .1 -= 725;
            assert_eq!(snapshot(&mut doc), expected, "after moving {text:?}");
        }
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        let strip = |v: Vec<(ObjId, String, (i64, i64))>| {
            v.into_iter().map(|(_, t, p)| (t, p)).collect::<Vec<_>>()
        };
        assert_eq!(strip(snapshot(&mut again)), strip(expected));
    }

    #[test]
    fn fits_an_image_into_a_box() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let ids: Vec<ObjId> = snapshot(&mut doc)
            .iter()
            .filter(|(_, t, _)| t == "<image>")
            .map(|o| o.0)
            .collect();
        let target = Rect::new(10.0, 20.0, 110.0, 70.0);
        doc.fit(1, ids[2], target).unwrap();
        doc.fit(1, ids[0], Rect::new(200.0, 200.0, 260.0, 230.0))
            .unwrap();
        let page = doc.page_objects(1).unwrap();
        let bbox = |id| page.objects.iter().find(|o| o.id == id).unwrap().bbox;
        let close = |a: Rect, b: Rect| {
            (a.x0 - b.x0).abs() + (a.y0 - b.y0).abs() + (a.x1 - b.x1).abs() + (a.y1 - b.y1).abs()
                < 1e-4
        };
        assert!(close(bbox(ids[2]), target), "{:?}", bbox(ids[2]));
        assert!(close(bbox(ids[0]), Rect::new(200.0, 200.0, 260.0, 230.0)));
        assert_eq!(bbox(ids[1]), Rect::new(300.0, 600.0, 340.0, 640.0));
    }

    #[test]
    fn scales_a_text_run_about_a_point() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let before = snapshot(&mut doc);
        let beta = id_of(&mut doc, "Beta");
        let old = doc
            .page_objects(1)
            .unwrap()
            .objects
            .iter()
            .find(|o| o.id == beta)
            .unwrap()
            .clone();
        let (ox, oy) = old.as_text().unwrap().origin;
        doc.transform(1, beta, &Matrix::scale_about(2.0, 2.0, ox, oy))
            .unwrap();
        let page = doc.page_objects(1).unwrap();
        let new = page.objects.iter().find(|o| o.id == beta).unwrap();
        let run = new.as_text().unwrap();
        assert_eq!(run.size, 24.0);
        assert!((run.origin.0 - ox).abs() < 1e-9 && (run.origin.1 - oy).abs() < 1e-9);
        assert!((new.bbox.width() - 2.0 * old.bbox.width()).abs() < 1e-6);
        // Everything else, including the runs that follow in the same text object, is unmoved.
        let after = snapshot(&mut doc);
        for (a, b) in before.iter().zip(&after).filter(|(a, _)| a.0 != beta) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn undo_and_redo_restore_exact_states() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let s0 = snapshot(&mut doc);
        let beta = id_of(&mut doc, "Beta");
        doc.translate(1, beta, 30.0, 0.0).unwrap();
        let s1 = snapshot(&mut doc);
        let image = s0[0].0;
        doc.delete(1, image).unwrap();
        let s2 = snapshot(&mut doc);
        assert_eq!(doc.history(), ["transform", "delete"]);
        assert_eq!(doc.undo(), Some(1));
        assert_eq!(snapshot(&mut doc), s1);
        assert_eq!(doc.undo(), Some(1));
        assert_eq!(snapshot(&mut doc), s0);
        assert_eq!(doc.undo(), None);
        assert_eq!(doc.redo(), Some(1));
        assert_eq!(doc.redo(), Some(1));
        assert_eq!(snapshot(&mut doc), s2);
        assert_eq!(doc.redo(), None);
        // A new edit after an undo discards the redo branch.
        doc.undo();
        doc.delete(1, beta).unwrap();
        assert_eq!(doc.redo(), None);
        assert!(matches!(
            doc.delete(1, beta),
            Err(Error::NoSuchObject { .. })
        ));
        assert!(matches!(doc.delete(9, beta), Err(Error::NoSuchPage(9))));
    }

    #[test]
    fn lifts_an_object_that_leaves_its_clip() {
        let mut doc = Document::from_bytes(&fixtures::clipped()).unwrap();
        let before = snapshot(&mut doc);
        assert_eq!(
            before.iter().map(|o| o.1.as_str()).collect::<Vec<_>>(),
            ["<image>", "Clipped", " next", "Free"]
        );
        // A move that stays inside the clip rectangle edits in place.
        let clipped = id_of(&mut doc, "Clipped");
        assert_eq!(
            doc.translate(1, clipped, 2.0, 1.0).unwrap(),
            Placement {
                lifted: false,
                clipped: false
            }
        );
        doc.undo();
        // Moves that leave the clip lift the object out of its group; ids and neighbours hold.
        let mut expected = before.clone();
        for (index, dx, dy) in [(0usize, 200.0, 0.0), (1, 0.0, -100.0)] {
            let placement = doc.translate(1, expected[index].0, dx, dy).unwrap();
            assert_eq!(
                placement,
                Placement {
                    lifted: true,
                    clipped: false
                }
            );
            expected[index].2 .0 += (dx * 100.0) as i64;
            expected[index].2 .1 += (dy * 100.0) as i64;
            let mut got = snapshot(&mut doc);
            got.sort_by_key(|o| o.0);
            assert_eq!(got, expected);
        }
        // The lifted run keeps its colour and font, and is no longer inside a clip.
        let page = doc.page_objects(1).unwrap();
        let run = page
            .objects
            .iter()
            .find(|o| o.id == clipped)
            .unwrap()
            .as_text()
            .unwrap();
        assert_eq!(
            (run.color, run.size, run.font.as_str()),
            (Some([0.0, 0.0, 1.0]), 12.0, "Helvetica")
        );
        let state = doc.state(1).unwrap();
        let index = state
            .objects
            .objects
            .iter()
            .position(|o| o.id == clipped)
            .unwrap();
        assert!(state.interp.items[index].clip.is_none());
        // A lifted object can be moved again and survives a reload.
        doc.translate(1, clipped, 5.0, 5.0).unwrap();
        let mut again = Document::from_bytes(&doc.save().unwrap()).unwrap();
        let texts: Vec<String> = snapshot(&mut again).into_iter().map(|o| o.1).collect();
        assert_eq!(texts, ["<image>", " next", "Clipped", "Free"]);
        while doc.undo().is_some() {}
        assert_eq!(snapshot(&mut doc), before);
    }

    #[test]
    fn treats_a_clip_tighter_than_the_font_box_as_leavable() {
        let mut doc = Document::from_bytes(&fixtures::tight_clip()).unwrap();
        let tight = id_of(&mut doc, "Tight");
        let halved = id_of(&mut doc, "Halved");
        // Sliding along the line keeps the same crop of ascenders: edit in place.
        assert_eq!(
            doc.translate(1, tight, 10.0, 0.0).unwrap(),
            Placement {
                lifted: false,
                clipped: false
            }
        );
        doc.undo();
        // Leaving the cell lifts the run, even though its font box already overhangs the clip.
        assert_eq!(
            doc.translate(1, tight, 0.0, -100.0).unwrap(),
            Placement {
                lifted: true,
                clipped: false
            }
        );
        let state = doc.state(1).unwrap();
        let index = state
            .objects
            .objects
            .iter()
            .position(|o| o.id == tight)
            .unwrap();
        assert!(state.interp.items[index].clip.is_none());
        // A run the clip really cuts stays in its clip, and the caller is told.
        assert_eq!(
            doc.translate(1, halved, 0.0, 50.0).unwrap(),
            Placement {
                lifted: false,
                clipped: true
            }
        );
    }

    #[test]
    fn ids_are_reproducible_when_edits_are_replayed() {
        // A host without a long-lived document replays its edits on every call.
        let replay = |edits: usize, peek: bool| {
            let mut doc = Document::from_bytes(&fixtures::clipped()).unwrap();
            if peek {
                doc.page_objects(1).unwrap();
            }
            let ids: Vec<ObjId> = doc
                .page_objects(1)
                .unwrap()
                .objects
                .iter()
                .map(|o| o.id)
                .collect();
            if edits > 0 {
                doc.translate(1, ids[1], 0.0, -100.0).unwrap();
            }
            if edits > 1 {
                doc.replace_line(
                    1,
                    ids[3],
                    "Free text, longer",
                    &crate::TextOptions::default(),
                )
                .unwrap();
            }
            if edits > 2 {
                doc.delete(1, ids[0]).unwrap();
            }
            snapshot(&mut doc)
        };
        for edits in 0..=3 {
            assert_eq!(
                replay(edits, false),
                replay(edits, true),
                "after {edits} edits"
            );
        }
        // Each call sees the ids the previous call handed out, plus or minus the edit.
        let (two, three) = (replay(2, false), replay(3, true));
        assert_eq!(two[1..], three[..]);
    }

    #[test]
    fn reports_a_clip_it_cannot_leave() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let in_form = id_of(&mut doc, "In a form");
        // The form's BBox is 200 x 50 at (300, 400); moving 500pt right leaves it.
        let placement = doc.translate(1, in_form, 500.0, 0.0).unwrap();
        assert_eq!(
            placement,
            Placement {
                lifted: false,
                clipped: true
            }
        );
    }

    #[test]
    fn save_drops_objects_nothing_refers_to() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let ids: Vec<ObjId> = snapshot(&mut doc)
            .iter()
            .filter(|(_, t, _)| t == "<image>")
            .map(|o| o.0)
            .collect();
        let full = doc.save().unwrap();
        doc.delete(1, ids[0]).unwrap();
        doc.delete(1, ids[1]).unwrap();
        let saved = doc.save().unwrap();
        let count = |bytes: &[u8]| Document::from_bytes(bytes).unwrap().pdf.objects.len();
        // The original content stream is replaced, not kept alongside the new one.
        assert_eq!(count(&saved), count(&full));
        assert_eq!(
            snapshot(&mut Document::from_bytes(&saved).unwrap()).len(),
            6
        );
        // Undo still works after saving, because unused objects stay in memory.
        doc.undo();
        doc.undo();
        assert_eq!(snapshot(&mut doc).len(), 8);
    }
}
