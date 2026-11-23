//! `rustic-media://` — streams local files to the webview with HTTP Range
//! support, so previews of any size load from disk instead of being
//! base64-encoded over IPC (capped at 100 MB and fully held in memory).
//! Mirrors the web server's `/api/asset`.
//!
//! Guarded by the shared readable-path check and nothing under Rustic's own
//! app-data directory; served to the app window and origin only (see
//! `reader_origin`); responses carry `sandbox` CSP so a navigated SVG/HTML
//! asset can't run script in the app origin.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tauri::http::{header, Request, Response, StatusCode};

pub const SCHEME: &str = "rustic-media";

/// Largest slice served per request; callers ask for the next range as they read.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

/// The only window allowed to stream local files.
const APP_WINDOW: &str = "main";

/// Origins the app's own page is served from (macOS/Linux, then Windows).
const APP_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

/// Whether `origin` is the app's own page: a bundled origin, or the dev server in `tauri dev`.
fn is_app_origin(app: &tauri::AppHandle, origin: &str) -> bool {
    if APP_ORIGINS.contains(&origin) {
        return true;
    }
    app.config()
        .build
        .dev_url
        .as_ref()
        .is_some_and(|dev| dev.origin().ascii_serialization() == origin)
}

/// Who may read a response: `Ok(value)` for `Access-Control-Allow-Origin`, or
/// `Err` when the request must be refused.
///
/// A remote-backend window loads a page from another machine and must not read
/// this machine's files, so only the app window is served. Within it, a
/// cross-origin `fetch` is answered only for the app's own origin: a sandboxed
/// preview frame sends `Origin: null` and is refused. Requests without an
/// `Origin` (`<img>`, `<video>`, fonts) cannot expose bytes to script.
fn reader_origin(
    app: &tauri::AppHandle,
    webview: &str,
    req: &Request<Vec<u8>>,
) -> Result<String, (StatusCode, String)> {
    if webview != APP_WINDOW {
        return Err((
            StatusCode::FORBIDDEN,
            "local files are not served to this window".into(),
        ));
    }
    match req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        None => Ok("*".into()),
        Some(origin) if is_app_origin(app, origin) => Ok(origin.to_string()),
        Some(_) => Err((
            StatusCode::FORBIDDEN,
            "local files are not served to this origin".into(),
        )),
    }
}

/// Serve one `rustic-media://localhost/<encoded path>` request made by webview `webview`.
pub fn handle(app: &tauri::AppHandle, webview: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let failure = |status: StatusCode, msg: String, allow: Option<&str>| {
        let mut builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain");
        if let Some(origin) = allow {
            builder = builder
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
                .header(header::VARY, "Origin");
        }
        builder.body(msg.into_bytes()).unwrap_or_default()
    };
    let allow = match reader_origin(app, webview, req) {
        Ok(origin) => origin,
        Err((status, msg)) => return failure(status, msg, None),
    };
    serve(app, req, &allow).unwrap_or_else(|(status, msg)| failure(status, msg, Some(&allow)))
}

fn serve(
    app: &tauri::AppHandle,
    req: &Request<Vec<u8>>,
    allow_origin: &str,
) -> Result<Response<Vec<u8>>, (StatusCode, String)> {
    let raw = req.uri().path().trim_start_matches('/');
    let path = PathBuf::from(
        percent_decode(raw).ok_or((StatusCode::BAD_REQUEST, "bad path encoding".into()))?,
    );
    let mime = media_mime(&path);
    rustic_app::path_scope::validate_readable_path(&path)
        .map_err(|e| (StatusCode::FORBIDDEN, e))?;
    deny_app_data(app, &path)?;
    if path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "path is a directory".into()));
    }

    let mut file =
        std::fs::File::open(&path).map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let total = file
        .metadata()
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?
        .len();
    if total == 0 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .body(Vec::new())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    if range.is_none() {
        // Plain requests (<img>, fonts, iframes) never send Range — give them
        // the whole file; a 4 MB partial would silently truncate big assets.
        let mut buf = Vec::with_capacity(total as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total.to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin)
            .header(header::VARY, "Origin")
            .header(header::CONTENT_SECURITY_POLICY, "sandbox")
            .body(buf)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    let (start, end) = parse_range(range, total).ok_or((
        StatusCode::RANGE_NOT_SATISFIABLE,
        format!("bytes */{total}"),
    ))?;
    let end = end.min(start + MAX_CHUNK - 1);
    let len = end - start + 1;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        )
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin)
        .header(header::VARY, "Origin")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Range, Content-Length",
        )
        .header(header::CONTENT_SECURITY_POLICY, "sandbox")
        .body(buf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Refuse anything inside Rustic's own data directory (DB, secrets, history).
fn deny_app_data(app: &tauri::AppHandle, path: &Path) -> Result<(), (StatusCode, String)> {
    let Ok(data) = crate::app_paths::app_data_dir(app) else {
        return Ok(());
    };
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(path).starts_with(canon(&data)) {
        return Err((
            StatusCode::FORBIDDEN,
            "Rustic's internal data can't be streamed".into(),
        ));
    }
    Ok(())
}

/// MIME type guessed from the extension; unknown types are served as octet-stream.
fn media_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "weba" => "audio/webm",
        "aiff" | "aif" => "audio/aiff",
        "wma" => "audio/x-ms-wma",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        "avi" => "video/x-msvideo",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "pdf" => "application/pdf",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "stl" => "model/stl",
        "json" | "jsonl" | "ndjson" => "application/json",
        "txt" | "csv" | "tsv" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        _ => "application/octet-stream",
    }
}

/// Inclusive `(start, end)` for a single `bytes=` range; a missing header means from 0.
fn parse_range(header: Option<&str>, total: u64) -> Option<(u64, u64)> {
    let last = total - 1;
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Some((0, last));
    };
    let spec = spec.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (total.saturating_sub(n), last)
        }
        (s, "") => (s.parse().ok()?, last),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(last)),
    };
    (start <= end && start <= last).then_some((start, end))
}

/// Decode `%XX` escapes (the path segment `convertFileSrc` produces).
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range(None, 100), Some((0, 99)));
        assert_eq!(parse_range(Some("bytes=10-"), 100), Some((10, 99)));
        assert_eq!(parse_range(Some("bytes=10-19"), 100), Some((10, 19)));
        assert_eq!(parse_range(Some("bytes=-5"), 100), Some((95, 99)));
        assert_eq!(parse_range(Some("bytes=10-500"), 100), Some((10, 99)));
        assert_eq!(parse_range(Some("bytes=200-"), 100), None);
    }

    #[test]
    fn decodes_paths_and_gates_extensions() {
        assert_eq!(
            percent_decode("%2FUsers%2Fa%20b%2Fsong.mp3").as_deref(),
            Some("/Users/a b/song.mp3")
        );
        assert_eq!(
            percent_decode("C%3A%5Cmusic%5Cx.wav").as_deref(),
            Some("C:\\music\\x.wav")
        );
        assert!(percent_decode("%zz").is_none());
        assert_eq!(media_mime(Path::new("/a/B.MP3")), "audio/mpeg");
        assert_eq!(media_mime(Path::new("/a/clip.MOV")), "video/quicktime");
        assert_eq!(media_mime(Path::new("/a/clip.mp4")), "video/mp4");
        assert_eq!(media_mime(Path::new("/a/scan.pdf")), "application/pdf");
        assert_eq!(
            media_mime(Path::new("/a/data.bin")),
            "application/octet-stream"
        );
    }
}
