//! Fallback fonts: choosing one for characters the page's font lacks, and embedding
//! a subset of it as a Type 0 font.

use std::collections::HashMap;
use std::sync::Arc;

use lopdf::{dictionary, Object, ObjectId, Stream};
use ttf_parser::{Face, GlyphId, Tag};

use crate::doc::{Document, Step};
use crate::font::Font;
use crate::util::deflate;
use crate::{Error, Result};

/// A font the host registered for characters that embedded fonts cannot show.
pub(crate) struct Fallback {
    pub name: String,
    data: Arc<Vec<u8>>,
    index: u32,
    bold: bool,
    italic: bool,
    serif: bool,
    monospace: bool,
    /// Metric-compatibility class; zero when the family is not a known one.
    class: u8,
    /// Subset glyph `i + 1` is original glyph `used[i].0` and stands for `used[i].1`.
    used: Vec<(u16, String)>,
    map: HashMap<(u16, String), u16>,
    /// Type 0 font, CID font, descriptor, font file and ToUnicode objects.
    objects: Option<[ObjectId; 5]>,
}

/// One glyph of shaped text, measured in em.
pub(crate) struct Shaped {
    pub gid: u16,
    pub advance: f64,
    pub offset: (f32, f32),
    /// The text this glyph stands for when text is extracted.
    pub text: String,
}

/// True for characters of right-to-left scripts.
pub(crate) fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Groups families whose glyphs have the same advance widths.
fn metric_class(name: &str) -> u8 {
    let n: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    if any(&["narrow", "condensed", "black", "rounded", "unicode"]) {
        0
    } else if any(&["arial", "helvetica", "liberationsans", "arimo"]) {
        1
    } else if any(&["times", "liberationserif", "tinos"]) {
        2
    } else if any(&["courier", "liberationmono", "cousine"]) {
        3
    } else if any(&["calibri", "carlito"]) {
        4
    } else if any(&["cambria", "caladea"]) {
        5
    } else {
        0
    }
}

/// Picks the registered font that best matches `font` among those that have `c`.
pub(crate) fn pick(fallbacks: &[Fallback], font: &Font, c: char) -> Option<usize> {
    let class = metric_class(&font.name);
    let score = |f: &Fallback| {
        16 * u32::from(class != 0 && f.class == class)
            + 4 * u32::from(f.bold == font.bold)
            + 4 * u32::from(f.italic == font.italic)
            + 2 * u32::from(f.monospace == font.monospace)
            + 2 * u32::from(f.serif == font.serif)
    };
    let mut best: Option<(usize, u32)> = None;
    for (i, f) in fallbacks.iter().enumerate().filter(|(_, f)| f.has(c)) {
        if best.is_none_or(|(_, s)| score(f) > s) {
            best = Some((i, score(f)));
        }
    }
    best.map(|(i, _)| i)
}

impl Fallback {
    /// Reads face `index` of a TrueType font or collection.
    fn new(data: Arc<Vec<u8>>, index: u32, ordinal: usize) -> Result<Fallback> {
        let bad = |what: &str| Error::Unsupported(format!("fallback font: {what}"));
        let face = Face::parse(&data, index).map_err(|e| bad(&e.to_string()))?;
        if face.tables().glyf.is_none() {
            return Err(bad("only fonts with TrueType outlines can be embedded"));
        }
        // OS/2 `fsType`: the licence may forbid embedding, or allow only the whole font
        // or only its bitmaps. A subset of the outlines is all that is ever written.
        let restricted = face.permissions() == Some(ttf_parser::Permissions::Restricted);
        if restricted || !face.is_subsetting_allowed() || !face.is_outline_embedding_allowed() {
            return Err(bad(
                "the font's embedding permissions do not allow a subset of it in a document",
            ));
        }
        let named = |id: u16| {
            face.names()
                .into_iter()
                .filter(|n| n.name_id == id)
                .find_map(|n| n.to_string())
        };
        let name = named(ttf_parser::name_id::POST_SCRIPT_NAME)
            .or_else(|| named(ttf_parser::name_id::FULL_NAME));
        let name: String = name
            .unwrap_or_else(|| format!("Fallback{ordinal}"))
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let lower = name.to_ascii_lowercase();
        let class = metric_class(&name);
        let serif = matches!(class, 2 | 5)
            || (lower.contains("serif") && !lower.contains("sans"))
            || ["georgia", "garamond", "roman"]
                .iter()
                .any(|k| lower.contains(k));
        Ok(Fallback {
            bold: face.is_bold(),
            italic: face.is_italic(),
            monospace: face.is_monospaced(),
            serif,
            class,
            name,
            data: data.clone(),
            index,
            used: Vec::new(),
            map: HashMap::new(),
            objects: None,
        })
    }

    fn face(&self) -> Option<Face<'_>> {
        Face::parse(&self.data, self.index).ok()
    }

    /// True when the font has a glyph for `c`.
    pub fn has(&self, c: char) -> bool {
        self.face()
            .and_then(|f| f.glyph_index(c))
            .is_some_and(|g| g.0 != 0)
    }

    /// Turns text into positioned glyphs, one per character.
    #[cfg(not(feature = "shaping"))]
    pub fn shape(&self, text: &str) -> Vec<Shaped> {
        let Some(face) = self.face() else {
            return Vec::new();
        };
        let em = f64::from(face.units_per_em().max(1));
        let glyph = |c: char| {
            let gid = face.glyph_index(c)?;
            let advance = f64::from(face.glyph_hor_advance(gid).unwrap_or(0)) / em;
            Some(Shaped {
                gid: gid.0,
                advance,
                offset: (0.0, 0.0),
                text: c.to_string(),
            })
        };
        text.chars().filter_map(glyph).collect()
    }

    /// Turns text into positioned glyphs with OpenType shaping.
    #[cfg(feature = "shaping")]
    pub fn shape(&self, text: &str) -> Vec<Shaped> {
        let Some(face) = rustybuzz::Face::from_slice(&self.data, self.index) else {
            return Vec::new();
        };
        let em = face.units_per_em().max(1) as f32;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        // Right-to-left layout is out of scope; glyphs are kept in logical order.
        buffer.set_direction(rustybuzz::Direction::LeftToRight);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let (infos, places) = (shaped.glyph_infos(), shaped.glyph_positions());
        let mut out = Vec::with_capacity(infos.len());
        let mut i = 0;
        while i < infos.len() {
            // The glyphs of one cluster share its text: one character each in logical
            // order, the last glyph taking the rest, so that extraction reads correctly.
            let start = infos[i].cluster as usize;
            let n = infos[i..]
                .iter()
                .take_while(|g| g.cluster as usize == start)
                .count();
            let end = infos
                .iter()
                .map(|g| g.cluster as usize)
                .filter(|c| *c > start)
                .min()
                .unwrap_or(text.len());
            let chars: Vec<char> = text.get(start..end).unwrap_or("").chars().collect();
            let shared = n.min(chars.len());
            for (j, (info, place)) in infos[i..i + n].iter().zip(&places[i..i + n]).enumerate() {
                let text: String = match j + 1 {
                    k if k < shared => chars[j].to_string(),
                    k if k == shared && n <= chars.len() => chars[j..].iter().collect(),
                    k if k == shared => chars[j].to_string(),
                    _ => "\u{200B}".to_string(),
                };
                out.push(Shaped {
                    gid: info.glyph_id as u16,
                    advance: f64::from(place.x_advance as f32 / em),
                    offset: (place.x_offset as f32 / em, place.y_offset as f32 / em),
                    text,
                });
            }
            i += n;
        }
        out
    }

    /// The glyph id that stands for original glyph `gid` showing `text` in the embedded
    /// subset. One glyph may appear under several ids when it stands for different text.
    pub fn subset_gid(&mut self, gid: u16, text: &str) -> u16 {
        if let Some(sub) = self.map.get(&(gid, text.to_string())) {
            return *sub;
        }
        if self.used.len() >= 0xFFFE {
            return 0;
        }
        self.used.push((gid, text.to_string()));
        let sub = self.used.len() as u16;
        self.map.insert((gid, text.to_string()), sub);
        sub
    }
}

/// Reads a big-endian `u16`, zero when out of range.
fn be16(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2)
        .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

/// The bytes of glyph `gid` in the `glyf` table.
fn glyph_data<'a>(glyf: &'a [u8], loca: &[u8], long: bool, gid: u16) -> &'a [u8] {
    let at = |i: usize| {
        if long {
            loca.get(i * 4..i * 4 + 4)
                .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        } else {
            usize::from(be16(loca, i * 2)) * 2
        }
    };
    let (a, b) = (at(usize::from(gid)), at(usize::from(gid) + 1));
    glyf.get(a..b.max(a)).unwrap_or(&[])
}

/// Sums a table as big-endian 32-bit words, the sfnt checksum.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut word = [0u8; 4];
        word[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// Assembles an sfnt file from its tables.
fn sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|t| t.0);
    let count = tables.len() as u16;
    let log = 15 - count.max(1).leading_zeros() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for v in [count, 16 << log, log, count * 16 - (16 << log)] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_at = Some(offset);
        }
        out.extend_from_slice(tag);
        for v in [checksum(data), offset as u32, data.len() as u32] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    if let Some(at) = head_at {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjust.to_be_bytes());
    }
    out
}

impl Fallback {
    /// Builds a font program holding only the glyphs used so far, renumbered from 1,
    /// and returns it with their widths in 1/1000 em.
    fn build(&mut self) -> Option<(Vec<u8>, Vec<i64>)> {
        let data = self.data.clone();
        let face = Face::parse(&data, self.index).ok()?;
        let table = |tag: &[u8; 4]| face.raw_face().table(Tag::from_bytes(tag));
        let (head, glyf, loca, hhea, maxp) = (
            table(b"head")?,
            table(b"glyf")?,
            table(b"loca")?,
            table(b"hhea")?,
            table(b"maxp")?,
        );
        if head.len() < 54 || hhea.len() < 36 || maxp.len() < 6 {
            return None;
        }
        let long = be16(head, 50) != 0;
        let em = f64::from(face.units_per_em().max(1));
        let (mut new_glyf, mut new_loca, mut hmtx, mut widths) =
            (Vec::new(), vec![0u8; 4], Vec::new(), Vec::new());
        let mut sub = 0usize;
        loop {
            // Glyph 0 stays the font's own "missing glyph".
            let gid = match sub.checked_sub(1) {
                None => 0,
                Some(i) => match self.used.get(i) {
                    Some(u) => u.0,
                    None => break,
                },
            };
            let mut glyph = glyph_data(glyf, loca, long, gid).to_vec();
            if glyph.len() >= 10 && (be16(&glyph, 0) as i16) < 0 {
                // A composite glyph: bring its components along and renumber them.
                let mut at = 10;
                while at + 4 <= glyph.len() {
                    let flags = be16(&glyph, at);
                    let part = self.subset_gid(be16(&glyph, at + 2), "");
                    glyph[at + 2..at + 4].copy_from_slice(&part.to_be_bytes());
                    at += 4 + if flags & 1 != 0 { 4 } else { 2 };
                    at += if flags & 8 != 0 {
                        2
                    } else if flags & 0x40 != 0 {
                        4
                    } else if flags & 0x80 != 0 {
                        8
                    } else {
                        0
                    };
                    if flags & 0x20 == 0 {
                        break;
                    }
                }
            }
            new_glyf.extend_from_slice(&glyph);
            new_glyf.resize(new_glyf.len().div_ceil(4) * 4, 0);
            new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
            let advance = face.glyph_hor_advance(GlyphId(gid)).unwrap_or(0);
            hmtx.extend_from_slice(&advance.to_be_bytes());
            hmtx.extend_from_slice(
                &face
                    .glyph_hor_side_bearing(GlyphId(gid))
                    .unwrap_or(0)
                    .to_be_bytes(),
            );
            if sub > 0 {
                widths.push((f64::from(advance) * 1000.0 / em).round() as i64);
            }
            sub += 1;
        }
        let count = (sub as u16).to_be_bytes();
        let mut head = head[..54].to_vec();
        head[8..12].fill(0);
        head[50..52].copy_from_slice(&1u16.to_be_bytes());
        let (mut hhea, mut maxp) = (hhea.to_vec(), maxp.to_vec());
        hhea[34..36].copy_from_slice(&count);
        maxp[4..6].copy_from_slice(&count);
        let mut post = vec![0u8; 32];
        post[1] = 3;
        let mut tables = vec![
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"maxp", maxp),
            (*b"hmtx", hmtx),
            (*b"loca", new_loca),
            (*b"glyf", new_glyf),
            (*b"post", post),
        ];
        for tag in [b"cvt ", b"fpgm", b"prep"] {
            tables.extend(table(tag).map(|t| (*tag, t.to_vec())));
        }
        Some((sfnt(tables), widths))
    }

    /// The ToUnicode CMap for the glyphs used so far.
    fn to_unicode(&self) -> Vec<u8> {
        let mut out = String::from(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
             1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
        );
        let mapped: Vec<(usize, &str)> = self
            .used
            .iter()
            .enumerate()
            .filter(|(_, u)| !u.1.is_empty())
            .map(|(i, u)| (i + 1, u.1.as_str()))
            .collect();
        for chunk in mapped.chunks(100) {
            out.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, text) in chunk {
                let hex: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                out.push_str(&format!("<{gid:04X}> <{hex}>\n"));
            }
            out.push_str("endbfchar\n");
        }
        out.push_str("endcmap CMapName currentdict /CMap defineresource pop end end\n");
        out.into_bytes()
    }
}

impl Document {
    /// Registers a font to fall back on when a page's font lacks a character.
    ///
    /// `data` is a TrueType font file (`.ttf`), or a collection (`.ttc`) of which every
    /// face is registered. Fonts with PostScript outlines (`.otf`) cannot be embedded
    /// and are rejected, and so are fonts whose embedding permissions (`fsType` in the
    /// `OS/2` table) are restricted, forbid subsetting or allow bitmaps only. The
    /// engine bundles no fonts and reads none from the system.
    /// Returns the names of the faces that were registered.
    pub fn add_fallback_font(&mut self, data: Vec<u8>) -> Result<Vec<String>> {
        let data = Arc::new(data);
        let mut names = Vec::new();
        let mut failure = Error::Unsupported("fallback font: no usable face".into());
        for index in 0..ttf_parser::fonts_in_collection(&data).unwrap_or(1).min(64) {
            match Fallback::new(data.clone(), index, self.fallbacks.len() + 1) {
                Ok(face) => {
                    names.push(face.name.clone());
                    self.fallbacks.push(face);
                }
                Err(e) => failure = e,
            }
        }
        if names.is_empty() {
            Err(failure)
        } else {
            Ok(names)
        }
    }

    /// Writes or refreshes the embedded subset of fallback `fi` and returns its font object.
    pub(crate) fn embed_fallback(&mut self, step: &mut Step, fi: usize) -> Result<ObjectId> {
        let broken = || Error::Unsupported("the fallback font could not be subsetted".into());
        let fallback = self.fallbacks.get_mut(fi).ok_or_else(broken)?;
        let (program, widths) = fallback.build().ok_or_else(broken)?;
        let cmap = fallback.to_unicode();
        let face = fallback.face().ok_or_else(broken)?;
        let k = 1000.0 / f64::from(face.units_per_em().max(1));
        let scaled = |v: i16| (f64::from(v) * k).round() as i64;
        let bbox = face.global_bounding_box();
        let flags = 4
            + i64::from(fallback.monospace)
            + 2 * i64::from(fallback.serif)
            + 64 * i64::from(fallback.italic);
        let tag: String = (0..6)
            .map(|i| char::from(b'A' + ((fi * 7 + i * 3 + 15) % 26) as u8))
            .collect();
        let base = Object::Name(format!("{tag}+{}", fallback.name).into_bytes());
        let mut descriptor = dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => base.clone(),
            "Flags" => flags,
            "FontBBox" => vec![scaled(bbox.x_min).into(), scaled(bbox.y_min).into(), scaled(bbox.x_max).into(), scaled(bbox.y_max).into()],
            "ItalicAngle" => face.italic_angle().round() as i64,
            "Ascent" => scaled(face.ascender()),
            "Descent" => scaled(face.descender()),
            "CapHeight" => scaled(face.capital_height().unwrap_or(face.ascender())),
            "StemV" => if fallback.bold { 120 } else { 80 },
        };
        let known = fallback.objects;
        let ids = match known.filter(|ids| ids.iter().all(|id| self.pdf.objects.contains_key(id))) {
            Some(ids) => {
                ids.iter().for_each(|id| self.record(step, *id));
                ids
            }
            None => {
                let ids = [(); 5].map(|()| self.pdf.new_object_id());
                step.objects.extend(ids.iter().map(|id| (*id, None)));
                ids
            }
        };
        let [type0, cid, descriptor_id, file, to_unicode] = ids;
        descriptor.set("FontFile2", file);
        let file_dict =
            dictionary! { "Length1" => program.len() as i64, "Filter" => "FlateDecode" };
        let cid_font = dictionary! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => base.clone(),
            "CIDSystemInfo" => dictionary! {
                "Registry" => Object::string_literal("Adobe"),
                "Ordering" => Object::string_literal("Identity"),
                "Supplement" => 0,
            },
            "FontDescriptor" => descriptor_id,
            "DW" => 1000,
            "W" => vec![1.into(), Object::Array(widths.into_iter().map(Object::Integer).collect())],
            "CIDToGIDMap" => "Identity",
        };
        let font = dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => base,
            "Encoding" => "Identity-H",
            "DescendantFonts" => vec![cid.into()],
            "ToUnicode" => to_unicode,
        };
        self.pdf.objects.insert(type0, Object::Dictionary(font));
        self.pdf.objects.insert(cid, Object::Dictionary(cid_font));
        self.pdf
            .objects
            .insert(descriptor_id, Object::Dictionary(descriptor));
        self.pdf.objects.insert(
            file,
            Object::Stream(Stream::new(file_dict, deflate(&program)).with_compression(false)),
        );
        self.pdf.objects.insert(
            to_unicode,
            Object::Stream(Stream::new(dictionary! {}, cmap)),
        );
        self.plain.insert(to_unicode);
        self.fonts.by_id.remove(&type0);
        if let Some(fallback) = self.fallbacks.get_mut(fi) {
            fallback.objects = Some(ids);
        }
        Ok(type0)
    }
}
