//! Helpers for reading `lopdf` objects without panicking on malformed input.

use std::io::Read;

use lopdf::{Dictionary, Document as Pdf, Object, Stream};

use crate::geom::{Matrix, Rect};

static NULL: Object = Object::Null;

/// Follows indirect references; yields `Null` for a missing object or a reference loop.
pub(crate) fn resolve<'a>(doc: &'a Pdf, mut obj: &'a Object) -> &'a Object {
    for _ in 0..32 {
        match obj {
            Object::Reference(id) => match doc.objects.get(id) {
                Some(next) => obj = next,
                None => return &NULL,
            },
            other => return other,
        }
    }
    &NULL
}

/// Looks up a dictionary entry and resolves it; `None` when absent or null.
pub(crate) fn get<'a>(doc: &'a Pdf, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let obj = resolve(doc, dict.get(key).ok()?);
    (!obj.is_null()).then_some(obj)
}

/// A number from an integer or real object.
pub(crate) fn num(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        // lopdf stores reals as f32; go through the shortest decimal form so that
        // 594.96 does not become 594.9599609375.
        Object::Real(r) if r.is_finite() => r.to_string().parse().ok(),
        _ => None,
    }
}

/// A numeric dictionary entry.
pub(crate) fn get_num(doc: &Pdf, dict: &Dictionary, key: &[u8]) -> Option<f64> {
    num(get(doc, dict, key)?)
}

/// The dictionary of an object, looking inside streams too.
pub(crate) fn as_dict(obj: &Object) -> Option<&Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// A dictionary-valued entry.
pub(crate) fn get_dict<'a>(
    doc: &'a Pdf,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<&'a Dictionary> {
    as_dict(get(doc, dict, key)?)
}

/// A name-valued entry.
pub(crate) fn get_name<'a>(doc: &'a Pdf, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    get(doc, dict, key)?.as_name().ok()
}

/// An array-valued entry.
pub(crate) fn get_array<'a>(
    doc: &'a Pdf,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<&'a [Object]> {
    get(doc, dict, key)?.as_array().ok().map(Vec::as_slice)
}

/// Reads an array of at least `N` numbers.
pub(crate) fn numbers<const N: usize>(doc: &Pdf, arr: &[Object]) -> Option<[f64; N]> {
    let mut out = [0.0; N];
    for (slot, obj) in out.iter_mut().zip(arr.get(..N)?) {
        *slot = num(resolve(doc, obj))?;
    }
    Some(out)
}

/// A rectangle entry such as `MediaBox`, normalised.
pub(crate) fn get_rect(doc: &Pdf, dict: &Dictionary, key: &[u8]) -> Option<Rect> {
    let [a, b, c, d] = numbers::<4>(doc, get_array(doc, dict, key)?)?;
    Some(Rect::new(a, b, c, d))
}

/// A matrix entry such as `Matrix` or `FontMatrix`.
pub(crate) fn get_matrix(doc: &Pdf, dict: &Dictionary, key: &[u8]) -> Option<Matrix> {
    Some(Matrix(numbers::<6>(doc, get_array(doc, dict, key)?)?)).filter(Matrix::is_finite)
}

/// Decodes a stream, refusing to produce more than `cap` bytes.
pub(crate) fn stream_data(stream: &Stream, cap: usize) -> Option<Vec<u8>> {
    let filters = stream.filters().unwrap_or_default();
    if filters.is_empty() {
        return (stream.content.len() <= cap).then(|| stream.content.clone());
    }
    let predictor = stream
        .dict
        .get(b"DecodeParms")
        .ok()
        .and_then(as_dict)
        .and_then(|p| p.get(b"Predictor").ok())
        .and_then(num)
        .unwrap_or(1.0);
    if filters.len() == 1 && filters[0] == b"FlateDecode" && predictor <= 1.0 {
        let mut out = Vec::with_capacity(stream.content.len().saturating_mul(3).min(cap));
        let limit = cap as u64 + 1;
        // A truncated or corrupt tail still yields the bytes decoded so far, as viewers do.
        let _ = flate2::read::ZlibDecoder::new(stream.content.as_slice())
            .take(limit)
            .read_to_end(&mut out);
        return (!out.is_empty() && out.len() <= cap).then_some(out);
    }
    // Compressed input cannot expand without bound under the remaining filters lopdf knows.
    if stream.content.len() > cap {
        return None;
    }
    stream
        .decompressed_content()
        .ok()
        .filter(|d| d.len() <= cap)
}

/// Compresses bytes with zlib at a speed-oriented level.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(
        Vec::with_capacity(data.len() / 3 + 16),
        flate2::Compression::new(5),
    );
    if enc.write_all(data).is_err() {
        return Vec::new();
    }
    enc.finish().unwrap_or_default()
}
