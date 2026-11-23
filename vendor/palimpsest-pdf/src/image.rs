//! Replacing the pixels of an image, and registering new resources.

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream};

use crate::content::put_name;
use crate::doc::{Document, Step};
use crate::edit::{Fate, Splice};
use crate::interp::{RawKind, StreamKey};
use crate::model::ObjId;
use crate::util::{as_dict, deflate};
use crate::{Error, Result};

/// Pixels for [`Document::replace_image`].
///
/// The engine does not decode image files. JPEG data is embedded as it is; anything
/// else is passed as raw 8-bit samples, row by row from the top.
#[derive(Debug, Clone, Copy)]
pub enum ImageData<'a> {
    /// A complete baseline or progressive JPEG file.
    Jpeg(&'a [u8]),
    /// One byte per pixel.
    Gray {
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
    /// Three bytes per pixel.
    Rgb {
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
    /// Four bytes per pixel, alpha not premultiplied.
    Rgba {
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
}

/// Reads width, height and component count from a JPEG's frame header.
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8, bool)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut adobe = false;
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF || marker == 0x00 || (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            i += if marker == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        let body = data.get(i + 4..i + 2 + len)?;
        match marker {
            0xEE => adobe |= body.starts_with(b"Adobe"),
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let height = u32::from(u16::from_be_bytes([*body.get(1)?, *body.get(2)?]));
                let width = u32::from(u16::from_be_bytes([*body.get(3)?, *body.get(4)?]));
                return Some((width, height, *body.get(5)?, adobe))
                    .filter(|_| width > 0 && height > 0);
            }
            0xDA | 0xD9 => return None,
            _ => {}
        }
        i += 2 + len.max(2);
    }
    None
}

/// Builds an image XObject dictionary.
fn image_dict(width: u32, height: u32, space: &str) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Image".to_vec()));
    dict.set("Width", i64::from(width));
    dict.set("Height", i64::from(height));
    dict.set("ColorSpace", Object::Name(space.as_bytes().to_vec()));
    dict.set("BitsPerComponent", 8);
    dict
}

/// Builds a Flate-compressed image stream from raw samples.
fn flate_image(width: u32, height: u32, space: &str, samples: &[u8]) -> Stream {
    let mut dict = image_dict(width, height, space);
    dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    Stream::new(dict, deflate(samples)).with_compression(false)
}

/// The dictionary inside object `id` reached by descending through `path`.
fn dict_at<'a>(pdf: &'a mut Pdf, id: ObjectId, path: &[&[u8]]) -> Option<&'a mut Dictionary> {
    let mut dict = match pdf.objects.get_mut(&id)? {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &mut s.dict,
        _ => return None,
    };
    for key in path {
        dict = dict.get_mut(key).ok()?.as_dict_mut().ok()?;
    }
    Some(dict)
}

impl Document {
    /// Registers `target` under a fresh name in the `category` resources (`XObject`,
    /// `Font`) used by a stream of `page`, and returns that name.
    pub(crate) fn add_resource(
        &mut self,
        step: &mut Step,
        page: u32,
        key: StreamKey,
        category: &'static [u8],
        prefix: &str,
        target: ObjectId,
    ) -> Result<Vec<u8>> {
        let page_id = self.page_id(page)?;
        let has_resources = |pdf: &Pdf, id: ObjectId| {
            pdf.objects
                .get(&id)
                .and_then(as_dict)
                .is_some_and(|d| d.has(b"Resources"))
        };
        let mut owner = page_id;
        match key {
            StreamKey::Form(id) if has_resources(&self.pdf, id) => owner = id,
            _ => {
                let mut node = page_id;
                for _ in 0..64 {
                    if has_resources(&self.pdf, node) {
                        owner = node;
                        break;
                    }
                    let parent = self
                        .pdf
                        .objects
                        .get(&node)
                        .and_then(as_dict)
                        .and_then(|d| d.get(b"Parent").ok());
                    match parent.and_then(|p| p.as_reference().ok()) {
                        Some(next) => node = next,
                        None => break,
                    }
                }
            }
        }
        let broken = || Error::Unsupported("the page's resources cannot be extended".into());
        let mut holder = owner;
        let mut path: Vec<&[u8]> = Vec::new();
        self.record(step, holder);
        for name in [&b"Resources"[..], category] {
            let mut hops = 0;
            loop {
                let dict = dict_at(&mut self.pdf, holder, &path).ok_or_else(broken)?;
                match dict.get(name) {
                    Ok(Object::Reference(next)) => {
                        let next = *next;
                        hops += 1;
                        if hops > 8 || self.pdf.objects.get(&next).and_then(as_dict).is_none() {
                            return Err(broken());
                        }
                        self.record(step, next);
                        // The referenced object is itself the dictionary we are looking for.
                        holder = next;
                        path.clear();
                        break;
                    }
                    Ok(Object::Dictionary(_)) => {
                        path.push(name);
                        break;
                    }
                    _ => dict.set(name.to_vec(), Object::Dictionary(Dictionary::new())),
                }
            }
        }
        let dict = dict_at(&mut self.pdf, holder, &path).ok_or_else(broken)?;
        let mut n = dict.len() + 1;
        let name = loop {
            let candidate = format!("{prefix}{n}").into_bytes();
            if !dict.has(&candidate) {
                break candidate;
            }
            n += 1;
        };
        dict.set(name.clone(), Object::Reference(target));
        Ok(name)
    }

    /// Replaces the pixels of an image. The image keeps its place and size on the page;
    /// other places that paint the same image XObject are not affected.
    pub fn replace_image(&mut self, page: u32, id: ObjId, data: ImageData<'_>) -> Result<()> {
        let index = self.locate(page, id)?;
        let state = self.state(page)?;
        let item = state
            .interp
            .items
            .get(index)
            .ok_or(Error::NoSuchObject { page, id })?;
        if !matches!(item.kind, RawKind::Image(_)) {
            return Err(Error::Unsupported("the object is not an image".into()));
        }
        let key = state
            .interp
            .streams
            .get(item.stream as usize)
            .map(|s| s.key)
            .ok_or(Error::NoSuchObject { page, id })?;
        let (start, end) = (item.start, item.end);

        let bad = |what: &str| Error::Unsupported(format!("image data: {what}"));
        let sized = |w: u32, h: u32, channels: usize, pixels: &[u8]| {
            let need = (w as usize)
                .checked_mul(h as usize)
                .and_then(|n| n.checked_mul(channels));
            match need {
                Some(n) if n > 0 && n == pixels.len() => Ok(()),
                _ => Err(bad("the pixel buffer does not match the stated size")),
            }
        };
        let mut mask = None;
        let stream = match data {
            ImageData::Jpeg(bytes) => {
                let (w, h, components, adobe) =
                    jpeg_info(bytes).ok_or_else(|| bad("not a readable JPEG"))?;
                let space = match components {
                    1 => "DeviceGray",
                    3 => "DeviceRGB",
                    4 => "DeviceCMYK",
                    _ => return Err(bad("unsupported number of JPEG components")),
                };
                let mut dict = image_dict(w, h, space);
                dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
                if components == 4 && adobe {
                    // Adobe CMYK JPEGs store inverted samples.
                    dict.set(
                        "Decode",
                        Object::Array([1, 0, 1, 0, 1, 0, 1, 0].map(Object::Integer).to_vec()),
                    );
                }
                Stream::new(dict, bytes.to_vec()).with_compression(false)
            }
            ImageData::Gray {
                width,
                height,
                pixels,
            } => {
                sized(width, height, 1, pixels)?;
                flate_image(width, height, "DeviceGray", pixels)
            }
            ImageData::Rgb {
                width,
                height,
                pixels,
            } => {
                sized(width, height, 3, pixels)?;
                flate_image(width, height, "DeviceRGB", pixels)
            }
            ImageData::Rgba {
                width,
                height,
                pixels,
            } => {
                sized(width, height, 4, pixels)?;
                let mut rgb = Vec::with_capacity(pixels.len() / 4 * 3);
                let mut alpha = Vec::with_capacity(pixels.len() / 4);
                for px in pixels.chunks_exact(4) {
                    rgb.extend_from_slice(&px[..3]);
                    alpha.push(px[3]);
                }
                if alpha.iter().any(|a| *a != 255) {
                    mask = Some(flate_image(width, height, "DeviceGray", &alpha));
                }
                flate_image(width, height, "DeviceRGB", &rgb)
            }
        };

        let mut step = self.begin("replace image", page);
        let mut stream = stream;
        if let Some(mask) = mask {
            let mask_id = self.add_object(&mut step, Object::Stream(mask));
            stream.dict.set("SMask", Object::Reference(mask_id));
        }
        let image_id = self.add_object(&mut step, Object::Stream(stream));
        let outcome = self
            .add_resource(&mut step, page, key, b"XObject", "PlIm", image_id)
            .and_then(|name| {
                let mut bytes = vec![b' '];
                put_name(&mut bytes, &name);
                bytes.extend_from_slice(b" Do ");
                self.splice(
                    &mut step,
                    page,
                    key,
                    vec![Splice {
                        start,
                        end,
                        bytes,
                        fate: Fate::Keep(1),
                    }],
                )
            });
        match outcome {
            Ok(()) => {
                self.commit(step);
                Ok(())
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

    /// A minimal JPEG header: SOI, APP0, SOF0 for a 3-component 7x5 image.
    fn jpeg_header() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x05, 0x00, 0x07, 0x03]);
        v.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 0xFF, 0xD9]);
        v
    }

    #[test]
    fn reads_jpeg_dimensions() {
        assert_eq!(jpeg_info(&jpeg_header()), Some((7, 5, 3, false)));
        assert_eq!(jpeg_info(b"\xFF\xD8\xFF"), None);
        assert_eq!(jpeg_info(b"not a jpeg"), None);
    }

    #[test]
    fn replaces_pixels_and_keeps_the_place() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let images: Vec<(ObjId, Rect)> = doc
            .page_objects(1)
            .unwrap()
            .objects
            .iter()
            .filter(|o| o.as_image().is_some())
            .map(|o| (o.id, o.bbox))
            .collect();
        let total = doc.page_objects(1).unwrap().objects.len();
        // The first two paint the same XObject; replacing one must not touch the other.
        let rgba: Vec<u8> = (0..3 * 2)
            .flat_map(|i| [i as u8 * 40, 0, 255, if i == 0 { 0 } else { 255 }])
            .collect();
        doc.replace_image(
            1,
            images[0].0,
            ImageData::Rgba {
                width: 3,
                height: 2,
                pixels: &rgba,
            },
        )
        .unwrap();
        doc.replace_image(1, images[2].0, ImageData::Jpeg(&jpeg_header()))
            .unwrap();
        let check = |doc: &mut Document, by_id: bool| {
            let page = doc.page_objects(1).unwrap();
            assert_eq!(page.objects.len(), total);
            let got: Vec<_> = page
                .objects
                .iter()
                .filter(|o| o.as_image().is_some())
                .collect();
            let dims: Vec<_> = got
                .iter()
                .map(|o| {
                    o.as_image()
                        .map(|i| (i.pixel_width, i.pixel_height, i.inline))
                        .unwrap()
                })
                .collect();
            assert_eq!(dims, [(3, 2, false), (2, 2, false), (7, 5, false)]);
            for (o, (id, bbox)) in got.iter().zip(&images) {
                assert_eq!(o.bbox, *bbox);
                assert!(!by_id || o.id == *id);
            }
        };
        check(&mut doc, true);
        let saved = doc.save().unwrap();
        check(&mut Document::from_bytes(&saved).unwrap(), false);
        // The transparent pixel produced a soft mask.
        assert!(saved.windows(6).any(|w| w == b"/SMask"));
        doc.undo();
        doc.undo();
        let page = doc.page_objects(1).unwrap();
        assert!(page
            .objects
            .iter()
            .filter_map(|o| o.as_image())
            .all(|i| (i.pixel_width, i.pixel_height) == (2, 2)));
    }

    #[test]
    fn rejects_bad_image_data_without_changing_the_document() {
        let mut doc = Document::from_bytes(&fixtures::rich()).unwrap();
        let before = doc.save().unwrap();
        let page = doc.page_objects(1).unwrap();
        let image = page
            .objects
            .iter()
            .find(|o| o.as_image().is_some())
            .unwrap()
            .id;
        let text = page
            .objects
            .iter()
            .find(|o| o.as_text().is_some())
            .unwrap()
            .id;
        let short = ImageData::Rgb {
            width: 4,
            height: 4,
            pixels: &[0; 5],
        };
        assert!(matches!(
            doc.replace_image(1, image, short),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            doc.replace_image(1, image, ImageData::Jpeg(b"nope")),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            doc.replace_image(
                1,
                text,
                ImageData::Gray {
                    width: 1,
                    height: 1,
                    pixels: &[0]
                }
            ),
            Err(Error::Unsupported(_))
        ));
        assert!(doc.history().is_empty());
        assert_eq!(doc.save().unwrap(), before);
    }
}
