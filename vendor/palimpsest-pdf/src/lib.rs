//! Palimpsest: a pure-Rust engine for editing the content of existing PDF pages.
//!
//! A [`Document`] is loaded from bytes, exposes each page as a list of text runs and
//! images ([`Document::page_objects`]), applies edits addressed by object id, and is
//! written back to bytes. Nothing is rendered; the host re-renders a page after an edit.
//!
//! # Coordinates
//!
//! Every coordinate the API takes or returns is in *page space*: PDF points
//! (1/72 inch), origin at the bottom-left corner of the visible page, x to the right
//! and y up, after the page's `CropBox` and `Rotate` entries have been applied. A
//! viewer showing the page upright at scale `s` therefore maps a page-space point
//! `(x, y)` to the pixel `(x * s, (height - y) * s)`.

use std::fmt;

mod content;
mod doc;
mod edit;
mod embed;
mod font;
pub mod geom;
mod image;
mod interp;
mod layout;
mod model;
mod std14;
mod text;
mod util;

pub use doc::{DocInfo, Document};
pub use edit::Placement;
pub use geom::{Matrix, Rect};
pub use image::ImageData;
pub use interp::Limits;
pub use layout::{Align, PageLayout, TextBlock, TextLine};
pub use model::{
    GlyphBox, ImageInfo, ObjId, ObjectKind, PageObject, PageObjects, PageText, TextRun,
};
pub use text::{Overflow, TextEdit, TextOptions};

/// Errors returned by the engine. Malformed input must surface here, never as a panic.
#[derive(Debug)]
pub enum Error {
    /// The bytes could not be parsed as a PDF document.
    Parse(String),
    /// A page's content stream could not be read or decoded.
    Content { page: u32, reason: String },
    /// The document is encrypted and no password was given.
    PasswordRequired,
    /// The document is encrypted and the password does not open it.
    WrongPassword,
    /// The 1-based page number is out of range.
    NoSuchPage(u32),
    /// The page has no object with this id.
    NoSuchObject { page: u32, id: ObjId },
    /// The edit does not apply to this object or cannot be expressed.
    Unsupported(String),
    /// Replaced text does not fit and the overflow policy is to report it. For a block
    /// both numbers are line counts; for a single line they are widths in points.
    Overflow { needed: f64, available: f64 },
    /// The document could not be written.
    Save(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(reason) => write!(f, "not a readable PDF: {reason}"),
            Error::Content { page, reason } => {
                write!(f, "page {page}: unreadable content stream: {reason}")
            }
            Error::PasswordRequired => write!(f, "the document is encrypted and needs a password"),
            Error::WrongPassword => write!(f, "the password does not open this document"),
            Error::NoSuchPage(page) => write!(f, "page {page} does not exist"),
            Error::NoSuchObject { page, id } => write!(f, "page {page} has no object {id}"),
            Error::Unsupported(reason) => write!(f, "unsupported edit: {reason}"),
            Error::Overflow { needed, available } => write!(
                f,
                "the text does not fit: needs {needed:.1}, has {available:.1}"
            ),
            Error::Save(reason) => write!(f, "could not write the document: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Operator counts for one page's content stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSummary {
    /// 1-based page number.
    pub number: u32,
    /// Total content-stream operations.
    pub operations: usize,
    /// Text-showing operations (`Tj`, `TJ`, `'`, `"`).
    pub text_show_ops: usize,
    /// XObject paint operations (`Do`): images and form XObjects.
    pub xobject_paint_ops: usize,
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod hardening;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_a_generated_page() {
        let doc = Document::from_bytes(&fixtures::simple()).unwrap();
        assert_eq!(doc.page_count(), 1);
        assert_eq!(
            doc.inspect().unwrap(),
            vec![PageSummary {
                number: 1,
                operations: 8,
                text_show_ops: 2,
                xobject_paint_ops: 1
            }]
        );
    }

    #[test]
    fn rejects_bytes_that_are_not_a_pdf() {
        assert!(matches!(
            Document::from_bytes(b"definitely not a pdf"),
            Err(Error::Parse(_))
        ));
        assert!(matches!(Document::from_bytes(b""), Err(Error::Parse(_))));
    }

    #[test]
    fn lists_text_runs_with_boxes_in_page_space() {
        let mut doc = Document::from_bytes(&fixtures::simple()).unwrap();
        let page = doc.page_objects(1).unwrap().clone();
        assert_eq!((page.width, page.height, page.rotate), (595.0, 842.0, 0));
        assert_eq!(page.text, PageText::Text);
        let runs: Vec<&TextRun> = page
            .objects
            .iter()
            .filter_map(PageObject::as_text)
            .collect();
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["First run", "Second run"]
        );
        assert_eq!(runs[0].font, "Helvetica");
        assert_eq!(runs[0].size, 24.0);
        assert_eq!(runs[0].origin, (72.0, 700.0));
        assert_eq!(runs[1].origin, (72.0, 670.0));
        // "First run" in Helvetica at 24pt: widths 611+222+333+500+278+278+333+556+556 = 3667/1000 em.
        let first = &page.objects[0];
        assert!((first.bbox.x0 - 72.0).abs() < 1e-6);
        assert!(
            (first.bbox.width() - 3.667 * 24.0).abs() < 1e-3,
            "width {}",
            first.bbox.width()
        );
        assert_eq!(runs[0].glyphs.len(), 9);
        assert!(page.to_json().contains("\"text\":\"First run\""));
    }

    #[test]
    fn resolves_rotation_and_crop_box() {
        let mut doc = Document::from_bytes(&fixtures::rotated()).unwrap();
        let page = doc.page_objects(1).unwrap().clone();
        // CropBox 100 100 500 700 (400 x 600), rotated 90 degrees clockwise.
        assert_eq!((page.width, page.height, page.rotate), (600.0, 400.0, 90));
        let run = page.objects[0].as_text().unwrap();
        // Text origin (172, 700) in user space is 72pt right of and 600pt above the crop origin;
        // after a clockwise quarter turn that is 600 across and 400 - 72 up.
        assert!(
            (run.origin.0 - 600.0).abs() < 1e-6 && (run.origin.1 - 328.0).abs() < 1e-6,
            "{:?}",
            run.origin
        );
        assert!((run.angle + 90.0).abs() < 1e-6);
    }

    #[test]
    fn reports_pages_without_text() {
        let mut doc = Document::from_bytes(&fixtures::outlined()).unwrap();
        let page = doc.page_objects(1).unwrap();
        assert_eq!(page.text, PageText::Outlined);
        assert!(!page.text.is_editable());
        assert!(page.objects.is_empty());
    }
}
