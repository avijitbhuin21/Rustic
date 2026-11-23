//! Font decoding: character codes to Unicode text and advance widths.

use std::collections::HashMap;
use std::sync::OnceLock;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::content::{decode_str, lex, name_of, Tok};
use crate::std14;
use crate::util::{
    as_dict, get, get_array, get_dict, get_matrix, get_name, get_num, get_rect, num, resolve,
    stream_data,
};

/// Largest CMap or font program the engine will decode.
const FONT_STREAM_CAP: usize = 64 << 20;

/// A parsed CMap: code space, code-to-CID ranges and code-to-Unicode mappings.
#[derive(Debug, Default)]
pub(crate) struct CMap {
    /// `(byte length, low, high)`.
    pub codespace: Vec<(u8, u32, u32)>,
    /// `(low, high, first cid)`, sorted by `low`.
    pub cid: Vec<(u32, u32, u32)>,
    /// `(low, high, first scalar value)`, sorted by `low`.
    pub uni: Vec<(u32, u32, u32)>,
    /// Codes that map to more than one character.
    pub multi: HashMap<u32, Box<str>>,
    pub vertical: bool,
}

/// Folds up to four bytes into a big-endian code.
fn be_code(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
}

/// Decodes the destination of a `bfchar`/`bfrange` entry as UTF-16BE.
fn utf16_be(bytes: &[u8]) -> String {
    if bytes.len() == 1 {
        return char::from(bytes[0]).to_string();
    }
    let units = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]));
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

impl CMap {
    /// Parses a CMap program. Unknown constructs are skipped.
    pub fn parse(data: &[u8]) -> CMap {
        let lx = lex(data, 400_000);
        let mut cmap = CMap::default();
        let mut buf = Vec::new();
        let mut expanded = 0usize;
        for op in &lx.ops {
            let args = lx.args(op);
            let strs = |tok: &Tok, buf: &mut Vec<u8>| match *tok {
                Tok::Str(sp, hex) => {
                    decode_str(data, sp, hex, buf);
                    true
                }
                _ => false,
            };
            match name_of(data, op.name) {
                b"endcodespacerange" => {
                    for pair in args.chunks_exact(2) {
                        if strs(&pair[0], &mut buf) {
                            let (len, lo) = (buf.len().min(4) as u8, be_code(&buf));
                            if strs(&pair[1], &mut buf) && len > 0 {
                                cmap.codespace.push((len, lo, be_code(&buf)));
                            }
                        }
                    }
                }
                b"endcidrange" => {
                    for t in args.chunks_exact(3) {
                        if let (true, Tok::Num(cid)) = (strs(&t[0], &mut buf), t[2]) {
                            let lo = be_code(&buf);
                            if strs(&t[1], &mut buf) {
                                cmap.cid
                                    .push((lo, be_code(&buf).max(lo), cid.max(0.0) as u32));
                            }
                        }
                    }
                }
                b"endcidchar" => {
                    for t in args.chunks_exact(2) {
                        if let (true, Tok::Num(cid)) = (strs(&t[0], &mut buf), t[1]) {
                            let code = be_code(&buf);
                            cmap.cid.push((code, code, cid.max(0.0) as u32));
                        }
                    }
                }
                b"endbfchar" => {
                    for pair in args.chunks_exact(2) {
                        if strs(&pair[0], &mut buf) {
                            let code = be_code(&buf);
                            if strs(&pair[1], &mut buf) {
                                cmap.map(code, code, &utf16_be(&buf), &mut expanded);
                            }
                        }
                    }
                }
                b"endbfrange" => {
                    let mut i = 0;
                    while i + 2 < args.len() {
                        if !strs(&args[i], &mut buf) {
                            i += 1;
                            continue;
                        }
                        let lo = be_code(&buf);
                        if !strs(&args[i + 1], &mut buf) {
                            i += 1;
                            continue;
                        }
                        let hi = be_code(&buf).max(lo);
                        i += 2;
                        if args[i] == Tok::ArrOpen {
                            i += 1;
                            let mut code = lo;
                            while i < args.len() && args[i] != Tok::ArrClose {
                                if strs(&args[i], &mut buf) && code <= hi {
                                    cmap.map(code, code, &utf16_be(&buf), &mut expanded);
                                    code = code.saturating_add(1);
                                }
                                i += 1;
                            }
                            i += 1;
                        } else {
                            if strs(&args[i], &mut buf) {
                                cmap.map(lo, hi, &utf16_be(&buf), &mut expanded);
                            }
                            i += 1;
                        }
                    }
                }
                b"def" => {
                    if let [Tok::Name(key), Tok::Num(v)] = args {
                        if name_of(data, *key) == b"WMode" && *v == 1.0 {
                            cmap.vertical = true;
                        }
                    }
                }
                _ => {}
            }
        }
        cmap.cid.sort_by_key(|r| r.0);
        cmap.uni.sort_by_key(|r| r.0);
        cmap
    }

    /// Records that codes `lo..=hi` map to `dst`, incrementing its last character.
    fn map(&mut self, lo: u32, hi: u32, dst: &str, expanded: &mut usize) {
        let mut chars = dst.chars();
        let (Some(first), rest) = (chars.next(), chars.as_str()) else {
            return;
        };
        if rest.is_empty() {
            self.uni.push((lo, hi, first as u32));
            return;
        }
        let Some(last) = dst.chars().last() else {
            return;
        };
        let stem = &dst[..dst.len() - last.len_utf8()];
        for code in lo..=hi.min(lo.saturating_add(0xFFFF)) {
            *expanded += 1;
            if *expanded > 200_000 {
                return;
            }
            let Some(tail) = char::from_u32(last as u32 + (code - lo)) else {
                continue;
            };
            self.multi
                .insert(code, format!("{stem}{tail}").into_boxed_str());
        }
    }

    /// Appends the text for `code`; false when the CMap has no entry.
    pub fn push_unicode(&self, code: u32, out: &mut String) -> bool {
        if let Some(s) = self.multi.get(&code) {
            out.push_str(s);
            return true;
        }
        let at = self.uni.partition_point(|r| r.0 <= code);
        // Later definitions sort after earlier ones with the same start; scan back a little.
        for &(lo, hi, dst) in self.uni[..at].iter().rev().take(4) {
            if code >= lo && code <= hi {
                if let Some(c) = char::from_u32(dst.saturating_add(code - lo)) {
                    out.push(c);
                    return true;
                }
            }
        }
        false
    }

    /// The CID for `code`, or `None` when no range covers it.
    pub fn cid(&self, code: u32) -> Option<u32> {
        let at = self.cid.partition_point(|r| r.0 <= code);
        let &(lo, hi, cid) = self.cid.get(at.checked_sub(1)?)?;
        (code <= hi).then(|| cid.saturating_add(code - lo))
    }
}

/// How a font program is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontKind {
    Simple,
    Type0,
    Type3,
}

/// Which built-in metrics set stands in for a font without `Widths`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Std14 {
    Latin(usize),
    Courier,
    Symbol,
    Zapf,
}

/// Cache of glyph name to Unicode scalar (zero when unknown), shared across fonts.
pub(crate) type GlyphNames = HashMap<Vec<u8>, u32>;

/// A decoded font: everything needed to turn show-string bytes into text and widths.
#[derive(Debug)]
pub(crate) struct Font {
    /// `BaseFont` without the subset tag.
    pub name: String,
    pub subset: bool,
    pub kind: FontKind,
    pub embedded: bool,
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub monospace: bool,
    /// Ascent and descent as fractions of the font size; descent is negative.
    pub ascent: f64,
    pub descent: f64,
    /// Glyph space to text space factor: 0.001, or a Type 3 font matrix scale.
    pub scale: f64,
    pub vertical: bool,
    /// Embedded font program and whether it is TrueType (`FontFile2`).
    pub file: Option<(ObjectId, bool)>,
    /// True when the encoding CMap is predefined and not built in; codes are then guessed.
    pub unsupported_cmap: bool,
    codespace: Vec<(u8, u32, u32)>,
    cidmap: Option<CMap>,
    code_is_utf16: bool,
    first_char: u32,
    widths: Vec<f32>,
    missing_width: f32,
    cid_widths: Vec<(u32, u32, f32)>,
    std: Option<Std14>,
    table: Option<Box<[u32; 256]>>,
    to_unicode: Option<CMap>,
    reverse: OnceLock<HashMap<char, u32>>,
    /// The `CIDToGIDMap` stream of a CIDFontType2, when it is not the identity.
    cid_to_gid: Option<ObjectId>,
    check: OnceLock<Option<GlyphCheck>>,
}

/// The four base encodings as code-to-scalar tables, taken from `lopdf`.
fn base_tables() -> &'static [[u32; 256]; 4] {
    static TABLES: OnceLock<[[u32; 256]; 4]> = OnceLock::new();
    TABLES.get_or_init(|| {
        let pdf = Pdf::new();
        let mut tables = [[0u32; 256]; 4];
        let names: [&[u8]; 4] = [
            b"StandardEncoding",
            b"WinAnsiEncoding",
            b"MacRomanEncoding",
            b"MacExpertEncoding",
        ];
        for (table, name) in tables.iter_mut().zip(names) {
            let mut dict = Dictionary::new();
            dict.set("Type", Object::Name(b"Font".to_vec()));
            dict.set("Encoding", Object::Name(name.to_vec()));
            let Ok(enc) = dict.get_font_encoding(&pdf) else {
                continue;
            };
            for code in 0..=255u8 {
                if let Ok(s) = enc.bytes_to_string(&[code]) {
                    table[usize::from(code)] = s.chars().next().map_or(0, |c| c as u32);
                }
            }
        }
        tables
    })
}

/// Index into [`base_tables`] for an encoding name.
fn base_index(name: &[u8]) -> Option<usize> {
    match name {
        b"StandardEncoding" => Some(0),
        b"WinAnsiEncoding" => Some(1),
        b"MacRomanEncoding" => Some(2),
        b"MacExpertEncoding" => Some(3),
        _ => None,
    }
}

/// Resolves a glyph name to a Unicode scalar, or zero when it is unknown.
fn glyph_unicode(name: &[u8], cache: &mut GlyphNames) -> u32 {
    if let Some(v) = cache.get(name) {
        return *v;
    }
    let hex = |s: &[u8]| {
        std::str::from_utf8(s)
            .ok()
            .and_then(|s| u32::from_str_radix(s, 16).ok())
    };
    let stem = name.split(|c| *c == b'.').next().unwrap_or(name);
    let scalar = if stem.len() == 7 && stem.starts_with(b"uni") {
        hex(&stem[3..])
    } else if (5..=7).contains(&stem.len())
        && stem[0] == b'u'
        && stem[1..].iter().all(u8::is_ascii_hexdigit)
    {
        hex(&stem[1..])
    } else {
        None
    };
    let value = scalar
        .filter(|v| char::from_u32(*v).is_some())
        .unwrap_or_else(|| {
            let lookup = |n: &[u8]| {
                let mut enc = Dictionary::new();
                enc.set("Type", Object::Name(b"Encoding".to_vec()));
                enc.set(
                    "Differences",
                    Object::Array(vec![Object::Integer(0), Object::Name(n.to_vec())]),
                );
                let mut dict = Dictionary::new();
                dict.set("Type", Object::Name(b"Font".to_vec()));
                dict.set("Encoding", Object::Dictionary(enc));
                let pdf = Pdf::new();
                let text = dict
                    .get_font_encoding(&pdf)
                    .ok()
                    .and_then(|e| e.bytes_to_string(&[0]).ok());
                text.and_then(|s| s.chars().next()).map_or(0, |c| c as u32)
            };
            // Names that are not identifiers cannot be in the glyph list; skip the lookup.
            let plausible =
                |n: &[u8]| !n.is_empty() && n.len() < 40 && n.iter().all(u8::is_ascii_alphanumeric);
            match (plausible(name), plausible(stem)) {
                (true, _) => match lookup(name) {
                    0 if stem != name => lookup(stem),
                    v => v,
                },
                (false, true) => lookup(stem),
                _ => 0,
            }
        });
    cache.insert(name.to_vec(), value);
    value
}

/// Picks the built-in metrics set for a font name such as `Arial,Bold`.
fn std14_for(name: &str) -> Option<Std14> {
    let n: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let bold = n.contains("bold");
    let italic = n.contains("italic") || n.contains("oblique");
    if n.starts_with("courier") {
        Some(Std14::Courier)
    } else if n.starts_with("helvetica") || n.starts_with("arial") {
        Some(Std14::Latin(usize::from(bold)))
    } else if n.starts_with("times") {
        Some(Std14::Latin(
            2 + usize::from(bold) + 2 * usize::from(italic),
        ))
    } else if n.starts_with("symbol") {
        Some(Std14::Symbol)
    } else if n.starts_with("zapfdingbats") {
        Some(Std14::Zapf)
    } else {
        None
    }
}

/// Width in 1/1000 em of a Unicode scalar in one of the Latin metric columns.
fn latin_width(scalar: u32, column: usize) -> Option<f32> {
    let key = u16::try_from(scalar).ok()?;
    let at = std14::LATIN.binary_search_by_key(&key, |row| row.0).ok()?;
    std14::LATIN[at].1.get(column).map(|w| f32::from(*w))
}

impl Font {
    /// A stand-in used when a show operator names a font that cannot be found.
    pub fn fallback() -> Font {
        Font {
            name: String::new(),
            subset: false,
            kind: FontKind::Simple,
            embedded: false,
            bold: false,
            italic: false,
            serif: false,
            monospace: false,
            ascent: 0.75,
            descent: -0.21,
            scale: 0.001,
            vertical: false,
            file: None,
            unsupported_cmap: false,
            codespace: Vec::new(),
            cidmap: None,
            code_is_utf16: false,
            first_char: 0,
            widths: Vec::new(),
            missing_width: 500.0,
            cid_widths: Vec::new(),
            std: None,
            table: Some(Box::new(base_tables()[1])),
            to_unicode: None,
            reverse: OnceLock::new(),
            cid_to_gid: None,
            check: OnceLock::new(),
        }
    }

    /// Builds a font from its dictionary. Malformed entries fall back to defaults.
    pub fn load(pdf: &Pdf, dict: &Dictionary, names: &mut GlyphNames) -> Font {
        let mut font = Font {
            table: None,
            missing_width: 0.0,
            ..Font::fallback()
        };
        let subtype = get_name(pdf, dict, b"Subtype").unwrap_or(b"");
        let base = get_name(pdf, dict, b"BaseFont")
            .or_else(|| get_name(pdf, dict, b"Name"))
            .unwrap_or(b"");
        let base = String::from_utf8_lossy(base);
        let tagged = base.len() > 7
            && base.as_bytes()[6] == b'+'
            && base.bytes().take(6).all(|c| c.is_ascii_uppercase());
        font.subset = tagged;
        font.name = if tagged {
            base[7..].to_string()
        } else {
            base.into_owned()
        };
        font.to_unicode = match get(pdf, dict, b"ToUnicode") {
            Some(Object::Stream(s)) => stream_data(s, FONT_STREAM_CAP).map(|d| CMap::parse(&d)),
            _ => None,
        };

        let descendant = get_array(pdf, dict, b"DescendantFonts")
            .and_then(|a| a.first())
            .and_then(|o| as_dict(resolve(pdf, o)));
        let holder = descendant.unwrap_or(dict);
        let descriptor = get_dict(pdf, holder, b"FontDescriptor");

        if subtype == b"Type0" {
            font.kind = FontKind::Type0;
            font.codespace = vec![(2, 0, 0xFFFF)];
            match get(pdf, dict, b"Encoding") {
                Some(Object::Name(name)) => {
                    font.vertical = name.ends_with(b"-V");
                    let ucs = name.windows(4).any(|w| w == b"UCS2")
                        || name.windows(5).any(|w| w == b"UTF16");
                    font.code_is_utf16 = ucs;
                    font.unsupported_cmap = !ucs && !name.starts_with(b"Identity");
                }
                Some(Object::Stream(s)) => {
                    if let Some(cmap) = stream_data(s, FONT_STREAM_CAP).map(|d| CMap::parse(&d)) {
                        font.vertical = cmap.vertical;
                        if !cmap.codespace.is_empty() {
                            font.codespace.clone_from(&cmap.codespace);
                        }
                        font.cidmap = (!cmap.cid.is_empty()).then_some(cmap);
                    }
                }
                _ => {}
            }
            font.missing_width = get_num(pdf, holder, b"DW").unwrap_or(1000.0) as f32;
            font.cid_to_gid = holder
                .get(b"CIDToGIDMap")
                .ok()
                .and_then(|o| o.as_reference().ok());
            if let Some(w) = get_array(pdf, holder, b"W") {
                font.cid_widths = parse_cid_widths(pdf, w);
            }
        } else {
            font.kind = if subtype == b"Type3" {
                FontKind::Type3
            } else {
                FontKind::Simple
            };
            font.first_char = get_num(pdf, dict, b"FirstChar").unwrap_or(0.0).max(0.0) as u32;
            if let Some(ws) = get_array(pdf, dict, b"Widths") {
                font.widths = ws
                    .iter()
                    .take(4096)
                    .map(|w| num(resolve(pdf, w)).unwrap_or(0.0) as f32)
                    .collect();
            }
            font.missing_width = descriptor
                .and_then(|d| get_num(pdf, d, b"MissingWidth"))
                .unwrap_or(0.0) as f32;
            if font.widths.is_empty() && font.kind == FontKind::Simple {
                font.std = std14_for(&font.name);
                if font.std.is_none() && font.missing_width == 0.0 {
                    font.missing_width = 500.0;
                }
            }
            font.table = Some(Box::new(simple_table(
                pdf, dict, subtype, font.std, descriptor, names,
            )));
        }

        if font.kind == FontKind::Type3 {
            let fm = get_matrix(pdf, dict, b"FontMatrix").map_or(0.001, |m| m.0[0].abs());
            font.scale = if fm > 0.0 { fm } else { 0.001 };
            // The y scale may be negative (Skia writes y-down glyph space), so keep its sign.
            let fy = get_matrix(pdf, dict, b"FontMatrix").map_or(0.001, |m| m.0[3]);
            if let Some(bbox) = get_rect(pdf, dict, b"FontBBox").filter(|b| b.height() > 0.0) {
                let (a, b) = (bbox.y0 * fy, bbox.y1 * fy);
                if a.max(b) > 0.0 && a.max(b) < 4.0 && a.min(b) > -4.0 {
                    font.ascent = a.max(b);
                    font.descent = a.min(b).min(0.0);
                }
            }
            font.embedded = true;
        }
        if let Some(d) = descriptor {
            let flags = get_num(pdf, d, b"Flags").unwrap_or(0.0) as u32;
            let weight = get_num(pdf, d, b"FontWeight").unwrap_or(0.0);
            let lower = font.name.to_ascii_lowercase();
            font.monospace = flags & 1 != 0;
            font.serif = flags & 2 != 0;
            font.italic = flags & 64 != 0 || get_num(pdf, d, b"ItalicAngle").unwrap_or(0.0) != 0.0;
            font.bold = flags & (1 << 18) != 0 || weight >= 600.0;
            font.italic |= lower.contains("italic") || lower.contains("oblique");
            font.bold |= ["bold", "black", "heavy"].iter().any(|k| lower.contains(k));
            for (key, truetype) in [
                (&b"FontFile2"[..], true),
                (b"FontFile3", false),
                (b"FontFile", false),
            ] {
                if let Ok(Object::Reference(id)) = d.get(key) {
                    font.file = Some((*id, truetype));
                    font.embedded = true;
                    break;
                }
            }
            if font.kind != FontKind::Type3 {
                let asc = get_num(pdf, d, b"Ascent").unwrap_or(0.0) / 1000.0;
                let desc = get_num(pdf, d, b"Descent").unwrap_or(0.0) / 1000.0;
                let bbox = get_rect(pdf, d, b"FontBBox");
                font.ascent = match (asc, bbox) {
                    (a, _) if a > 0.2 && a < 1.6 => a,
                    (_, Some(b)) if b.y1 > 200.0 && b.y1 < 1600.0 => b.y1 / 1000.0,
                    _ => font.ascent,
                };
                font.descent = match (desc, bbox) {
                    (d, _) if d < -0.02 && d > -0.8 => d,
                    (d, _) if d > 0.02 && d < 0.8 => -d,
                    (_, Some(b)) if b.y0 < -20.0 && b.y0 > -800.0 => b.y0 / 1000.0,
                    _ => font.descent,
                };
            }
        } else {
            let lower = font.name.to_ascii_lowercase();
            font.italic = lower.contains("italic") || lower.contains("oblique");
            font.bold = lower.contains("bold");
            font.serif = lower.starts_with("times");
            font.monospace = lower.starts_with("courier");
        }
        font
    }
}

/// Parses a CIDFont `W` array into `(first cid, last cid, width)` sorted by first cid.
fn parse_cid_widths(pdf: &Pdf, arr: &[Object]) -> Vec<(u32, u32, f32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < arr.len() && out.len() < 200_000 {
        let Some(first) = num(resolve(pdf, &arr[i])) else {
            break;
        };
        let first = first.max(0.0) as u32;
        match resolve(pdf, &arr[i + 1]) {
            Object::Array(ws) => {
                for (k, w) in ws.iter().enumerate().take(70_000) {
                    let cid = first.saturating_add(k as u32);
                    out.push((cid, cid, num(resolve(pdf, w)).unwrap_or(0.0) as f32));
                }
                i += 2;
            }
            other => {
                let (Some(last), Some(w)) = (
                    num(other),
                    arr.get(i + 2).and_then(|o| num(resolve(pdf, o))),
                ) else {
                    break;
                };
                out.push((first, (last.max(0.0) as u32).max(first), w as f32));
                i += 3;
            }
        }
    }
    out.sort_by_key(|r| r.0);
    out
}

/// Builds the code-to-Unicode table of a simple font from its encoding and differences.
fn simple_table(
    pdf: &Pdf,
    dict: &Dictionary,
    subtype: &[u8],
    std: Option<Std14>,
    descriptor: Option<&Dictionary>,
    names: &mut GlyphNames,
) -> [u32; 256] {
    let widen = |src: &[u16; 256]| {
        let mut t = [0u32; 256];
        for (dst, v) in t.iter_mut().zip(src) {
            *dst = u32::from(*v);
        }
        t
    };
    let flags = descriptor
        .and_then(|d| get_num(pdf, d, b"Flags"))
        .unwrap_or(32.0) as u32;
    let symbolic = flags & 4 != 0 && flags & 32 == 0;
    let enc = get(pdf, dict, b"Encoding");
    let named = match enc {
        Some(Object::Name(n)) => base_index(n),
        Some(Object::Dictionary(d)) => get_name(pdf, d, b"BaseEncoding").and_then(base_index),
        _ => None,
    };
    let mut table = match (named, std) {
        (Some(i), _) => base_tables()[i],
        (None, Some(Std14::Symbol)) => widen(&std14::SYMBOL_UNICODE),
        (None, Some(Std14::Zapf)) => widen(&std14::ZAPF_UNICODE),
        (None, _) if subtype == b"TrueType" && !symbolic => base_tables()[1],
        (None, _) => base_tables()[0],
    };
    if let Some(diffs) = enc
        .and_then(as_dict)
        .and_then(|d| get_array(pdf, d, b"Differences"))
    {
        let mut code = 0usize;
        for item in diffs {
            match resolve(pdf, item) {
                Object::Integer(i) => code = (*i).clamp(0, 256) as usize,
                Object::Name(name) => {
                    if let Some(slot) = table.get_mut(code) {
                        *slot = glyph_unicode(name, names);
                    }
                    code += 1;
                }
                _ => {}
            }
        }
    }
    table
}

impl Font {
    /// Reads the character code at `bytes[i..]`; returns the code and its byte length.
    pub fn next_code(&self, bytes: &[u8], i: usize) -> (u32, usize) {
        let rest = bytes.get(i..).unwrap_or(&[]);
        if self.kind != FontKind::Type0 {
            return (rest.first().map_or(0, |b| u32::from(*b)), 1);
        }
        for len in 1..=rest.len().min(4) {
            let code = be_code(&rest[..len]);
            if self
                .codespace
                .iter()
                .any(|&(l, lo, hi)| usize::from(l) == len && code >= lo && code <= hi)
            {
                return (code, len);
            }
        }
        let len = self
            .codespace
            .iter()
            .map(|r| usize::from(r.0))
            .min()
            .unwrap_or(2)
            .clamp(1, rest.len().max(1));
        (be_code(&rest[..len.min(rest.len())]), len)
    }

    /// The number of bytes used to write `code` in a show string.
    pub fn code_len(&self, code: u32) -> usize {
        if self.kind != FontKind::Type0 {
            return 1;
        }
        let fits = |len: u8| len >= 4 || code < (1u32 << (8 * u32::from(len)));
        let hit = self
            .codespace
            .iter()
            .find(|&&(l, lo, hi)| code >= lo && code <= hi && fits(l));
        hit.map_or(2, |r| usize::from(r.0))
    }

    /// Horizontal advance of `code` for a font size of one, before spacing.
    pub fn width(&self, code: u32) -> f64 {
        let units = if self.kind == FontKind::Type0 {
            let cid = self
                .cidmap
                .as_ref()
                .and_then(|m| m.cid(code))
                .unwrap_or(code);
            let at = self.cid_widths.partition_point(|r| r.0 <= cid);
            match at.checked_sub(1).and_then(|i| self.cid_widths.get(i)) {
                Some(&(_, hi, w)) if cid <= hi => w,
                _ => self.missing_width,
            }
        } else if let Some(w) = code
            .checked_sub(self.first_char)
            .and_then(|i| self.widths.get(i as usize))
        {
            *w
        } else {
            let scalar = self
                .table
                .as_ref()
                .and_then(|t| t.get(code as usize))
                .copied()
                .unwrap_or(0);
            let fallback = if self.missing_width > 0.0 {
                self.missing_width
            } else {
                500.0
            };
            match self.std {
                Some(Std14::Courier) => 600.0,
                Some(Std14::Latin(col)) => latin_width(scalar, col).unwrap_or(fallback),
                Some(Std14::Symbol) => std14::SYMBOL_WIDTHS
                    .get(code as usize)
                    .map_or(fallback, |w| f32::from(*w)),
                Some(Std14::Zapf) => std14::ZAPF_WIDTHS
                    .get(code as usize)
                    .map_or(fallback, |w| f32::from(*w)),
                None => self.missing_width,
            }
        };
        f64::from(units) * self.scale
    }

    /// Appends the Unicode text of `code`; pushes U+FFFD and returns false when unknown.
    pub fn push_unicode(&self, code: u32, out: &mut String) -> bool {
        if let Some(map) = &self.to_unicode {
            if map.push_unicode(code, out) {
                return true;
            }
        }
        let scalar = match &self.table {
            Some(t) => t.get(code as usize).copied().unwrap_or(0),
            None if self.code_is_utf16 => code,
            None => 0,
        };
        match char::from_u32(scalar).filter(|c| *c != '\0') {
            Some(c) => {
                out.push(c);
                true
            }
            None => {
                out.push('\u{FFFD}');
                false
            }
        }
    }

    /// The character code that shows `c`, when the font's maps contain one.
    pub fn encode(&self, c: char) -> Option<u32> {
        let map = self.reverse.get_or_init(|| {
            // Several codes may show one character: unused codes of WinAnsiEncoding all
            // read as a bullet. One the font lists a width for is one it really has;
            // among those the lowest wins.
            let better = |code: u32, than: u32| {
                (!self.has_width(code), code) < (!self.has_width(than), than)
            };
            let mut map = HashMap::new();
            if let Some(table) = &self.table {
                for (code, scalar) in table.iter().enumerate() {
                    if let Some(ch) = char::from_u32(*scalar).filter(|c| *c != '\0') {
                        let slot = map.entry(ch).or_insert(code as u32);
                        if better(code as u32, *slot) {
                            *slot = code as u32;
                        }
                    }
                }
            }
            if let Some(cmap) = &self.to_unicode {
                let mut direct = HashMap::new();
                for &(lo, hi, dst) in &cmap.uni {
                    for code in lo..=hi.min(lo.saturating_add(0xFFFF)) {
                        if let Some(ch) = char::from_u32(dst.saturating_add(code - lo)) {
                            let slot = direct.entry(ch).or_insert(code);
                            if better(code, *slot) {
                                *slot = code;
                            }
                        }
                    }
                }
                // A ToUnicode map describes the glyphs actually present, so it wins.
                map.extend(direct);
            }
            map
        });
        map.get(&c).copied()
    }

    /// True when `code` is the single-byte space that word spacing applies to.
    pub fn is_word_space(&self, code: u32, len: usize) -> bool {
        code == 32 && len == 1
    }

    /// True when the font has an explicit width for `code`, a hint that the glyph exists.
    pub fn has_width(&self, code: u32) -> bool {
        if self.kind == FontKind::Type0 {
            let cid = self
                .cidmap
                .as_ref()
                .and_then(|m| m.cid(code))
                .unwrap_or(code);
            let at = self.cid_widths.partition_point(|r| r.0 <= cid);
            return matches!(at.checked_sub(1).and_then(|i| self.cid_widths.get(i)), Some(&(_, hi, _)) if cid <= hi);
        }
        if self.std.is_some() {
            return self.width(code) > 0.0;
        }
        code.checked_sub(self.first_char)
            .and_then(|i| self.widths.get(i as usize))
            .is_some_and(|w| *w > 0.0)
    }
}

/// Which glyphs an embedded TrueType program really contains.
#[derive(Debug)]
pub(crate) struct GlyphCheck {
    /// Glyph ids that have an outline.
    outlined: Vec<bool>,
    /// Glyph id per character code of a simple font.
    simple: Option<Box<[u16; 256]>>,
    /// CID to glyph id, when the font carries an explicit map.
    cid_to_gid: Option<Vec<u16>>,
}

impl Font {
    /// Reads the embedded TrueType program once, to learn which glyphs it holds.
    fn glyph_check(&self, pdf: &Pdf) -> Option<&GlyphCheck> {
        self.check
            .get_or_init(|| {
                let (id, truetype) = self.file?;
                if !truetype {
                    return None;
                }
                let Some(Object::Stream(stream)) = pdf.objects.get(&id) else {
                    return None;
                };
                let data = stream_data(stream, FONT_STREAM_CAP)?;
                let face = ttf_parser::Face::parse(&data, 0).ok()?;
                let count = face.number_of_glyphs();
                let outlined = (0..count)
                    .map(|g| face.glyph_bounding_box(ttf_parser::GlyphId(g)).is_some())
                    .collect();
                let simple = self.table.as_ref().map(|table| {
                    let mut gids = Box::new([0u16; 256]);
                    for (code, slot) in gids.iter_mut().enumerate() {
                        let by_char = char::from_u32(table[code])
                            .filter(|c| *c != '\0')
                            .and_then(|c| face.glyph_index(c));
                        let by_code = || {
                            let cmap = face.tables().cmap?;
                            cmap.subtables.into_iter().find_map(|sub| {
                                sub.glyph_index(code as u32)
                                    .or_else(|| sub.glyph_index(0xF000 + code as u32))
                                    .filter(|g| g.0 != 0)
                            })
                        };
                        *slot = by_char
                            .filter(|g| g.0 != 0)
                            .or_else(by_code)
                            .map_or(0, |g| g.0);
                    }
                    gids
                });
                let cid_to_gid = self.cid_to_gid.and_then(|id| match pdf.objects.get(&id) {
                    Some(Object::Stream(s)) => stream_data(s, FONT_STREAM_CAP),
                    _ => None,
                });
                let cid_to_gid = cid_to_gid.map(|d| {
                    d.chunks_exact(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect()
                });
                Some(GlyphCheck {
                    outlined,
                    simple,
                    cid_to_gid,
                })
            })
            .as_ref()
    }

    /// True when showing `code` paints the glyph for `ch`, as far as the font tells.
    pub fn can_show(&self, pdf: &Pdf, code: u32, ch: char) -> bool {
        if !self.embedded {
            return self.kind != FontKind::Simple || self.std.is_some() || self.has_width(code);
        }
        let Some(check) = self.glyph_check(pdf) else {
            // No readable program: a subset lists real widths only for the glyphs it kept.
            return !self.subset || self.has_width(code);
        };
        let gid = match (&check.simple, &check.cid_to_gid) {
            (Some(gids), _) => gids.get(code as usize).copied().unwrap_or(0),
            (None, Some(map)) => {
                let cid = self
                    .cidmap
                    .as_ref()
                    .and_then(|m| m.cid(code))
                    .unwrap_or(code);
                map.get(cid as usize).copied().unwrap_or(0)
            }
            (None, None) => {
                let cid = self
                    .cidmap
                    .as_ref()
                    .and_then(|m| m.cid(code))
                    .unwrap_or(code);
                u16::try_from(cid).unwrap_or(0)
            }
        };
        match check.outlined.get(usize::from(gid)) {
            Some(outlined) => gid != 0 && (*outlined || ch.is_whitespace()),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_to_unicode_cmap() {
        let src = b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            2 beginbfchar <0003> <0020> <0010> <00660069> endbfchar\n\
            2 beginbfrange <0024> <0026> <0041> <0030> <0031> [<0061> <D83DDE00>] endbfrange";
        let cmap = CMap::parse(src);
        assert_eq!(cmap.codespace, vec![(2, 0, 0xFFFF)]);
        let text = |code| {
            let mut s = String::new();
            cmap.push_unicode(code, &mut s).then_some(s)
        };
        assert_eq!(text(3).as_deref(), Some(" "));
        assert_eq!(text(0x10).as_deref(), Some("fi"));
        assert_eq!(text(0x25).as_deref(), Some("B"));
        assert_eq!(text(0x30).as_deref(), Some("a"));
        assert_eq!(text(0x31).as_deref(), Some("\u{1F600}"));
        assert_eq!(text(0x27), None);
    }

    #[test]
    fn resolves_encodings_and_standard_widths() {
        assert_eq!(base_tables()[1][0x80], 0x20AC);
        assert_eq!(base_tables()[0][0x27], 0x2019);
        let mut names = GlyphNames::new();
        assert_eq!(glyph_unicode(b"eacute", &mut names), 0xE9);
        assert_eq!(glyph_unicode(b"uni20AC", &mut names), 0x20AC);
        assert_eq!(glyph_unicode(b"a.sc", &mut names), 0x61);
        assert_eq!(glyph_unicode(b"g1234", &mut names), 0);
        assert_eq!(std14_for("Arial,Bold"), Some(Std14::Latin(1)));
        assert_eq!(
            std14_for("TimesNewRomanPS-BoldItalicMT"),
            Some(Std14::Latin(5))
        );
        assert_eq!(latin_width(u32::from(b'W'), 0), Some(944.0));
    }
}
