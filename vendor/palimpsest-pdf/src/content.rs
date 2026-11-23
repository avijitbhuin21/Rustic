//! Content-stream tokenizer that keeps the byte span of every operation.

use crate::geom::Matrix;

/// A half-open byte range into the stream being tokenized.
pub(crate) type Sp = (u32, u32);

/// One operand token. Strings and names keep spans and are decoded on demand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Tok {
    Num(f64),
    /// Name without its leading slash.
    Name(Sp),
    /// String including its delimiters; `true` for a hex string.
    Str(Sp, bool),
    ArrOpen,
    ArrClose,
    DictOpen,
    DictClose,
    /// `true`, `false` or `null`.
    Word(Sp),
}

/// Operator classes the interpreter cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum K {
    Save,
    Restore,
    Cm,
    BT,
    ET,
    Tf,
    Td,
    TD,
    Tm,
    TStar,
    Tj,
    TJ,
    Quote,
    DQuote,
    Tc,
    Tw,
    Tz,
    TL,
    Ts,
    Tr,
    Do,
    /// A whole inline image, `BI` through `EI`.
    Inline,
    Gs,
    FillColor,
    FillSpace,
    StrokeColor,
    StrokeSpace,
    MoveTo,
    LineTo,
    Curve,
    Rect,
    Close,
    Clip,
    EndPath,
    Paint,
    Shade,
    Other,
}

/// One operation: its operands, operator and the bytes they occupy.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Op {
    pub k: K,
    pub name: Sp,
    /// Range into [`Lexed::toks`].
    pub toks: Sp,
    /// First byte of the first operand (or of the operator when there are none).
    pub start: u32,
    /// One past the last byte of the operator.
    pub end: u32,
}

/// A tokenized content stream.
#[derive(Debug, Default)]
pub(crate) struct Lexed {
    pub ops: Vec<Op>,
    pub toks: Vec<Tok>,
    /// True when tokenizing stopped early at the operation limit.
    pub truncated: bool,
}

impl Lexed {
    /// The operand tokens of an operation.
    pub fn args(&self, op: &Op) -> &[Tok] {
        self.toks
            .get(op.toks.0 as usize..op.toks.1 as usize)
            .unwrap_or(&[])
    }
}

fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(c: u8) -> bool {
    !is_ws(c) && !is_delim(c)
}

/// Maps an operator keyword to its class.
fn classify(w: &[u8]) -> K {
    match w {
        b"q" => K::Save,
        b"Q" => K::Restore,
        b"cm" => K::Cm,
        b"BT" => K::BT,
        b"ET" => K::ET,
        b"Tf" => K::Tf,
        b"Td" => K::Td,
        b"TD" => K::TD,
        b"Tm" => K::Tm,
        b"T*" => K::TStar,
        b"Tj" => K::Tj,
        b"TJ" => K::TJ,
        b"'" => K::Quote,
        b"\"" => K::DQuote,
        b"Tc" => K::Tc,
        b"Tw" => K::Tw,
        b"Tz" => K::Tz,
        b"TL" => K::TL,
        b"Ts" => K::Ts,
        b"Tr" => K::Tr,
        b"Do" => K::Do,
        b"gs" => K::Gs,
        b"g" | b"rg" | b"k" | b"sc" | b"scn" => K::FillColor,
        b"cs" => K::FillSpace,
        b"G" | b"RG" | b"K" | b"SC" | b"SCN" => K::StrokeColor,
        b"CS" => K::StrokeSpace,
        b"m" => K::MoveTo,
        b"l" => K::LineTo,
        b"c" | b"v" | b"y" => K::Curve,
        b"re" => K::Rect,
        b"h" => K::Close,
        b"W" | b"W*" => K::Clip,
        b"n" => K::EndPath,
        b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => K::Paint,
        b"sh" => K::Shade,
        _ => K::Other,
    }
}

/// Powers of ten that are exact in `f64`.
const POW10: [f64; 19] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18,
];

/// Parses a number starting at `i`; returns the value and the index after it.
fn lex_number(src: &[u8], i: usize) -> (f64, usize) {
    let mut j = i;
    let mut neg = false;
    while let Some(&c) = src.get(j) {
        match c {
            b'-' => neg = !neg,
            b'+' => {}
            _ => break,
        }
        j += 1;
    }
    // Digits go into an integer mantissa so that the result is correctly rounded.
    let (mut mant, mut digits, mut frac, mut dropped) = (0u64, 0usize, 0usize, 0usize);
    while let Some(c @ b'0'..=b'9') = src.get(j).copied() {
        if digits < 18 {
            mant = mant * 10 + u64::from(c - b'0');
            digits += usize::from(mant > 0);
        } else {
            dropped += 1;
        }
        j += 1;
    }
    if src.get(j) == Some(&b'.') {
        j += 1;
        while let Some(c) = src.get(j).copied() {
            match c {
                b'0'..=b'9' if digits < 18 => {
                    mant = mant * 10 + u64::from(c - b'0');
                    digits += usize::from(mant > 0);
                    frac += 1;
                }
                b'0'..=b'9' | b'-' => {}
                _ => break,
            }
            j += 1;
        }
    }
    let mut v = mant as f64;
    if frac > 0 {
        v /= POW10[frac.min(18)];
        if frac > 18 {
            v /= 10f64.powi(frac as i32 - 18);
        }
    }
    if dropped > 0 {
        v *= 10f64.powi(dropped.min(300) as i32);
    }
    (if neg { -v } else { v }, j.max(i + 1))
}

/// Returns the index one past the closing parenthesis of a literal string starting at `i`.
fn skip_literal(src: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(&c) = src.get(j) {
        j += 1;
        match c {
            b'\\' => j += 1,
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j.min(src.len());
                }
            }
            _ => {}
        }
    }
    src.len()
}

/// Finds the end of inline image data starting at `from`; returns the index after `EI`.
fn find_inline_end(src: &[u8], from: usize, declared: Option<usize>) -> usize {
    if let Some(len) = declared {
        let mut j = from.saturating_add(len);
        while src.get(j).copied().is_some_and(is_ws) {
            j += 1;
        }
        if src.get(j..j + 2) == Some(b"EI") {
            return j + 2;
        }
    }
    let mut j = from;
    while j + 1 < src.len() {
        if src[j] == b'E' && src[j + 1] == b'I' && (j == from || is_ws(src[j - 1])) {
            let after = j + 2;
            let boundary = src
                .get(after)
                .copied()
                .is_none_or(|c| is_ws(c) || is_delim(c));
            let tail = &src[after..(after + 16).min(src.len())];
            let texty = tail.iter().all(|&c| is_ws(c) || (0x20..0x7f).contains(&c));
            if boundary && texty {
                return after;
            }
        }
        j += 1;
    }
    src.len()
}

/// Tokenizes a content stream, stopping after `max_ops` operations.
pub(crate) fn lex(src: &[u8], max_ops: usize) -> Lexed {
    let mut out = Lexed::default();
    if src.len() >= u32::MAX as usize {
        out.truncated = true;
        return out;
    }
    let n = src.len();
    let mut i = 0usize;
    let mut first: Option<usize> = None;
    let mut tok_start = 0usize;
    let mut inline: Option<(usize, Sp)> = None;
    let sp = |a: usize, b: usize| (a as u32, b as u32);
    while i < n {
        let c = src[i];
        if is_ws(c) {
            i += 1;
            continue;
        }
        let begin = i;
        let tok = match c {
            b'%' => {
                while i < n && src[i] != b'\n' && src[i] != b'\r' {
                    i += 1;
                }
                continue;
            }
            b'/' => {
                i += 1;
                while i < n && is_regular(src[i]) {
                    i += 1;
                }
                Tok::Name(sp(begin + 1, i))
            }
            b'(' => {
                i = skip_literal(src, i);
                Tok::Str(sp(begin, i), false)
            }
            b'<' if src.get(i + 1) == Some(&b'<') => {
                i += 2;
                Tok::DictOpen
            }
            b'<' => {
                while i < n && src[i] != b'>' {
                    i += 1;
                }
                i = (i + 1).min(n);
                Tok::Str(sp(begin, i), true)
            }
            b'>' if src.get(i + 1) == Some(&b'>') => {
                i += 2;
                Tok::DictClose
            }
            b'[' => {
                i += 1;
                Tok::ArrOpen
            }
            b']' => {
                i += 1;
                Tok::ArrClose
            }
            b'>' | b')' | b'{' | b'}' => {
                i += 1;
                continue;
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let (v, j) = lex_number(src, i);
                i = j;
                Tok::Num(v)
            }
            _ => {
                while i < n && is_regular(src[i]) {
                    i += 1;
                }
                let word = &src[begin..i];
                match word {
                    b"true" | b"false" | b"null" => Tok::Word(sp(begin, i)),
                    b"BI" => {
                        inline = Some((first.unwrap_or(begin), sp(begin, i)));
                        first = None;
                        tok_start = out.toks.len();
                        continue;
                    }
                    _ => {
                        let mut op = Op {
                            k: classify(word),
                            name: sp(begin, i),
                            toks: sp(tok_start, out.toks.len()),
                            start: first.unwrap_or(begin) as u32,
                            end: i as u32,
                        };
                        if let (b"ID", Some((start, name))) = (word, inline.take()) {
                            let args = out.toks.get(tok_start..).unwrap_or(&[]);
                            let declared = args.windows(2).find_map(|w| match (w[0], w[1]) {
                                (Tok::Name(k), Tok::Num(v))
                                    if matches!(name_of(src, k), b"L" | b"Length") =>
                                {
                                    Some(v.max(0.0) as usize)
                                }
                                _ => None,
                            });
                            let data = if src.get(i).copied().is_some_and(is_ws) {
                                i + 1
                            } else {
                                i
                            };
                            i = find_inline_end(src, data, declared);
                            op = Op {
                                k: K::Inline,
                                name,
                                start: start as u32,
                                end: i as u32,
                                ..op
                            };
                        }
                        if out.ops.len() >= max_ops {
                            out.truncated = true;
                            return out;
                        }
                        out.ops.push(op);
                        first = None;
                        tok_start = out.toks.len();
                        continue;
                    }
                }
            }
        };
        first.get_or_insert(begin);
        out.toks.push(tok);
    }
    out
}

/// The raw bytes of a span, empty when out of range.
pub(crate) fn name_of(src: &[u8], sp: Sp) -> &[u8] {
    src.get(sp.0 as usize..sp.1 as usize).unwrap_or(&[])
}

/// Decodes a name, resolving `#xx` escapes.
pub(crate) fn decode_name(src: &[u8], sp: Sp) -> Vec<u8> {
    let raw = name_of(src, sp);
    if !raw.contains(&b'#') {
        return raw.to_vec();
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            raw[i],
            raw.get(i + 1).copied().and_then(hex),
            raw.get(i + 2).copied().and_then(hex),
        ) {
            (b'#', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Decodes a string token (with delimiters) into `out`, which is cleared first.
pub(crate) fn decode_str(src: &[u8], sp: Sp, hex: bool, out: &mut Vec<u8>) {
    out.clear();
    let raw = name_of(src, sp);
    if hex {
        let mut hi: Option<u8> = None;
        for &c in raw {
            let Some(d) = (c as char).to_digit(16) else {
                continue;
            };
            match hi.take() {
                Some(h) => out.push(h * 16 + d as u8),
                None => hi = Some(d as u8),
            }
        }
        if let Some(h) = hi {
            out.push(h * 16);
        }
        return;
    }
    let body = raw
        .get(1..raw.len().saturating_sub(usize::from(raw.ends_with(b")"))))
        .unwrap_or(&[]);
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        i += 1;
        if c != b'\\' {
            if c == b'\r' {
                out.push(b'\n');
                if body.get(i) == Some(&b'\n') {
                    i += 1;
                }
            } else {
                out.push(c);
            }
            continue;
        }
        let Some(&e) = body.get(i) else { break };
        i += 1;
        match e {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'0'..=b'7' => {
                let mut v = u32::from(e - b'0');
                for _ in 0..2 {
                    match body.get(i) {
                        Some(&d @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(v as u8);
            }
            b'\r' => {
                if body.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => {}
            other => out.push(other),
        }
    }
}

/// Writes a number in the shortest form that keeps twelve decimals.
///
/// Positions written back must match what a viewer computed from the original
/// operators closely enough not to disturb sub-pixel glyph placement; six decimals
/// measurably do in pdf.js. Twelve also absorb floating-point noise such as
/// 71.99999999999999.
pub(crate) fn put_num(out: &mut Vec<u8>, v: f64) {
    if !v.is_finite() {
        out.push(b'0');
        return;
    }
    if v.abs() >= 1e15 {
        out.extend_from_slice(format!("{v:.0}").as_bytes());
        return;
    }
    let s = format!("{v:.12}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    out.extend_from_slice(if s.is_empty() || s == "-0" || s == "-" {
        b"0"
    } else {
        s.as_bytes()
    });
}

/// Writes space-separated numbers followed by a space.
pub(crate) fn put_nums(out: &mut Vec<u8>, vs: &[f64]) {
    for v in vs {
        put_num(out, *v);
        out.push(b' ');
    }
}

/// Writes `a b c d e f <op> `.
pub(crate) fn put_matrix_op(out: &mut Vec<u8>, m: &Matrix, op: &str) {
    put_nums(out, &m.0);
    out.extend_from_slice(op.as_bytes());
    out.push(b' ');
}

/// Writes a name with its slash, escaping bytes that are not regular characters.
pub(crate) fn put_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &c in name {
        if is_regular(c) && c != b'#' && (0x21..0x7f).contains(&c) {
            out.push(c);
        } else {
            out.extend_from_slice(format!("#{c:02X}").as_bytes());
        }
    }
}

/// Writes bytes as a hex string.
pub(crate) fn put_hex(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'<');
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)]);
        out.push(HEX[usize::from(b & 15)]);
    }
    out.push(b'>');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(src: &[u8], op: &Op) -> String {
        String::from_utf8_lossy(&src[op.start as usize..op.end as usize]).into_owned()
    }

    #[test]
    fn keeps_the_span_of_every_operation() {
        let src =
            b"q 1 0 0 1 72 700 cm\nBT /F1 12 Tf (Hi \\(there\\)) Tj [(a) -20 <4142>] TJ ET % c\nQ";
        let lx = lex(src, usize::MAX);
        let got: Vec<String> = lx.ops.iter().map(|o| text(src, o)).collect();
        assert_eq!(
            got,
            [
                "q",
                "1 0 0 1 72 700 cm",
                "BT",
                "/F1 12 Tf",
                "(Hi \\(there\\)) Tj",
                "[(a) -20 <4142>] TJ",
                "ET",
                "Q"
            ]
        );
        assert_eq!(lx.ops[4].k, K::Tj);
        let mut buf = Vec::new();
        let Tok::Str(sp, hex) = lx.args(&lx.ops[4])[0] else {
            panic!("expected a string")
        };
        decode_str(src, sp, hex, &mut buf);
        assert_eq!(buf, b"Hi (there)");
        let Tok::Str(sp, hex) = lx.args(&lx.ops[5])[3] else {
            panic!("expected a string")
        };
        decode_str(src, sp, hex, &mut buf);
        assert_eq!(buf, b"AB");
    }

    #[test]
    fn treats_an_inline_image_as_one_operation() {
        let src = b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xff EI Q";
        let lx = lex(src, usize::MAX);
        let kinds: Vec<K> = lx.ops.iter().map(|o| o.k).collect();
        assert_eq!(kinds, [K::Save, K::Inline, K::Restore]);
        assert!(text(src, &lx.ops[1]).starts_with("BI") && text(src, &lx.ops[1]).ends_with("EI"));
        assert_eq!(lx.args(&lx.ops[1]).len(), 8);
    }

    #[test]
    fn survives_garbage_and_honours_the_limit() {
        let lx = lex(b"((( <<< ]] >> \\ (unterminated", usize::MAX);
        assert!(lx.ops.is_empty());
        let lx = lex(b"q q q q", 2);
        assert!(lx.truncated && lx.ops.len() == 2);
        let mut out = Vec::new();
        put_nums(
            &mut out,
            &[
                1.0,
                -0.5,
                0.1234567,
                f64::NAN,
                -0.0,
                71.99999999999999,
                -1e-14,
                566.910123456789,
            ],
        );
        assert_eq!(out, b"1 -0.5 0.1234567 0 0 72 0 566.910123456789 ");
    }

    #[test]
    fn parses_numbers_exactly() {
        let value = |s: &str| match lex(format!("{s} x").as_bytes(), 8).toks.first() {
            Some(Tok::Num(v)) => *v,
            other => panic!("{s}: {other:?}"),
        };
        for s in [
            "13.5492",
            "-167.8364",
            "16.93649",
            ".00048828125",
            "0.1",
            "595.32",
            "12345678901234567890",
            "-.5",
            "4.",
            "000.250",
        ] {
            assert_eq!(value(s), s.parse::<f64>().unwrap(), "{s}");
        }
        assert_eq!(value("+7"), 7.0);
        assert_eq!(value("--3"), 3.0);
        assert_eq!(value("1.5-3"), 1.53);
        assert_eq!(
            value("0.12345678901234567890123"),
            "0.123456789012345678".parse::<f64>().unwrap()
        );
    }
}
