//! `rustic-media://` — streams local audio files to the webview with HTTP Range
//! support, so large previews play from disk instead of being base64-encoded
//! over IPC (which is capped at 100 MB and fully decoded in memory).
//!
//! Deliberately narrow: audio extensions only, the shared readable-path guard,
//! and nothing under Rustic's own app-data directory.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tauri::http::{header, Request, Response, StatusCode};

pub const SCHEME: &str = "rustic-media";

const AUDIO_EXT: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "weba", "aiff", "aif", "wma",
];
/// Largest slice served per request; media elements ask for the next range as they play.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

/// Serve one `rustic-media://localhost/<encoded path>` request.
pub fn handle(app: &tauri::AppHandle, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    serve(app, req).unwrap_or_else(|(status, msg)| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(msg.into_bytes())
            .unwrap_or_default()
    })
}

fn serve(
    app: &tauri::AppHandle,
    req: &Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, (StatusCode, String)> {
    let raw = req.uri().path().trim_start_matches('/');
    let path = PathBuf::from(
        percent_decode(raw).ok_or((StatusCode::BAD_REQUEST, "bad path encoding".into()))?,
    );
    let mime = audio_mime(&path).ok_or((
        StatusCode::FORBIDDEN,
        "only audio files can be streamed".into(),
    ))?;
    rustic_app::path_scope::validate_readable_path(&path)
        .map_err(|e| (StatusCode::FORBIDDEN, e))?;
    deny_app_data(app, &path)?;

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
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
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

/// MIME type for a whitelisted audio extension.
fn audio_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !AUDIO_EXT.contains(&ext.as_str()) {
        return None;
    }
    Some(match ext.as_str() {
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
        _ => "application/octet-stream",
    })
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
        assert_eq!(audio_mime(Path::new("/a/B.MP3")), Some("audio/mpeg"));
        assert!(audio_mime(Path::new("/a/secrets.json")).is_none());
    }
}
