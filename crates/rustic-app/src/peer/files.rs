//! Per-file browsing and transfer between paired devices: list a shared
//! project's folders, preview one file on demand, measure a selection,
//! and pack / unpack selections as a zstd tar with name-conflict handling.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Largest text file returned whole by [`read_preview`].
pub const MAX_TEXT_PREVIEW: u64 = 2 * 1024 * 1024;
/// Largest image returned (base64) by [`read_preview`].
pub const MAX_IMAGE_PREVIEW: u64 = 12 * 1024 * 1024;

/// A project-relative path checked to stay inside the project.
pub fn safe_rel(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path.trim_matches('/'));
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return Err(format!("invalid path: {path}")),
        }
    }
    Ok(out)
}

/// Local root folder of project `project_id`.
pub fn project_root(state: &crate::state::AppState, project_id: &str) -> Result<PathBuf, String> {
    let ws = state.workspace.lock().map_err(|_| "workspace unavailable".to_string())?;
    ws.list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .map(|p| p.root_path.clone())
        .ok_or_else(|| "That project doesn't exist on this machine".to_string())
}

/// One entry of a folder listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEntry {
    pub name: String,
    /// Project-relative, `/`-separated.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: Option<i64>,
}

/// `/`-joined relative path.
fn rel_string(p: &Path) -> String {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Folders hidden from browsing (never transferred implicitly either).
fn hidden(name: &str) -> bool {
    name == ".git"
}

/// Direct children of `rel` under `root`, folders first, then by name.
pub fn list_dir(root: &Path, rel: &str) -> Result<Vec<FsEntry>, String> {
    let rel_path = safe_rel(rel)?;
    let dir = root.join(&rel_path);
    let rd = std::fs::read_dir(&dir).map_err(|e| format!("Can't open {}: {e}", rel_string(&rel_path)))?;
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        out.push(FsEntry {
            path: rel_string(&rel_path.join(&name)),
            name,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64),
        });
    }
    out.sort_by(|a, b| (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase())));
    Ok(out)
}

/// What [`read_preview`] returns for one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub path: String,
    pub size: u64,
    /// `"text"`, `"image"`, `"binary"` or `"too_large"`.
    pub kind: String,
    /// UTF-8 text, or base64 for images.
    pub content: Option<String>,
    pub mime: Option<String>,
}

/// Image MIME type by extension.
fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// Load one file for previewing (text whole, images as base64; large or
/// binary files report only their size).
pub fn read_preview(root: &Path, rel: &str) -> Result<Preview, String> {
    use base64::Engine;
    let rel_path = safe_rel(rel)?;
    let path = root.join(&rel_path);
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("That's a folder".into());
    }
    let size = meta.len();
    let base = Preview { path: rel_string(&rel_path), size, kind: "binary".into(), content: None, mime: None };
    if let Some(mime) = image_mime(&path) {
        if size > MAX_IMAGE_PREVIEW {
            return Ok(Preview { kind: "too_large".into(), mime: Some(mime.into()), ..base });
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        return Ok(Preview {
            kind: "image".into(),
            content: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            mime: Some(mime.into()),
            ..base
        });
    }
    if size > MAX_TEXT_PREVIEW {
        return Ok(Preview { kind: "too_large".into(), ..base });
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Ok(base);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Preview { kind: "text".into(), content: Some(text), ..base }),
        Err(_) => Ok(base),
    }
}

/// Total size of a selection.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SizeInfo {
    pub bytes: u64,
    pub files: u64,
}

/// Recursively measure `abs` (file or folder), skipping hidden folders.
fn measure_path(abs: &Path, out: &mut SizeInfo) {
    let Ok(meta) = std::fs::symlink_metadata(abs) else { return };
    if meta.is_file() {
        out.bytes += meta.len();
        out.files += 1;
    } else if meta.is_dir() {
        if let Ok(rd) = std::fs::read_dir(abs) {
            for e in rd.flatten() {
                if hidden(&e.file_name().to_string_lossy()) {
                    continue;
                }
                measure_path(&e.path(), out);
            }
        }
    }
}

/// Total bytes and file count of `items` (absolute paths).
pub fn measure(items: &[PathBuf]) -> SizeInfo {
    let mut out = SizeInfo::default();
    for p in items {
        measure_path(p, &mut out);
    }
    out
}

/// One thing to pack: absolute source path and its top-level name in the archive.
pub struct PackItem {
    pub abs: PathBuf,
    pub name: String,
}

/// Recursively append `abs` as `name` to the tar.
fn append_path<W: Write>(tar: &mut tar::Builder<W>, abs: &Path, name: &Path, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    if cancel() {
        return Err("cancelled".into());
    }
    let meta = std::fs::symlink_metadata(abs).map_err(|e| e.to_string())?;
    if meta.is_file() {
        let mut f = std::fs::File::open(abs).map_err(|e| e.to_string())?;
        tar.append_file(name, &mut f).map_err(|e| e.to_string())?;
    } else if meta.is_dir() {
        tar.append_dir(name, abs).map_err(|e| e.to_string())?;
        let rd = std::fs::read_dir(abs).map_err(|e| e.to_string())?;
        for e in rd.flatten() {
            let child = e.file_name();
            if hidden(&child.to_string_lossy()) {
                continue;
            }
            append_path(tar, &e.path(), &name.join(&child), cancel)?;
        }
    }
    Ok(())
}

/// Write `items` as a zstd-compressed tar into `sink`. `cancel` is polled
/// between files; `progress` gets the cumulative uncompressed bytes written
/// (≈ file bytes, comparable with [`measure`]).
pub fn pack(
    items: &[PackItem],
    sink: impl Write,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let enc = zstd::stream::write::Encoder::new(sink, 1).map_err(|e| e.to_string())?;
    let counted = Counting { inner: enc, n: 0, progress };
    let mut tar = tar::Builder::new(counted);
    tar.follow_symlinks(false);
    for it in items {
        append_path(&mut tar, &it.abs, Path::new(&it.name), cancel)?;
    }
    let counted = tar.into_inner().map_err(|e| e.to_string())?;
    counted.inner.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())
}

/// Reader / writer wrapper reporting cumulative bytes.
struct Counting<'a, T> {
    inner: T,
    n: u64,
    progress: &'a dyn Fn(u64),
}

impl<T: Write> Write for Counting<'_, T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let k = self.inner.write(buf)?;
        self.n += k as u64;
        (self.progress)(self.n);
        Ok(k)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<T: Read> Read for Counting<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n += k as u64;
        (self.progress)(self.n);
        Ok(k)
    }
}

/// `Write` end feeding an async byte stream (for streaming HTTP bodies from
/// blocking packers). Buffers ~256 KiB per message.
pub struct ChannelWriter {
    tx: tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>,
    buf: Vec<u8>,
}

const CHANNEL_CHUNK: usize = 256 * 1024;

impl ChannelWriter {
    /// Writer + the receiving end to turn into a body stream.
    pub fn new() -> (Self, tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, std::io::Error>>) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        (Self { tx, buf: Vec::with_capacity(CHANNEL_CHUNK) }, rx)
    }

    /// Send an error to the reader (aborts the HTTP body).
    pub fn fail(&self, msg: &str) {
        let _ = self.tx.blocking_send(Err(std::io::Error::other(msg.to_string())));
    }

    fn send_buf(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = axum::body::Bytes::from(std::mem::replace(&mut self.buf, Vec::with_capacity(CHANNEL_CHUNK)));
        self.tx
            .blocking_send(Ok(chunk))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHANNEL_CHUNK {
            self.send_buf()?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.send_buf()
    }
}

impl Drop for ChannelWriter {
    fn drop(&mut self) {
        let _ = self.send_buf();
    }
}

/// Byte stream for an HTTP body from a [`ChannelWriter`]'s receiver.
pub fn channel_stream(
    rx: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, std::io::Error>>,
) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) })
}

/// `Read` end over an async byte stream (for unpacking streamed HTTP bodies
/// in a blocking thread).
pub struct ChannelReader {
    rx: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, String>>,
    cur: axum::body::Bytes,
}

impl ChannelReader {
    /// Reader + the sender the async side pushes chunks into.
    pub fn new() -> (Self, tokio::sync::mpsc::Sender<Result<axum::body::Bytes, String>>) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        (Self { rx, cur: axum::body::Bytes::new() }, tx)
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.cur.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(b)) => self.cur = b,
                Some(Err(e)) => return Err(std::io::Error::other(e)),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.cur.len());
        buf[..n].copy_from_slice(&self.cur[..n]);
        let _ = self.cur.split_to(n);
        Ok(n)
    }
}

/// How to resolve a top-level name that already exists at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// `name-1`, `name-2`, …
    #[default]
    AutoRename,
    Replace,
    Skip,
}

/// Which of `names` already exist in `dest_dir`.
pub fn conflicts(dest_dir: &Path, names: &[String]) -> Vec<String> {
    names.iter().filter(|n| dest_dir.join(n).exists()).cloned().collect()
}

/// First free `stem-N.ext` next to `name` in `dir`.
pub fn auto_rename(dir: &Path, name: &str) -> String {
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let (stem, ext) = if name.starts_with('.') && p.extension().is_none() { (name.to_string(), String::new()) } else { (stem, ext) };
    (1..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|c| !dir.join(c).exists())
        .unwrap_or_else(|| name.to_string())
}

/// Result of [`unpack`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnpackSummary {
    pub files: u64,
    pub bytes: u64,
    /// Original top-level name → name it was written as.
    pub renamed: HashMap<String, String>,
    pub skipped: Vec<String>,
    pub replaced: Vec<String>,
}

/// Extract a [`pack`] stream into `dest_dir`. Top-level names that already
/// exist follow `renames` (explicit, from the user) else `policy`.
pub fn unpack(
    source: impl Read,
    dest_dir: &Path,
    policy: ConflictPolicy,
    renames: &HashMap<String, String>,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Result<UnpackSummary, String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let dec = zstd::stream::read::Decoder::new(source).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(Counting { inner: dec, n: 0, progress });
    let mut summary = UnpackSummary::default();
    let mut mapped: HashMap<String, Option<String>> = HashMap::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        if cancel() {
            return Err("cancelled".into());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let rel = safe_rel(&path.to_string_lossy())?;
        let mut comps = rel.components();
        let Some(Component::Normal(top)) = comps.next() else { continue };
        let top = top.to_string_lossy().into_owned();
        let rest: PathBuf = comps.collect();
        let target_top = mapped
            .entry(top.clone())
            .or_insert_with(|| {
                if let Some(new) = renames.get(&top).filter(|n| !n.trim().is_empty()) {
                    summary.renamed.insert(top.clone(), new.clone());
                    return Some(new.clone());
                }
                if !dest_dir.join(&top).exists() {
                    return Some(top.clone());
                }
                match policy {
                    ConflictPolicy::Skip => {
                        summary.skipped.push(top.clone());
                        None
                    }
                    ConflictPolicy::Replace => {
                        let p = dest_dir.join(&top);
                        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                        summary.replaced.push(top.clone());
                        Some(top.clone())
                    }
                    ConflictPolicy::AutoRename => {
                        let new = auto_rename(dest_dir, &top);
                        summary.renamed.insert(top.clone(), new.clone());
                        Some(new)
                    }
                }
            })
            .clone();
        let Some(target_top) = target_top else { continue };
        safe_rel(&target_top)?;
        let out = if rest.as_os_str().is_empty() {
            dest_dir.join(&target_top)
        } else {
            dest_dir.join(&target_top).join(&rest)
        };
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else if kind.is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let n = std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
            summary.files += 1;
            summary.bytes += n;
        }
    }
    Ok(summary)
}

/// Top-level archive name for a selected path (`""` = the project itself).
pub fn top_name(project_name: &str, rel: &str) -> String {
    let rel = rel.trim_matches('/');
    if rel.is_empty() {
        crate::cloud_sync::safe_dir_name(project_name)
    } else {
        rel.rsplit('/').next().unwrap_or(rel).to_string()
    }
}

/// One file or folder of a resumable transfer, in sending order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    /// Destination-relative path: `<top>/<sub/path>` (`/`-separated).
    pub rel: String,
    /// Pull side: project the bytes come from, and the project-relative source path.
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub src: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Deterministic, sorted walk of `abs` (skipping hidden folders and
/// symlinks), pushing entries named under `rel` and sourced at `src`.
fn walk_manifest(abs: &Path, rel: &str, project_id: &str, src: &str, out: &mut Vec<ManifestEntry>) {
    let Ok(meta) = std::fs::symlink_metadata(abs) else { return };
    if meta.is_file() {
        out.push(ManifestEntry { rel: rel.into(), project_id: project_id.into(), src: src.into(), is_dir: false, size: meta.len() });
    } else if meta.is_dir() {
        out.push(ManifestEntry { rel: rel.into(), project_id: project_id.into(), src: src.into(), is_dir: true, size: 0 });
        let Ok(rd) = std::fs::read_dir(abs) else { return };
        let mut children: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !hidden(n))
            .collect();
        children.sort();
        for c in children {
            let child_src = if src.is_empty() { c.clone() } else { format!("{src}/{c}") };
            walk_manifest(&abs.join(&c), &format!("{rel}/{c}"), project_id, &child_src, out);
        }
    }
}

/// Manifest for a selection: `(project_id, project_rel_path, abs_path, top_name)` per item.
pub fn manifest(items: &[(String, String, PathBuf, String)]) -> Vec<ManifestEntry> {
    let mut out = Vec::new();
    for (pid, src, abs, top) in items {
        walk_manifest(abs, top, pid, src.trim_matches('/'), &mut out);
    }
    out
}

/// Where each top-level name of a transfer lands (`None` = skipped), decided
/// once up front so a resumed transfer keeps the same renames. `Replace`
/// removes the existing item here.
pub fn plan_tops(
    dest_dir: &Path,
    tops: &[String],
    policy: ConflictPolicy,
    renames: &HashMap<String, String>,
) -> Result<(HashMap<String, Option<String>>, UnpackSummary), String> {
    let mut plan = HashMap::new();
    let mut summary = UnpackSummary::default();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    for top in tops {
        if plan.contains_key(top) {
            continue;
        }
        let target = if let Some(new) = renames.get(top).filter(|n| !n.trim().is_empty()) {
            safe_rel(new)?;
            summary.renamed.insert(top.clone(), new.clone());
            Some(new.clone())
        } else if !dest_dir.join(top).exists() {
            Some(top.clone())
        } else {
            match policy {
                ConflictPolicy::Skip => {
                    summary.skipped.push(top.clone());
                    None
                }
                ConflictPolicy::Replace => {
                    let p = dest_dir.join(top);
                    let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                    summary.replaced.push(top.clone());
                    Some(top.clone())
                }
                ConflictPolicy::AutoRename => {
                    let mut new = auto_rename(dest_dir, top);
                    while taken.contains(&new) {
                        new = auto_rename(dest_dir, &new);
                    }
                    summary.renamed.insert(top.clone(), new.clone());
                    Some(new)
                }
            }
        };
        if let Some(t) = &target {
            taken.insert(t.clone());
        }
        plan.insert(top.clone(), target);
    }
    Ok((plan, summary))
}

/// Local path for manifest entry `rel` under `dest_dir` per `plan` (`None` = skipped).
pub fn planned_target(dest_dir: &Path, plan: &HashMap<String, Option<String>>, rel: &str) -> Result<Option<PathBuf>, String> {
    let rel_path = safe_rel(rel)?;
    let mut comps = rel_path.components();
    let Some(Component::Normal(top)) = comps.next() else { return Err(format!("invalid path: {rel}")) };
    let top = top.to_string_lossy().into_owned();
    let Some(mapped) = plan.get(&top).cloned().unwrap_or(Some(top.clone())) else { return Ok(None) };
    let rest: PathBuf = comps.collect();
    Ok(Some(if rest.as_os_str().is_empty() { dest_dir.join(mapped) } else { dest_dir.join(mapped).join(rest) }))
}

/// In-progress sibling of a file being received.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".rustic-part");
    target.with_file_name(name)
}

/// Bytes already received for `target` (its `.rustic-part`, or the full size
/// when the final file is already complete at `size`).
pub fn received_len(target: &Path, size: u64) -> u64 {
    if let Ok(m) = std::fs::metadata(target) {
        if m.is_file() && m.len() == size && !part_path(target).exists() {
            return size;
        }
    }
    std::fs::metadata(part_path(target)).map(|m| m.len()).unwrap_or(0)
}

/// Open `target`'s part file for appending at `offset` (truncating anything
/// past it, e.g. a half-written chunk).
pub fn open_part_at(target: &Path, offset: u64) -> Result<std::fs::File, String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let part = part_path(target);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    if offset > len {
        return Err(format!("resume offset {offset} is past the {len} bytes received"));
    }
    f.set_len(offset).map_err(|e| e.to_string())?;
    use std::io::Seek;
    let mut f = f;
    f.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    Ok(f)
}

/// Move a fully received part file into place.
pub fn finish_part(target: &Path) -> Result<(), String> {
    let part = part_path(target);
    if !part.exists() {
        return Ok(());
    }
    if target.is_file() {
        let _ = std::fs::remove_file(target);
    }
    std::fs::rename(&part, target).map_err(|e| format!("{}: {e}", target.display()))
}

#[cfg(test)]
mod resumable_tests {
    use super::*;

    #[test]
    fn manifest_is_sorted_and_plan_maps_conflicts() {
        let d = std::env::temp_dir().join(format!("rustic-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/b")).unwrap();
        std::fs::write(d.join("src/z.txt"), "zz").unwrap();
        std::fs::write(d.join("src/b/a.txt"), "a").unwrap();
        std::fs::create_dir_all(d.join("src/.git")).unwrap();
        let m = manifest(&[("p".into(), "src".into(), d.join("src"), "src".into())]);
        let rels: Vec<&str> = m.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["src", "src/b", "src/b/a.txt", "src/z.txt"]);
        assert_eq!(m[2].src, "src/b/a.txt");
        assert_eq!(m[3].size, 2);

        let dest = d.join("dest");
        std::fs::create_dir_all(dest.join("src")).unwrap();
        let (plan, s) = plan_tops(&dest, &["src".into()], ConflictPolicy::AutoRename, &HashMap::new()).unwrap();
        assert_eq!(plan["src"].as_deref(), Some("src-1"));
        assert_eq!(s.renamed["src"], "src-1");
        assert_eq!(planned_target(&dest, &plan, "src/b/a.txt").unwrap().unwrap(), dest.join("src-1").join("b").join("a.txt"));
        let (skip, _) = plan_tops(&dest, &["src".into()], ConflictPolicy::Skip, &HashMap::new()).unwrap();
        assert_eq!(planned_target(&dest, &skip, "src/z.txt").unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn part_files_resume_at_offset_and_finish() {
        let d = std::env::temp_dir().join(format!("rustic-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let t = d.join("x/file.bin");
        assert_eq!(received_len(&t, 10), 0);
        {
            use std::io::Write;
            let mut f = open_part_at(&t, 0).unwrap();
            f.write_all(b"hello").unwrap();
        }
        assert_eq!(received_len(&t, 10), 5);
        {
            use std::io::Write;
            let mut f = open_part_at(&t, 3).unwrap();
            f.write_all(b"LOWORLD").unwrap();
        }
        finish_part(&t).unwrap();
        assert_eq!(std::fs::read(&t).unwrap(), b"helLOWORLD");
        assert_eq!(received_len(&t, 10), 10);
        assert!(open_part_at(&d.join("y"), 4).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustic-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn safe_rel_rejects_escapes() {
        assert!(safe_rel("../x").is_err());
        assert!(safe_rel("a/../../x").is_err());
        assert_eq!(safe_rel("/src/lib/").unwrap(), PathBuf::from("src").join("lib"));
        assert_eq!(safe_rel("").unwrap(), PathBuf::new());
    }

    #[test]
    fn auto_rename_picks_next_free_name() {
        let d = tmp("rename");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        std::fs::write(d.join("a-1.txt"), "x").unwrap();
        assert_eq!(auto_rename(&d, "a.txt"), "a-2.txt");
        std::fs::create_dir(d.join("src")).unwrap();
        assert_eq!(auto_rename(&d, "src"), "src-1");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pack_unpack_round_trip_with_conflicts() {
        let src = tmp("src");
        std::fs::create_dir_all(src.join("lib/inner")).unwrap();
        std::fs::write(src.join("lib/inner/a.rs"), "fn a() {}").unwrap();
        std::fs::write(src.join("readme.md"), "hi").unwrap();
        std::fs::create_dir_all(src.join("lib/.git")).unwrap();
        std::fs::write(src.join("lib/.git/HEAD"), "ref").unwrap();
        let items = vec![
            PackItem { abs: src.join("lib"), name: "lib".into() },
            PackItem { abs: src.join("readme.md"), name: "readme.md".into() },
        ];
        assert_eq!(measure(&items.iter().map(|i| i.abs.clone()).collect::<Vec<_>>()).files, 2);
        let mut buf = Vec::new();
        pack(&items, &mut buf, &|| false, &|_| {}).unwrap();

        let dst = tmp("dst");
        std::fs::write(dst.join("readme.md"), "mine").unwrap();
        let s = unpack(&buf[..], &dst, ConflictPolicy::AutoRename, &HashMap::new(), &|| false, &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("lib/inner/a.rs")).unwrap(), "fn a() {}");
        assert!(!dst.join("lib/.git").exists());
        assert_eq!(std::fs::read_to_string(dst.join("readme.md")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(dst.join("readme-1.md")).unwrap(), "hi");
        assert_eq!(s.renamed.get("readme.md").map(String::as_str), Some("readme-1.md"));

        let s = unpack(&buf[..], &dst, ConflictPolicy::Replace, &HashMap::new(), &|| false, &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("readme.md")).unwrap(), "hi");
        assert!(s.replaced.contains(&"lib".to_string()));

        let mut ren = HashMap::new();
        ren.insert("lib".to_string(), "lib-copy".to_string());
        unpack(&buf[..], &dst, ConflictPolicy::Skip, &ren, &|| false, &|_| {}).unwrap();
        assert!(dst.join("lib-copy/inner/a.rs").exists());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn list_and_preview() {
        let d = tmp("list");
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("b.txt"), "hello").unwrap();
        std::fs::write(d.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let l = list_dir(&d, "").unwrap();
        assert_eq!(l[0].name, "sub");
        assert!(l[0].is_dir);
        let p = read_preview(&d, "b.txt").unwrap();
        assert_eq!(p.kind, "text");
        assert_eq!(p.content.as_deref(), Some("hello"));
        assert_eq!(read_preview(&d, "bin.dat").unwrap().kind, "binary");
        assert!(read_preview(&d, "../x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
