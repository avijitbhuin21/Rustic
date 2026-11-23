//! Request handling for one film session: path resolution, headers, and the
//! small edits made to served text (bridge script, root-absolute URLs).

use std::path::{Path, PathBuf};

use super::{Kind, Layout, Session, SCHEME};

pub(super) const SHELL: &str = "@rustic/shell";
const BRIDGE: &str = "@rustic/bridge.js";
const BRIDGE_JS: &str = include_str!("bridge.js");
const PAGE_BRIDGE_JS: &str = include_str!("page-bridge.js");

/// Largest file a session serves; a page has no business pulling more.
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// Largest text file that gets its root-absolute URLs re-pointed.
const MAX_REWRITE: usize = 8 * 1024 * 1024;

/// A response for a film request, independent of the host's HTTP types.
#[derive(Debug)]
pub struct FilmResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub csp: String,
    pub body: Vec<u8>,
}

impl FilmResponse {
    /// A plain-text error that is itself sandboxed.
    pub fn error(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            csp: "default-src 'none'; sandbox".into(),
            body: message.as_bytes().to_vec(),
        }
    }

    /// Headers every host must send with this response.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("content-type", self.content_type.to_string()),
            ("content-security-policy", self.csp.clone()),
            // The frame has an opaque origin, so its own scripts and fetches are cross-origin.
            ("access-control-allow-origin", "*".into()),
            ("cache-control", "no-store".into()),
            ("x-content-type-options", "nosniff".into()),
            ("referrer-policy", "no-referrer".into()),
        ]
    }
}

pub(super) fn respond(id: &str, s: &Session, rel: &str, layout: Layout<'_>) -> FilmResponse {
    let (source, prefix) = match layout {
        Layout::Scheme => (format!("{SCHEME}://{id}"), String::new()),
        Layout::Path { host } => {
            let ok = !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b));
            if !ok {
                return FilmResponse::error(400, "Bad Host header.");
            }
            (format!("{host}/film/{id}/"), format!("/film/{id}"))
        }
    };
    let rel = rel.trim_start_matches('/');
    if rel == SHELL {
        // A page without its own background shows the shell's: white like a browser, black for films.
        let bg = if s.kind == Kind::Page { "#fff" } else { "#000" };
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;height:100%;overflow:hidden;background:{bg}}}iframe{{border:0;width:100%;height:100%;display:block}}</style></head><body><iframe sandbox=\"allow-scripts\" src=\"{prefix}/{}\"></iframe></body></html>",
            s.entry
        );
        return FilmResponse {
            status: 200,
            content_type: "text/html; charset=utf-8",
            csp: format!("default-src 'none'; style-src 'unsafe-inline'; frame-src {source}; base-uri 'none'; form-action 'none'; sandbox allow-scripts"),
            body: html.into_bytes(),
        };
    }
    let csp = match s.kind {
        Kind::Film => page_csp(&source),
        Kind::Page => live_page_csp(&source),
    };
    if rel == BRIDGE {
        return FilmResponse {
            status: 200,
            content_type: "text/javascript; charset=utf-8",
            csp,
            body: match s.kind {
                Kind::Film => BRIDGE_JS,
                Kind::Page => PAGE_BRIDGE_JS,
            }
            .as_bytes()
            .to_vec(),
        };
    }
    let path = match resolve(&s.root, rel, &s.deny) {
        Ok(p) => p,
        Err((status, msg)) => return FilmResponse::error(status, msg),
    };
    match std::fs::metadata(&path) {
        Ok(m) if m.len() > MAX_FILE => {
            return FilmResponse::error(413, "File too large for the film view.")
        }
        Ok(_) => {}
        Err(_) => return FilmResponse::error(404, "Not found."),
    }
    let Ok(mut body) = std::fs::read(&path) else {
        return FilmResponse::error(404, "Not found.");
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_html = matches!(ext.as_str(), "html" | "htm");
    let is_css = ext == "css";
    if !prefix.is_empty()
        && body.len() <= MAX_REWRITE
        && (is_html || is_css || matches!(ext.as_str(), "js" | "mjs"))
    {
        body = rewrite_absolute(&body, &s.names, &prefix, is_css);
    }
    if is_css && s.kind == Kind::Page && body.len() <= MAX_REWRITE {
        body = freeze_viewport_units(&body);
    }
    if is_html {
        body = inject_bridge(&body, &format!("{prefix}/{BRIDGE}"));
    }
    // Only pages and scripts (workers) get a policy that lets script run. An SVG or XML
    // file opened as a document would otherwise be a script realm without the bridge.
    let csp = if is_html || matches!(ext.as_str(), "js" | "mjs") {
        csp
    } else {
        "default-src 'none'; sandbox".into()
    };
    FilmResponse {
        status: 200,
        content_type: mime(&ext),
        csp,
        body,
    }
}

/// CSP for everything a film page loads: fetches limited to the session, sandboxed document.
/// It does NOT stop WebRTC: Chromium ignores the `webrtc` directive (kept for engines that
/// honour it) and a child frame gets fresh constructors. The film view's per-project
/// prompt is the gate for that.
fn page_csp(s: &str) -> String {
    format!(
        "default-src 'none'; script-src {s} 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob:; \
         style-src {s} 'unsafe-inline'; img-src {s} data: blob:; font-src {s} data:; \
         media-src {s} data: blob:; connect-src {s} data: blob:; worker-src {s} blob:; \
         frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; \
         webrtc 'block'; sandbox allow-scripts"
    )
}

/// CSP for a page in the live HTML preview. Like `page_csp`, but pages may load scripts,
/// styles, fonts and media from `https:` and connect out, because real sites lean on CDNs.
/// The preview's per-project prompt says so: such a page can send what it reads anywhere.
fn live_page_csp(s: &str) -> String {
    format!(
        "default-src 'none'; script-src {s} https: 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob:; \
         style-src {s} https: 'unsafe-inline'; img-src {s} https: data: blob:; font-src {s} https: data:; \
         media-src {s} https: data: blob:; connect-src {s} https: wss: data: blob:; worker-src {s} blob:; \
         frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; \
         webrtc 'block'; sandbox allow-scripts"
    )
}

const FROZEN_VH: &[u8] = b"var(--rustic-vh, 1vh)";
const FROZEN_VMIN: &[u8] = b"min(1vw, var(--rustic-vh, 1vh))";
const FROZEN_VMAX: &[u8] = b"max(1vw, var(--rustic-vh, 1vh))";

/// Point viewport-height units (`vh`, `dvh`/`svh`/`lvh`, `vmin`, `vmax`) at `--rustic-vh`,
/// which the page bridge pins at the artboard's device height. The live preview grows a
/// frame to its page's full height, so `100vh` would otherwise grow with it, forever.
/// Comments, strings, `url()` and at-rule preludes (media queries can't hold `var()`) are
/// left alone.
fn freeze_viewport_units(css: &[u8]) -> Vec<u8> {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80;
    let digit = |i: usize| css.get(i).is_some_and(u8::is_ascii_digit);
    let n = css.len();
    let mut out = Vec::with_capacity(n + 256);
    let mut i = 0;
    let mut stmt_start = true;
    let mut prelude = false;
    while i < n {
        let b = css[i];
        if b == b'/' && css.get(i + 1) == Some(&b'*') {
            let end = css[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(n, |p| i + 2 + p + 2);
            out.extend_from_slice(&css[i..end]);
            i = end;
            continue;
        }
        if b == b'"' || b == b'\'' {
            let mut j = i + 1;
            while j < n && css[j] != b {
                j += if css[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(n);
            out.extend_from_slice(&css[i..end]);
            i = end;
            stmt_start = false;
            continue;
        }
        if n - i >= 4
            && css[i..i + 4].eq_ignore_ascii_case(b"url(")
            && (i == 0 || !ident(css[i - 1]))
        {
            let end = css[i..]
                .iter()
                .position(|&c| c == b')')
                .map_or(n, |p| i + p + 1);
            out.extend_from_slice(&css[i..end]);
            i = end;
            stmt_start = false;
            continue;
        }
        if matches!(b, b'{' | b'}' | b';') {
            prelude = false;
            stmt_start = true;
            out.push(b);
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            out.push(b);
            i += 1;
            continue;
        }
        if stmt_start {
            prelude = b == b'@';
            stmt_start = false;
        }
        let unsigned = b.is_ascii_digit() || (b == b'.' && digit(i + 1));
        let signed = (b == b'-' || b == b'+')
            && (digit(i + 1) || (css.get(i + 1) == Some(&b'.') && digit(i + 2)));
        let after_boundary = i == 0 || !(ident(css[i - 1]) || matches!(css[i - 1], b'.' | b'#'));
        if prelude || !(unsigned || signed) || !after_boundary {
            out.push(b);
            i += 1;
            continue;
        }
        let mut j = i + usize::from(signed);
        while digit(j) {
            j += 1;
        }
        if css.get(j) == Some(&b'.') && digit(j + 1) {
            j += 1;
            while digit(j) {
                j += 1;
            }
        }
        if matches!(css.get(j), Some(b'e' | b'E')) {
            let k = j + 1 + usize::from(matches!(css.get(j + 1), Some(b'+' | b'-')));
            if digit(k) {
                j = k;
                while digit(j) {
                    j += 1;
                }
            }
        }
        let mut u = j;
        if matches!(
            css.get(u).map(u8::to_ascii_lowercase),
            Some(b'd' | b's' | b'l')
        ) {
            u += 1;
        }
        let rest = &css[u.min(n)..];
        let starts = |p: &[u8]| rest.len() >= p.len() && rest[..p.len()].eq_ignore_ascii_case(p);
        let unit = if starts(b"vmin") {
            Some((4, FROZEN_VMIN))
        } else if starts(b"vmax") {
            Some((4, FROZEN_VMAX))
        } else if starts(b"vh") {
            Some((2, FROZEN_VH))
        } else {
            None
        };
        match unit {
            Some((len, frozen)) if !css.get(u + len).is_some_and(|&c| ident(c)) => {
                out.extend_from_slice(b"calc(");
                out.extend_from_slice(&css[i..j]);
                out.extend_from_slice(b" * ");
                out.extend_from_slice(frozen);
                out.push(b')');
                i = u + len;
            }
            _ => {
                out.extend_from_slice(&css[i..j]);
                i = j;
            }
        }
    }
    out
}
pub(super) fn resolve(
    root: &Path,
    rel: &str,
    deny: &[PathBuf],
) -> Result<PathBuf, (u16, &'static str)> {
    if rel.contains(['\0', '\\', ':']) {
        return Err((400, "Bad path."));
    }
    let mut p = root.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        if seg.starts_with('.') {
            return Err((403, "Hidden files and folders are not served."));
        }
        p.push(seg);
    }
    let canon = std::fs::canonicalize(&p).map_err(|_| (404, "Not found."))?;
    let inside = canon
        .strip_prefix(root)
        .map_err(|_| (403, "Outside the served folder."))?;
    let hidden = inside
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'));
    if hidden {
        return Err((403, "Hidden files and folders are not served."));
    }
    if !canon.is_file() {
        return Err((404, "Not found."));
    }
    if crate::path_scope::validate_readable_path(&canon).is_err()
        || deny.iter().any(|d| canon.starts_with(d))
    {
        return Err((403, "Not served."));
    }
    Ok(canon)
}

/// Re-point quoted root-absolute URLs (`"/core/lib.js"`) at the session prefix.
/// Only URLs whose first segment is a real top-level name in the served folder are touched.
fn rewrite_absolute(body: &[u8], names: &[String], prefix: &str, css: bool) -> Vec<u8> {
    let ends = |c: &u8| {
        matches!(
            c,
            b'/' | b'"'
                | b'\''
                | b'`'
                | b')'
                | b'?'
                | b'#'
                | b' '
                | b'\n'
                | b'\r'
                | b'\t'
                | b'<'
                | b'>'
        )
    };
    let mut out = Vec::with_capacity(body.len() + 1024);
    for (i, &b) in body.iter().enumerate() {
        out.push(b);
        let opens = matches!(b, b'"' | b'\'' | b'`') || (css && b == b'(');
        if !opens || body.get(i + 1) != Some(&b'/') {
            continue;
        }
        let rest = &body[i + 2..];
        let window = &rest[..rest.len().min(128)];
        let seg = &window[..window.iter().position(ends).unwrap_or(window.len())];
        if !seg.is_empty() && names.iter().any(|n| n.as_bytes() == seg) {
            out.extend_from_slice(prefix.as_bytes());
        }
    }
    out
}

/// Insert the bridge script ahead of everything except a leading doctype, so no page
/// script can run before it (a script placed before `<head>` must not get there first).
fn inject_bridge(html: &[u8], src: &str) -> Vec<u8> {
    let lead = if html.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let first = html[lead..]
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(html.len(), |i| lead + i);
    let rest = &html[first..];
    let at = if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"<!doctype") {
        rest.iter()
            .position(|&b| b == b'>')
            .map_or(lead, |i| first + i + 1)
    } else {
        lead
    };
    let tag = format!("<script src=\"{src}\"></script>");
    let mut out = Vec::with_capacity(html.len() + tag.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(tag.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

fn mime(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "txt" | "md" | "csv" | "glsl" | "vert" | "frag" | "srt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Percent-encode one path segment.
pub(super) fn percent_encode(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` escapes in a URL path; `None` when the escapes or the UTF-8 are invalid.
pub fn percent_decode(s: &str) -> Option<String> {
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
pub(super) fn rewrite_for_test(body: &str, names: &[&str], css: bool) -> String {
    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    String::from_utf8(rewrite_absolute(body.as_bytes(), &names, "/film/ID", css)).unwrap()
}

#[cfg(test)]
pub(super) fn freeze_for_test(css: &str) -> String {
    String::from_utf8(freeze_viewport_units(css.as_bytes())).unwrap()
}
