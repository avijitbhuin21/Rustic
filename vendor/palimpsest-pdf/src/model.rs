//! Public page object model and its JSON form.

use crate::geom::Rect;

/// Identifies an object on a page. Ids are unique per page and survive edits.
///
/// They are also reproducible: loading the same bytes and applying the same edits in
/// the same order gives the same ids, whichever pages were read in between. A host
/// that keeps no document in memory can therefore replay a list of edits on each call.
pub type ObjId = u32;

/// What kind of text, if any, a page carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageText {
    /// Visible text operators are present; the text can be edited.
    Text,
    /// Only invisible text (render mode 3), typically an OCR layer over a scan.
    HiddenText,
    /// No text operators; letters are drawn as vector outlines.
    Outlined,
    /// No text operators; the page is covered by an image.
    Scanned,
    /// No text operators and nothing that looks like outlined or scanned text.
    Empty,
}

impl PageText {
    /// A stable lower-case name, used in JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            PageText::Text => "text",
            PageText::HiddenText => "hidden_text",
            PageText::Outlined => "outlined",
            PageText::Scanned => "scanned",
            PageText::Empty => "empty",
        }
    }

    /// True when the host should offer text editing on this page.
    pub fn is_editable(&self) -> bool {
        matches!(self, PageText::Text)
    }
}

/// One glyph of a text run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphBox {
    /// Axis-aligned box in page space.
    pub bbox: Rect,
    /// Byte offset of the glyph's text within [`TextRun::text`].
    pub offset: u32,
}

/// A run of text painted by one show operator.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    /// Font name without the subset tag; empty when the font is unnamed.
    pub font: String,
    /// Font size in page-space points, after all matrices.
    pub size: f64,
    /// Fill colour as RGB in `0..=1`; `None` for patterns and unknown colour spaces.
    pub color: Option<[f32; 3]>,
    pub bold: bool,
    pub italic: bool,
    /// Render mode 3: the text is not painted.
    pub invisible: bool,
    /// Baseline start in page space.
    pub origin: (f64, f64),
    /// Baseline direction in degrees, counter-clockwise from the x axis.
    pub angle: f64,
    /// Characters with no Unicode mapping; they appear as U+FFFD in `text`.
    pub unmapped: u32,
    pub glyphs: Vec<GlyphBox>,
}

/// An image painted on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// True for an inline image (`BI`/`ID`/`EI`) rather than an image XObject.
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    Text(TextRun),
    Image(ImageInfo),
}

/// A text run or image on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageObject {
    pub id: ObjId,
    /// Paint order; higher is drawn later, on top.
    pub z: u32,
    /// Axis-aligned bounding box in page space.
    pub bbox: Rect,
    /// The object's own box as four corners, counter-clockwise from its bottom-left.
    pub quad: [(f64, f64); 4],
    /// True when the object lives in a form XObject rather than the page stream.
    /// Editing it changes every place that form is painted.
    pub in_form: bool,
    pub kind: ObjectKind,
}

impl PageObject {
    /// The text run, when this object is one.
    pub fn as_text(&self) -> Option<&TextRun> {
        match &self.kind {
            ObjectKind::Text(t) => Some(t),
            ObjectKind::Image(_) => None,
        }
    }

    /// The image, when this object is one.
    pub fn as_image(&self) -> Option<&ImageInfo> {
        match &self.kind {
            ObjectKind::Image(i) => Some(i),
            ObjectKind::Text(_) => None,
        }
    }
}

/// The objects of one page.
///
/// Coordinates are PDF points in *page space*: origin at the bottom-left corner of
/// the visible page, x to the right, y up, after the page's `CropBox` and `Rotate`
/// have been applied. `width` and `height` are the visible page size in that space.
#[derive(Debug, Clone, PartialEq)]
pub struct PageObjects {
    /// 1-based page number.
    pub page: u32,
    pub width: f64,
    pub height: f64,
    /// The page's `Rotate` entry, normalised to 0, 90, 180 or 270.
    pub rotate: u16,
    pub text: PageText,
    /// Objects in paint order.
    pub objects: Vec<PageObject>,
    /// True when a resource limit stopped interpretation early.
    pub truncated: bool,
    /// Human-readable notes, for example fonts whose encoding is not supported.
    pub warnings: Vec<String>,
}

/// Appends a JSON string literal.
pub(crate) fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Appends a number with at most three decimals.
pub(crate) fn json_num(out: &mut String, v: f64) {
    if !v.is_finite() {
        out.push('0');
        return;
    }
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() && r.abs() < 1e15 {
        out.push_str(&(r as i64).to_string());
    } else {
        let s = format!("{r:.3}");
        out.push_str(s.trim_end_matches('0').trim_end_matches('.'));
    }
}

/// Appends `[a,b,...]`.
pub(crate) fn json_nums(out: &mut String, vs: &[f64]) {
    out.push('[');
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_num(out, *v);
    }
    out.push(']');
}

/// Appends a rectangle as `[x0,y0,x1,y1]`.
pub(crate) fn json_rect(out: &mut String, r: &Rect) {
    json_nums(out, &[r.x0, r.y0, r.x1, r.y1]);
}

impl PageObject {
    /// Appends this object as JSON.
    pub(crate) fn write_json(&self, out: &mut String) {
        out.push_str(&format!("{{\"id\":{},\"z\":{},\"bbox\":", self.id, self.z));
        json_rect(out, &self.bbox);
        out.push_str(",\"quad\":");
        let q: Vec<f64> = self.quad.iter().flat_map(|p| [p.0, p.1]).collect();
        json_nums(out, &q);
        out.push_str(&format!(",\"in_form\":{}", self.in_form));
        match &self.kind {
            ObjectKind::Text(t) => {
                out.push_str(",\"kind\":\"text\",\"text\":");
                json_str(out, &t.text);
                out.push_str(",\"font\":");
                json_str(out, &t.font);
                out.push_str(",\"size\":");
                json_num(out, t.size);
                out.push_str(",\"color\":");
                match t.color {
                    Some(c) => json_nums(out, &c.map(f64::from)),
                    None => out.push_str("null"),
                }
                out.push_str(&format!(
                    ",\"bold\":{},\"italic\":{},\"invisible\":{}",
                    t.bold, t.italic, t.invisible
                ));
                out.push_str(",\"origin\":");
                json_nums(out, &[t.origin.0, t.origin.1]);
                out.push_str(",\"angle\":");
                json_num(out, t.angle);
                out.push_str(&format!(",\"unmapped\":{},\"glyphs\":[", t.unmapped));
                for (i, g) in t.glyphs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_nums(
                        out,
                        &[
                            g.bbox.x0,
                            g.bbox.y0,
                            g.bbox.x1,
                            g.bbox.y1,
                            f64::from(g.offset),
                        ],
                    );
                }
                out.push(']');
            }
            ObjectKind::Image(im) => {
                out.push_str(&format!(
                    ",\"kind\":\"image\",\"pixel_width\":{},\"pixel_height\":{},\"inline\":{}",
                    im.pixel_width, im.pixel_height, im.inline
                ));
            }
        }
        out.push('}');
    }
}

impl PageObjects {
    /// Serialises the page as JSON. Each glyph is `[x0,y0,x1,y1,text_offset]`.
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(256 + self.objects.len() * 256);
        out.push_str(&format!("{{\"page\":{},\"width\":", self.page));
        json_num(&mut out, self.width);
        out.push_str(",\"height\":");
        json_num(&mut out, self.height);
        out.push_str(&format!(
            ",\"rotate\":{},\"text\":\"{}\",\"text_editable\":{},\"truncated\":{},\"warnings\":[",
            self.rotate,
            self.text.as_str(),
            self.text.is_editable(),
            self.truncated
        ));
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            json_str(&mut out, w);
        }
        out.push_str("],\"objects\":[");
        for (i, o) in self.objects.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            o.write_json(&mut out);
        }
        out.push_str("]}");
        out
    }
}
