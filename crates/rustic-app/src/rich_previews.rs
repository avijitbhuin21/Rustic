//! Host-independent decoders for file types the webview can't show natively:
//! HEIC/HEIF, TIFF, camera RAW (embedded preview), ICNS, archives and SQLite.
//! Both hosts expose these as thin commands (see host_ops parity notes).
//! Pure Rust only — no C libraries (user constraint).

use base64::Engine as _;
use serde::Serialize;
use std::io::Read;
use std::path::Path;

use crate::path_scope::validate_readable_path;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// One displayable image produced from a non-web format.
#[derive(Debug, Clone, Serialize)]
pub struct DecodedImage {
    pub mime: String,
    /// Base64 of an encoded PNG/JPEG the webview can display.
    pub data: String,
    pub width: u32,
    pub height: u32,
    /// Short human label, e.g. "512×512" for an ICNS member.
    pub label: String,
}

/// Result of `decode_image_preview`: one or more images plus an explanatory note.
#[derive(Debug, Clone, Serialize)]
pub struct ImagePreview {
    pub images: Vec<DecodedImage>,
    pub note: Option<String>,
}

const RAW_EXT: &[&str] = &[
    "raw", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "orf", "rw2", "raf",
    "pef", "srw", "3fr", "erf", "kdc", "mos", "mrw", "x3f", "iiq", "rwl",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn read_all(path: &Path) -> Result<Vec<u8>, String> {
    validate_readable_path(path)?;
    std::fs::read(path).map_err(|e| e.to_string())
}

/// Decodes HEIC/HEIF, TIFF, camera RAW and ICNS into web-displayable images.
pub fn decode_image_preview(path: &str) -> Result<ImagePreview, String> {
    let p = Path::new(path);
    let bytes = read_all(p)?;
    let ext = ext_of(p);
    match ext.as_str() {
        "heic" | "heif" | "hif" => decode_heic(&bytes),
        "tif" | "tiff" => decode_with_image_crate(&bytes, image::ImageFormat::Tiff),
        "icns" => decode_icns(&bytes),
        e if RAW_EXT.contains(&e) => decode_raw_preview(&bytes),
        _ => Err(format!("No decoder for .{ext} files")),
    }
}

fn encode_rgba(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    has_alpha: bool,
) -> Result<DecodedImage, String> {
    let img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "decoded buffer has the wrong size".to_string())?;
    let mut out = Vec::new();
    let mime = if has_alpha {
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        "image/png"
    } else {
        // Photos: JPEG keeps a 48 MP frame to a few MB instead of ~100 MB of PNG.
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
            .encode_image(&rgb)
            .map_err(|e| e.to_string())?;
        "image/jpeg"
    };
    Ok(DecodedImage {
        mime: mime.into(),
        data: B64.encode(&out),
        width,
        height,
        label: format!("{width}×{height}"),
    })
}

fn decode_heic(bytes: &[u8]) -> Result<ImagePreview, String> {
    let img = heifer::decode_srgb_rgba8(bytes, &heifer::Options::new())
        .map_err(|e| format!("HEIC decode failed: {e}"))?;
    let decoded = encode_rgba(img.width, img.height, img.data, img.has_alpha)?;
    Ok(ImagePreview {
        images: vec![decoded],
        note: None,
    })
}

fn decode_with_image_crate(
    bytes: &[u8],
    format: image::ImageFormat,
) -> Result<ImagePreview, String> {
    let img = image::load_from_memory_with_format(bytes, format).map_err(|e| e.to_string())?;
    let has_alpha = img.color().has_alpha();
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(ImagePreview {
        images: vec![encode_rgba(w, h, rgba.into_raw(), has_alpha)?],
        note: None,
    })
}
/// Extent and size of one embedded baseline/progressive JPEG.
#[derive(Debug, Clone, Copy)]
struct JpegSpan {
    start: usize,
    end: usize,
    width: u32,
    height: u32,
}

/// Walks a JPEG's marker segments from `start` (an FF D8) and returns its full
/// extent if it is a displayable (SOF0/1/2) image. Lossless SOF3 — the raw
/// sensor data inside CR2/DNG — is rejected.
fn parse_jpeg_at(b: &[u8], start: usize) -> Option<JpegSpan> {
    let mut i = start + 2;
    let mut dims = None;
    loop {
        if i >= b.len() || b[i] != 0xFF {
            return None;
        }
        while i < b.len() && b[i] == 0xFF {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        match marker {
            0xD9 => {
                let (width, height) = dims?;
                return Some(JpegSpan {
                    start,
                    end: i,
                    width,
                    height,
                });
            }
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let len = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        if len < 2 {
            return None;
        }
        match marker {
            0xC0..=0xC2 => {
                let h = u16::from_be_bytes([*b.get(i + 3)?, *b.get(i + 4)?]) as u32;
                let w = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]) as u32;
                dims = Some((w, h));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            _ => {}
        }
        i += len;
        if marker == 0xDA {
            // Entropy-coded data: runs until a marker that isn't a stuffed 00 or a restart.
            loop {
                let ff = i + b.get(i..)?.iter().position(|&x| x == 0xFF)?;
                let next = *b.get(ff + 1)?;
                if next == 0x00 || (0xD0..=0xD7).contains(&next) || next == 0xFF {
                    i = ff + 1;
                    continue;
                }
                i = ff;
                break;
            }
        }
    }
}

/// Every displayable JPEG embedded in `b`, found by scanning for SOI markers.
fn embedded_jpegs(b: &[u8]) -> Vec<JpegSpan> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == 0xFF && b[i + 1] == 0xD8 && b[i + 2] == 0xFF {
            if let Some(span) = parse_jpeg_at(b, i) {
                found.push(span);
            }
        }
        i += 1;
    }
    found
}

/// Camera RAW: every format embeds a camera-rendered JPEG preview (usually full
/// size). Show the largest one — a true RAW development would need LGPL code.
fn decode_raw_preview(bytes: &[u8]) -> Result<ImagePreview, String> {
    let best = embedded_jpegs(bytes)
        .into_iter()
        .max_by_key(|s| (s.width as u64) * (s.height as u64))
        .ok_or_else(|| "No embedded preview image found in this RAW file".to_string())?;
    Ok(ImagePreview {
        images: vec![DecodedImage {
            mime: "image/jpeg".into(),
            data: B64.encode(&bytes[best.start..best.end]),
            width: best.width,
            height: best.height,
            label: format!("{}×{}", best.width, best.height),
        }],
        note: Some(
            "Camera-embedded preview (as rendered by the camera), not a RAW development.".into(),
        ),
    })
}

/// Apple .icns: modern members (ic07–ic14, icp4–icp6) are PNGs; list each size.
fn decode_icns(bytes: &[u8]) -> Result<ImagePreview, String> {
    if bytes.len() < 8 || &bytes[0..4] != b"icns" {
        return Err("Not an ICNS file".into());
    }
    let mut images: Vec<DecodedImage> = Vec::new();
    let mut skipped_legacy = 0usize;
    let mut i = 8;
    while i + 8 <= bytes.len() {
        let kind = String::from_utf8_lossy(&bytes[i..i + 4]).to_string();
        let len =
            u32::from_be_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if len < 8 || i + len > bytes.len() {
            break;
        }
        let data = &bytes[i + 8..i + len];
        if data.starts_with(b"\x89PNG\r\n\x1a\n") && data.len() > 24 {
            let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
            let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
            if !images.iter().any(|im| im.width == w && im.height == h) {
                images.push(DecodedImage {
                    mime: "image/png".into(),
                    data: B64.encode(data),
                    width: w,
                    height: h,
                    label: format!("{w}×{h} ({kind})"),
                });
            }
        } else if kind != "TOC " && kind != "icnV" && kind != "name" && kind != "info" {
            skipped_legacy += 1;
        }
        i += len;
    }
    if images.is_empty() {
        return Err(
            "This ICNS only contains legacy (pre-PNG) icon data, which isn't supported".into(),
        );
    }
    images.sort_by_key(|im| std::cmp::Reverse(im.width));
    Ok(ImagePreview {
        images,
        note: (skipped_legacy > 0)
            .then(|| format!("{skipped_legacy} legacy/JPEG-2000 member(s) not shown.")),
    })
}

// ── Archives ────────────────────────────────────────────────────────────────

/// One file or directory inside an archive.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub compressed_size: Option<u64>,
    pub is_dir: bool,
}

/// Archive contents plus the detected container kind (`zip`, `tar`, `tar.gz`, `gz`).
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveListing {
    pub kind: String,
    pub entries: Vec<ArchiveEntry>,
}

/// Bytes of one archive member, base64-encoded.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveEntryContent {
    pub name: String,
    pub size: u64,
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    Gz,
}

/// Detects the container by magic bytes (extensions lie: .jar/.apk/.whl are zips).
fn archive_kind(path: &Path) -> Result<ArchiveKind, String> {
    let mut head = [0u8; 512];
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let n = f.read(&mut head).map_err(|e| e.to_string())?;
    if n >= 4 && (&head[0..4] == b"PK\x03\x04" || &head[0..4] == b"PK\x05\x06") {
        return Ok(ArchiveKind::Zip);
    }
    if n >= 2 && head[0] == 0x1F && head[1] == 0x8B {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut gz = flate2::read::GzDecoder::new(file);
        let mut inner = [0u8; 512];
        let mut got = 0;
        while got < inner.len() {
            match gz.read(&mut inner[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) => return Err(e.to_string()),
            }
        }
        return Ok(if got >= 262 && &inner[257..262] == b"ustar" {
            ArchiveKind::TarGz
        } else {
            ArchiveKind::Gz
        });
    }
    if n >= 262 && &head[257..262] == b"ustar" {
        return Ok(ArchiveKind::Tar);
    }
    Err("Unsupported archive format (supported: zip family, tar, tar.gz, gz)".into())
}

fn gz_inner_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "content".into())
}

fn tar_entries<R: Read>(reader: R) -> Result<Vec<ArchiveEntry>, String> {
    let mut ar = tar::Archive::new(reader);
    let mut out = Vec::new();
    for entry in ar.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        out.push(ArchiveEntry {
            is_dir: entry.header().entry_type().is_dir(),
            size: entry.header().size().unwrap_or(0),
            compressed_size: None,
            name,
        });
    }
    Ok(out)
}

/// Lists every member of a zip-family, tar, tar.gz or single-file gz archive.
pub fn archive_list(path: &str) -> Result<ArchiveListing, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;
    let kind = archive_kind(p)?;
    let open = || std::fs::File::open(p).map_err(|e| e.to_string());
    let (label, entries) = match kind {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(open()?).map_err(|e| e.to_string())?;
            let mut entries = Vec::with_capacity(zip.len());
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i).map_err(|e| e.to_string())?;
                entries.push(ArchiveEntry {
                    name: f
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| f.name_raw().iter().map(|&b| b as char).collect()),
                    size: f.size(),
                    compressed_size: Some(f.compressed_size()),
                    is_dir: f.is_dir(),
                });
            }
            ("zip", entries)
        }
        ArchiveKind::Tar => ("tar", tar_entries(open()?)?),
        ArchiveKind::TarGz => (
            "tar.gz",
            tar_entries(flate2::read::GzDecoder::new(open()?))?,
        ),
        ArchiveKind::Gz => {
            let mut gz = flate2::read::GzDecoder::new(open()?);
            let size = std::io::copy(&mut gz, &mut std::io::sink()).map_err(|e| e.to_string())?;
            let compressed = std::fs::metadata(p).map(|m| m.len()).ok();
            (
                "gz",
                vec![ArchiveEntry {
                    name: gz_inner_name(p),
                    size,
                    compressed_size: compressed,
                    is_dir: false,
                }],
            )
        }
    };
    Ok(ArchiveListing {
        kind: label.into(),
        entries,
    })
}

/// Reads one member's full bytes (no size cap) from an archive.
pub fn archive_read_entry(path: &str, entry: &str) -> Result<ArchiveEntryContent, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;
    let open = || std::fs::File::open(p).map_err(|e| e.to_string());
    let mut buf = Vec::new();
    match archive_kind(p)? {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(open()?).map_err(|e| e.to_string())?;
            let mut f = zip.by_name(entry).map_err(|e| e.to_string())?;
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        }
        kind @ (ArchiveKind::Tar | ArchiveKind::TarGz) => {
            let reader: Box<dyn Read> = if kind == ArchiveKind::Tar {
                Box::new(open()?)
            } else {
                Box::new(flate2::read::GzDecoder::new(open()?))
            };
            let mut ar = tar::Archive::new(reader);
            let mut found = false;
            for e in ar.entries().map_err(|e| e.to_string())? {
                let mut e = e.map_err(|e| e.to_string())?;
                if e.path().map_err(|e| e.to_string())?.to_string_lossy() == entry {
                    e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!("{entry} not found in archive"));
            }
        }
        ArchiveKind::Gz => {
            flate2::read::GzDecoder::new(open()?)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(ArchiveEntryContent {
        name: entry.to_string(),
        size: buf.len() as u64,
        data: B64.encode(&buf),
    })
}

// ── SQLite ──────────────────────────────────────────────────────────────────

/// One column of a table or view.
#[derive(Debug, Clone, Serialize)]
pub struct SqliteColumn {
    pub name: String,
    pub decl_type: String,
    pub pk: bool,
    pub not_null: bool,
}

/// A table or view with its columns and row count.
#[derive(Debug, Clone, Serialize)]
pub struct SqliteTable {
    pub name: String,
    pub kind: String,
    pub columns: Vec<SqliteColumn>,
    pub row_count: Option<i64>,
    pub sql: Option<String>,
}

/// One page of a table or query result. Values are JSON (blobs summarised).
#[derive(Debug, Clone, Serialize)]
pub struct SqliteRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub offset: i64,
    pub has_more: bool,
}

fn open_sqlite_ro(path: &str) -> Result<rusqlite::Connection, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let uri = |extra: &str| {
        let enc: String = path
            .chars()
            .map(|c| match c {
                '?' => "%3f".to_string(),
                '#' => "%23".to_string(),
                '%' => "%25".to_string(),
                '\\' => "/".to_string(),
                c => c.to_string(),
            })
            .collect();
        format!("file:{enc}?mode=ro{extra}")
    };
    let conn = rusqlite::Connection::open_with_flags(uri(""), flags)
        // A DB on a read-only volume (or another app holding it) can still be
        // read as an immutable snapshot.
        .or_else(|_| rusqlite::Connection::open_with_flags(uri("&immutable=1"), flags))
        .map_err(|e| e.to_string())?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|e| format!("Not a readable SQLite database: {e}"))?;
    Ok(conn)
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Tables and views of a SQLite file (read-only), with columns and row counts.
pub fn sqlite_schema(path: &str) -> Result<Vec<SqliteTable>, String> {
    let conn = open_sqlite_ro(path)?;
    let mut stmt = conn
        .prepare("SELECT name, type, sql FROM sqlite_master WHERE type IN ('table','view') ORDER BY type, name")
        .map_err(|e| e.to_string())?;
    let objects: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let mut out = Vec::with_capacity(objects.len());
    for (name, kind, sql) in objects {
        let mut cols = Vec::new();
        if let Ok(mut info) = conn.prepare(&format!("PRAGMA table_info({})", quote_ident(&name))) {
            if let Ok(rows) = info.query_map([], |r| {
                Ok(SqliteColumn {
                    name: r.get(1)?,
                    decl_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    not_null: r.get::<_, i64>(3)? != 0,
                    pk: r.get::<_, i64>(5)? != 0,
                })
            }) {
                cols = rows.filter_map(Result::ok).collect();
            }
        }
        let row_count = conn
            .query_row(
                &format!("SELECT count(*) FROM {}", quote_ident(&name)),
                [],
                |r| r.get(0),
            )
            .ok();
        out.push(SqliteTable {
            name,
            kind,
            columns: cols,
            row_count,
            sql,
        });
    }
    Ok(out)
}

fn sqlite_value(v: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::json!(i),
        ValueRef::Real(f) => serde_json::json!(f),
        ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("<blob {} bytes>", b.len())),
    }
}

/// One page of rows from `table`, or from a read-only `query` when given.
pub fn sqlite_rows(
    path: &str,
    table: Option<&str>,
    query: Option<&str>,
    offset: i64,
    page_size: i64,
) -> Result<SqliteRows, String> {
    let conn = open_sqlite_ro(path)?;
    let base = match (query.map(str::trim).filter(|q| !q.is_empty()), table) {
        (Some(q), _) => format!("SELECT * FROM ({})", q.trim_end_matches(';')),
        (None, Some(t)) => format!("SELECT * FROM {}", quote_ident(t)),
        (None, None) => return Err("table or query required".into()),
    };
    let page = page_size.clamp(1, 10_000);
    let sql = format!("{base} LIMIT {} OFFSET {}", page + 1, offset.max(0));
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let columns: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let ncol = columns.len();
    let mut rows = Vec::new();
    let mut q = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(r) = q.next().map_err(|e| e.to_string())? {
        let mut row = Vec::with_capacity(ncol);
        for i in 0..ncol {
            row.push(sqlite_value(r.get_ref(i).map_err(|e| e.to_string())?));
        }
        rows.push(row);
    }
    let has_more = rows.len() as i64 > page;
    rows.truncate(page as usize);
    Ok(SqliteRows {
        columns,
        rows,
        offset: offset.max(0),
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rustic-rich-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut out)
            .encode_image(&img)
            .unwrap();
        out
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn raw_picks_largest_embedded_jpeg_and_skips_lossless() {
        let mut raw = b"II*\0junk-tiff-header".to_vec();
        raw.extend(jpeg(160, 120)); // thumbnail
                                    // Lossless (SOF3) sensor data must be ignored even though it is a JPEG.
        raw.extend([
            0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0B, 8, 0, 10, 0, 10, 1, 1, 0x11, 0, 0xFF, 0xD9,
        ]);
        raw.extend(jpeg(1200, 800)); // full-size preview
        raw.extend(b"trailing sensor bytes");
        let dir = tmp("raw");
        let path = dir.join("shot.cr2");
        std::fs::write(&path, &raw).unwrap();
        let out = decode_image_preview(path.to_str().unwrap()).unwrap();
        assert_eq!((out.images[0].width, out.images[0].height), (1200, 800));
        let bytes = B64.decode(&out.images[0].data).unwrap();
        assert!(
            image::load_from_memory(&bytes).is_ok(),
            "extracted preview must decode"
        );
    }

    #[test]
    fn icns_lists_png_members_largest_first() {
        let mut body = Vec::new();
        for (kind, size) in [(*b"ic07", 128u32), (*b"ic09", 512)] {
            let data = png(size, size);
            body.extend(kind);
            body.extend(((data.len() + 8) as u32).to_be_bytes());
            body.extend(data);
        }
        let mut icns = b"icns".to_vec();
        icns.extend(((body.len() + 8) as u32).to_be_bytes());
        icns.extend(body);
        let dir = tmp("icns");
        let path = dir.join("App.icns");
        std::fs::write(&path, icns).unwrap();
        let out = decode_image_preview(path.to_str().unwrap()).unwrap();
        let sizes: Vec<u32> = out.images.iter().map(|i| i.width).collect();
        assert_eq!(sizes, vec![512, 128]);
    }

    #[test]
    fn tiff_decodes_through_image_crate() {
        let img = image::RgbImage::from_pixel(64, 32, image::Rgb([10, 200, 10]));
        let dir = tmp("tiff");
        let path = dir.join("scan.tiff");
        img.save_with_format(&path, image::ImageFormat::Tiff)
            .unwrap();
        let out = decode_image_preview(path.to_str().unwrap()).unwrap();
        assert_eq!((out.images[0].width, out.images[0].height), (64, 32));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn heic_from_apples_encoder_decodes() {
        let dir = tmp("heic");
        let src = dir.join("src.jpg");
        std::fs::write(&src, jpeg(640, 480)).unwrap();
        let out = dir.join("photo.heic");
        let ok = std::process::Command::new("sips")
            .args(["-s", "format", "heic"])
            .arg(&src)
            .arg("--out")
            .arg(&out)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("sips unavailable — skipping");
            return;
        }
        let res = decode_image_preview(out.to_str().unwrap()).unwrap();
        assert_eq!((res.images[0].width, res.images[0].height), (640, 480));
        let px = image::load_from_memory(&B64.decode(&res.images[0].data).unwrap())
            .unwrap()
            .to_rgb8();
        let p = px.get_pixel(320, 240);
        assert!(
            p[0] > 150 && p[1] < 80 && p[2] < 80,
            "colour preserved, got {p:?}"
        );
    }

    #[test]
    fn zip_tar_gz_list_and_read() {
        let dir = tmp("arch");
        let zp = dir.join("pkg.jar");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            z.start_file(
                "META-INF/MANIFEST.MF",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(b"Main-Class: App\n").unwrap();
            z.finish().unwrap();
        }
        let l = archive_list(zp.to_str().unwrap()).unwrap();
        assert_eq!(l.kind, "zip");
        let c = archive_read_entry(zp.to_str().unwrap(), "META-INF/MANIFEST.MF").unwrap();
        assert_eq!(B64.decode(c.data).unwrap(), b"Main-Class: App\n");

        let tp = dir.join("src.tgz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&tp).unwrap(),
                flate2::Compression::default(),
            );
            let mut t = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(5);
            h.set_mode(0o644);
            h.set_cksum();
            t.append_data(&mut h, "dir/a.txt", &b"hello"[..]).unwrap();
            t.into_inner().unwrap().finish().unwrap();
        }
        let l = archive_list(tp.to_str().unwrap()).unwrap();
        assert_eq!(l.kind, "tar.gz");
        assert_eq!(l.entries[0].name, "dir/a.txt");
        let c = archive_read_entry(tp.to_str().unwrap(), "dir/a.txt").unwrap();
        assert_eq!(B64.decode(c.data).unwrap(), b"hello");

        let gp = dir.join("notes.txt.gz");
        {
            let mut gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&gp).unwrap(),
                flate2::Compression::default(),
            );
            gz.write_all(b"plain gz body").unwrap();
            gz.finish().unwrap();
        }
        let l = archive_list(gp.to_str().unwrap()).unwrap();
        assert_eq!(
            (
                l.kind.as_str(),
                l.entries[0].name.as_str(),
                l.entries[0].size
            ),
            ("gz", "notes.txt", 13)
        );
    }

    #[test]
    fn sqlite_schema_rows_and_readonly_query() {
        let dir = tmp("sql");
        let path = dir.join("app.sqlite");
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT NOT NULL, avatar BLOB);
                 INSERT INTO users(name, avatar) VALUES ('ada', x'0102'), ('bob', NULL), ('cy', NULL);",
            )
            .unwrap();
        }
        let p = path.to_str().unwrap();
        let schema = sqlite_schema(p).unwrap();
        assert_eq!(schema[0].name, "users");
        assert_eq!(schema[0].row_count, Some(3));
        assert!(schema[0].columns[0].pk);
        let page = sqlite_rows(p, Some("users"), None, 0, 2).unwrap();
        assert_eq!(page.rows.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.rows[0][2], serde_json::json!("<blob 2 bytes>"));
        let q = sqlite_rows(p, None, Some("select name from users where id > 1"), 0, 50).unwrap();
        assert_eq!(q.rows.len(), 2);
        assert!(
            sqlite_rows(p, None, Some("delete from users"), 0, 50).is_err(),
            "writes must fail"
        );
    }
}
